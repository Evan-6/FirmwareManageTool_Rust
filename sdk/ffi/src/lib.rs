//! C ABI over the GDBOX SDK. C++, Python and Go bindings all call these
//! functions, so behaviour is defined once, here.
//!
//! Conventions:
//! * Every function returning `int32_t` returns an `ErrorCode` (0 = ok).
//! * After a failure, `gdb_last_error` returns the same code and a UTF-8 message.
//! * Functions returning pointers return NULL on failure.
//! * Strings returned to the caller must be freed with `gdb_string_free`.
//! * No function panics across the boundary; panics become `Io` errors.
//!
//! The pointers are checked for NULL but their validity is the caller's
//! contract, which is why this lint is allowed crate-wide.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use gdb_motion::{Ease, HumanProfile};
use gdb_sdk::error::{Error, ErrorCode};
use gdb_sdk::event::{ConnectionState, EventHandler};
use gdb_sdk::log::{self, Level};
use gdb_sdk::manager::Manager;
use gdb_sdk::mock::MockHandle;
use gdb_sdk::motion::{MotionOptions, Movement};
use gdb_sdk::task::{Priority, Scheduler, TaskId};
use gdb_sdk::{Button, Device, DeviceOptions, TypeTextConfig};
use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::Arc;
use std::time::Duration;

thread_local! {
    static LAST_ERROR: RefCell<(i32, String)> = const { RefCell::new((0, String::new())) };
}

fn set_last(error: &Error) {
    LAST_ERROR.with(|slot| {
        let mut slot = slot.borrow_mut();
        slot.0 = i32::from(error.code().as_u8());
        slot.1 = error.message.clone();
    });
}

fn clear_last() {
    LAST_ERROR.with(|slot| slot.borrow_mut().0 = 0);
}

fn guard<T>(body: impl FnOnce() -> gdb_sdk::Result<T>) -> gdb_sdk::Result<T> {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(result) => result,
        Err(payload) => {
            let message = payload
                .downcast_ref::<&str>()
                .map(|text| (*text).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic".to_string());
            Err(Error::io(format!("panic across the C ABI: {message}")))
        }
    }
}

fn code(result: gdb_sdk::Result<()>) -> i32 {
    match result {
        Ok(()) => {
            clear_last();
            0
        }
        Err(error) => {
            let value = i32::from(error.code().as_u8());
            set_last(&error);
            value
        }
    }
}

fn pointer<T>(result: gdb_sdk::Result<Box<T>>) -> *mut T {
    match result {
        Ok(value) => Box::into_raw(value),
        Err(error) => {
            set_last(&error);
            ptr::null_mut()
        }
    }
}

fn json_string(result: gdb_sdk::Result<String>) -> *mut c_char {
    match result.and_then(|text| {
        CString::new(text).map_err(|_| Error::invalid("JSON contained a NUL byte"))
    }) {
        Ok(value) => value.into_raw(),
        Err(error) => {
            set_last(&error);
            ptr::null_mut()
        }
    }
}

unsafe fn device<'a>(raw: *const GdbDevice) -> gdb_sdk::Result<&'a GdbDevice> {
    unsafe { raw.as_ref() }.ok_or_else(|| Error::invalid("device handle is NULL"))
}

unsafe fn movement<'a>(raw: *mut GdbMovement) -> gdb_sdk::Result<&'a mut GdbMovement> {
    unsafe { raw.as_mut() }.ok_or_else(|| Error::invalid("movement handle is NULL"))
}

unsafe fn scheduler<'a>(raw: *const GdbScheduler) -> gdb_sdk::Result<&'a Scheduler> {
    unsafe { raw.as_ref() }
        .map(|handle| &handle.scheduler)
        .ok_or_else(|| Error::invalid("scheduler handle is NULL"))
}

pub struct GdbDevice {
    device: Device,
    _mock: Option<MockHandle>,
}

pub struct GdbMovement {
    movement: Option<Movement>,
}

pub struct GdbScheduler {
    scheduler: Scheduler,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GdbPoint {
    pub x: i32,
    pub y: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GdbKeyStep {
    pub page: u16,
    pub usage: u16,
    pub hold_ms: u32,
    pub gap_ms: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GdbProfile {
    /// 0 jerk, 1 ease-in-out, 2 linear, 3 ease-in, 4 ease-out, 5 cubic bezier.
    pub ease: i32,
    pub bezier: [f64; 4],
    pub curve: f64,
    pub jitter_px: f64,
    pub overshoot_pct: f64,
    pub settle_ms: u32,
    pub min_step_px: i32,
}

impl GdbProfile {
    fn to_profile(self) -> HumanProfile {
        let ease = match self.ease {
            1 => Ease::EaseInOut,
            2 => Ease::Linear,
            3 => Ease::EaseIn,
            4 => Ease::EaseOut,
            5 => Ease::CubicBezier {
                x1: self.bezier[0],
                y1: self.bezier[1],
                x2: self.bezier[2],
                y2: self.bezier[3],
            },
            _ => Ease::Jerk,
        };
        HumanProfile {
            curve: self.curve,
            jitter_px: self.jitter_px,
            overshoot_pct: self.overshoot_pct,
            settle_ms: self.settle_ms,
            ease,
            min_step_px: self.min_step_px,
        }
    }
}

unsafe fn motion_options(duration_ms: u32, profile: *const GdbProfile) -> MotionOptions {
    let profile = unsafe { profile.as_ref() }
        .copied()
        .map(GdbProfile::to_profile)
        .unwrap_or_default();
    MotionOptions {
        duration_ms,
        profile,
        // The C ABI has no target yet: absolute moves stay on the primary monitor.
        ..MotionOptions::default()
    }
}

type EventFn = extern "C" fn(user: *mut c_void, kind: i32, code: i32, message: *const c_char);

struct FfiHandler {
    func: EventFn,
    user: usize,
}

unsafe impl Send for FfiHandler {}
unsafe impl Sync for FfiHandler {}

impl EventHandler for FfiHandler {
    fn state(&self, state: ConnectionState) {
        (self.func)(self.user as *mut c_void, 0, state as i32, ptr::null());
    }
    fn error(&self, error: &Error) {
        let message = CString::new(error.message.clone()).unwrap_or_default();
        (self.func)(
            self.user as *mut c_void,
            2,
            i32::from(error.code().as_u8()),
            message.as_ptr(),
        );
    }
}

type TaskFn = extern "C" fn(user: *mut c_void) -> i32;

fn task_code(func: TaskFn, user: usize) -> gdb_sdk::Result<()> {
    let code = func(user as *mut c_void);
    if code == 0 {
        Ok(())
    } else {
        Err(Error::new(
            ErrorCode::from_u8(code as u8),
            "task callback failed",
        ))
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_abi_version() -> u32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_version() -> *const c_char {
    c"0.1.0".as_ptr()
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_last_error(buffer: *mut c_char, capacity: usize) -> i32 {
    LAST_ERROR.with(|slot| {
        let slot = slot.borrow();
        if !buffer.is_null() && capacity > 0 {
            let mut bytes = slot.1.as_bytes().to_vec();
            bytes.push(0);
            let count = bytes.len().min(capacity);
            unsafe {
                ptr::copy_nonoverlapping(bytes.as_ptr().cast::<c_char>(), buffer, count);
                if count == capacity {
                    *buffer.add(capacity - 1) = 0;
                }
            }
        }
        slot.0
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_string_free(text: *mut c_char) {
    if !text.is_null() {
        unsafe { drop(CString::from_raw(text)) };
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_set_log_level(level: u8) -> i32 {
    log::set_level(match level {
        0 => Level::Off,
        1 => Level::Error,
        2 => Level::Warn,
        3 => Level::Info,
        4 => Level::Debug,
        _ => Level::Trace,
    });
    0
}

type LogFn = extern "C" fn(user: *mut c_void, level: i32, message: *const c_char);

#[unsafe(no_mangle)]
pub extern "C" fn gdb_set_log_sink(func: Option<LogFn>, user: *mut c_void) -> i32 {
    match func {
        Some(func) => {
            let user = user as usize;
            log::set_sink(Some(Box::new(move |level, message| {
                let text = CString::new(message.to_string()).unwrap_or_default();
                func(user as *mut c_void, level as i32, text.as_ptr());
            })));
        }
        None => log::set_sink(None),
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_device_list() -> *mut c_char {
    json_string(guard(|| {
        serde_json::to_string(&Manager::list()?).map_err(|error| Error::io(error.to_string()))
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_open(
    selector: *const c_char,
    timeout_ms: u32,
    auto_reconnect: bool,
    heartbeat: bool,
) -> *mut GdbDevice {
    let selector = if selector.is_null() {
        None
    } else {
        Some(
            unsafe { CStr::from_ptr(selector) }
                .to_string_lossy()
                .into_owned(),
        )
    };
    pointer(guard(|| {
        let device = Manager::open(
            selector.as_deref(),
            DeviceOptions {
                auto_reconnect,
                timeout_ms: timeout_ms.max(100),
                heartbeat,
            },
        )?;
        Ok(Box::new(GdbDevice {
            device,
            _mock: None,
        }))
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_open_mock(
    timeout_ms: u32,
    auto_reconnect: bool,
    heartbeat: bool,
) -> *mut GdbDevice {
    pointer(guard(|| {
        let (device, handle) = Manager::open_mock_with_handle(DeviceOptions {
            auto_reconnect,
            timeout_ms: timeout_ms.max(100),
            heartbeat,
        })?;
        Ok(Box::new(GdbDevice {
            device,
            _mock: Some(handle),
        }))
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_device_free(raw: *mut GdbDevice) {
    if raw.is_null() {
        return;
    }
    let handle = unsafe { Box::from_raw(raw) };
    let _ = handle.device.close();
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_close(raw: *mut GdbDevice) -> i32 {
    code(guard(|| unsafe { device(raw) }?.device.close()))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_reconnect(raw: *mut GdbDevice) -> i32 {
    code(guard(|| unsafe { device(raw) }?.device.reconnect()))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_set_auto_reconnect(raw: *mut GdbDevice, enabled: bool) -> i32 {
    code(guard(|| {
        unsafe { device(raw) }?.device.set_auto_reconnect(enabled);
        Ok(())
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_device_state(raw: *const GdbDevice) -> i32 {
    match guard(|| Ok(unsafe { device(raw) }?.device.state())) {
        Ok(state) => state as i32,
        Err(error) => {
            set_last(&error);
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_device_info(raw: *const GdbDevice) -> *mut c_char {
    json_string(guard(|| {
        let info = unsafe { device(raw) }?.device.info();
        serde_json::to_string(&info).map_err(|error| Error::io(error.to_string()))
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_status(raw: *const GdbDevice) -> *mut c_char {
    json_string(guard(|| {
        let status = unsafe { device(raw) }?.device.status()?;
        Ok(serde_json::json!({
            "received": status.received,
            "completed": status.completed,
            "pending": status.pending,
            "leaseMs": status.lease_ms,
            "rx": status.rx,
            "tx": status.tx,
            "hid": status.hid,
            "failsafe": status.failsafe,
            "state": {
                "modifiers": status.state.modifiers,
                "keys": status.state.keys,
                "consumer": status.state.consumer,
                "buttons": status.state.buttons,
            }
        })
        .to_string())
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_status_ext(raw: *const GdbDevice) -> *mut c_char {
    json_string(guard(|| {
        let ext = unsafe { device(raw) }?.device.status_ext()?;
        Ok(serde_json::json!({
            "uptimeMs": ext.uptime_ms,
            "lateMaxUs": ext.late_max_us,
            "outDepth": ext.out_depth,
            "rxDepth": ext.rx_depth,
            "txDepth": ext.tx_depth,
            "sessions": ext.sessions,
            "lastFault": ext.last_fault,
            "lastFaultAgeMs": ext.last_fault_age_ms,
            "resetReason": ext.reset_reason,
            "bootCount": ext.boot_count,
            "build": ext.build,
        })
        .to_string())
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_sessions(raw: *const GdbDevice) -> *mut c_char {
    json_string(guard(|| {
        let sessions = unsafe { device(raw) }?.device.sessions()?;
        serde_json::to_string(&sessions).map_err(|error| Error::io(error.to_string()))
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_release_all(raw: *mut GdbDevice) -> i32 {
    code(guard(|| unsafe { device(raw) }?.device.release_all()))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_barrier(raw: *mut GdbDevice) -> i32 {
    code(guard(|| unsafe { device(raw) }?.device.barrier()))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_bootloader(raw: *mut GdbDevice) -> i32 {
    code(guard(|| unsafe { device(raw) }?.device.bootloader()))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_set_event_handler(
    raw: *mut GdbDevice,
    func: Option<EventFn>,
    user: *mut c_void,
) -> i32 {
    code(guard(|| {
        let handle = unsafe { device(raw) }?;
        match func {
            Some(func) => handle.device.set_event_handler(Some(Arc::new(FfiHandler {
                func,
                user: user as usize,
            }))),
            None => handle.device.set_event_handler(None),
        }
        Ok(())
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_profile_default() -> GdbProfile {
    let profile = HumanProfile::default();
    GdbProfile {
        ease: match profile.ease {
            Ease::EaseInOut => 1,
            Ease::Linear => 2,
            Ease::EaseIn => 3,
            Ease::EaseOut => 4,
            Ease::CubicBezier { .. } => 5,
            Ease::Jerk => 0,
        },
        bezier: [0.42, 0.0, 0.58, 1.0],
        curve: profile.curve,
        jitter_px: profile.jitter_px,
        overshoot_pct: profile.overshoot_pct,
        settle_ms: profile.settle_ms,
        min_step_px: profile.min_step_px,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_mouse_move_by(
    raw: *mut GdbDevice,
    dx: i32,
    dy: i32,
    duration_ms: u32,
    profile: *const GdbProfile,
) -> i32 {
    code(guard(|| {
        let handle = unsafe { device(raw) }?;
        let options = unsafe { motion_options(duration_ms, profile) };
        handle.device.mouse().move_by(dx, dy, options)
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_mouse_move_to(
    raw: *mut GdbDevice,
    x: i32,
    y: i32,
    duration_ms: u32,
    profile: *const GdbProfile,
) -> i32 {
    code(guard(|| {
        let handle = unsafe { device(raw) }?;
        let options = unsafe { motion_options(duration_ms, profile) };
        handle.device.mouse().move_to(x, y, options)
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_mouse_move_path(
    raw: *mut GdbDevice,
    points: *const GdbPoint,
    count: usize,
    duration_ms: u32,
    profile: *const GdbProfile,
) -> i32 {
    code(guard(|| {
        let handle = unsafe { device(raw) }?;
        if points.is_null() || count == 0 {
            return Err(Error::invalid("path needs at least one point"));
        }
        let slice = unsafe { std::slice::from_raw_parts(points, count) };
        let points: Vec<(i32, i32)> = slice.iter().map(|point| (point.x, point.y)).collect();
        let options = unsafe { motion_options(duration_ms, profile) };
        handle.device.mouse().move_path(&points, options)
    }))
}

fn spawn(raw: *mut GdbDevice, spawn_fn: impl FnOnce(&Device) -> Movement) -> *mut GdbMovement {
    pointer(guard(|| {
        let handle = unsafe { device(raw) }?;
        Ok(Box::new(GdbMovement {
            movement: Some(spawn_fn(&handle.device)),
        }))
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_mouse_start_move_to(
    raw: *mut GdbDevice,
    x: i32,
    y: i32,
    duration_ms: u32,
    profile: *const GdbProfile,
) -> *mut GdbMovement {
    let options = unsafe { motion_options(duration_ms, profile) };
    spawn(raw, |device| device.mouse().start_move_to(x, y, options))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_mouse_start_move_by(
    raw: *mut GdbDevice,
    dx: i32,
    dy: i32,
    duration_ms: u32,
    profile: *const GdbProfile,
) -> *mut GdbMovement {
    let options = unsafe { motion_options(duration_ms, profile) };
    spawn(raw, |device| device.mouse().start_move_by(dx, dy, options))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_mouse_start_move_path(
    raw: *mut GdbDevice,
    points: *const GdbPoint,
    count: usize,
    duration_ms: u32,
    profile: *const GdbProfile,
) -> *mut GdbMovement {
    if points.is_null() || count == 0 {
        set_last(&Error::invalid("path needs at least one point"));
        return ptr::null_mut();
    }
    let slice = unsafe { std::slice::from_raw_parts(points, count) };
    let points: Vec<(i32, i32)> = slice.iter().map(|point| (point.x, point.y)).collect();
    let options = unsafe { motion_options(duration_ms, profile) };
    spawn(raw, move |device| {
        device.mouse().start_move_path(points, options)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_movement_cancel(raw: *mut GdbMovement) -> i32 {
    code(guard(|| {
        unsafe { movement(raw) }?
            .movement
            .as_ref()
            .map(|movement| movement.cancel())
            .ok_or_else(|| Error::invalid("movement already finished"))
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_movement_retarget(raw: *mut GdbMovement, x: i32, y: i32) -> i32 {
    code(guard(|| {
        unsafe { movement(raw) }?
            .movement
            .as_ref()
            .map(|movement| movement.retarget(x, y))
            .ok_or_else(|| Error::invalid("movement already finished"))
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_movement_is_finished(raw: *const GdbMovement) -> bool {
    unsafe { raw.as_ref() }
        .and_then(|handle| handle.movement.as_ref())
        .is_some_and(|movement| movement.is_finished())
}

/// Waits up to `timeout_ms`; the handle stays valid either way.
#[unsafe(no_mangle)]
pub extern "C" fn gdb_movement_wait(raw: *mut GdbMovement, timeout_ms: u32) -> i32 {
    code(guard(|| {
        let handle = unsafe { movement(raw) }?;
        let movement = handle
            .movement
            .as_mut()
            .ok_or_else(|| Error::invalid("movement already finished"))?;
        movement.wait_timeout(Duration::from_millis(u64::from(timeout_ms.max(1))))
    }))
}

/// Frees the handle, cancelling it if it is still running.
#[unsafe(no_mangle)]
pub extern "C" fn gdb_movement_free(raw: *mut GdbMovement) {
    if raw.is_null() {
        return;
    }
    let mut handle = unsafe { Box::from_raw(raw) };
    if let Some(movement) = handle.movement.take() {
        movement.cancel();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_mouse_button(raw: *mut GdbDevice, button: u8, down: bool) -> i32 {
    code(guard(|| {
        let handle = unsafe { device(raw) }?;
        let button = Button::from_u8(button)?;
        handle.device.mouse().button(button, down)
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_mouse_click(raw: *mut GdbDevice, button: u8, count: u8) -> i32 {
    code(guard(|| {
        let handle = unsafe { device(raw) }?;
        let button = Button::from_u8(button)?;
        handle.device.mouse().click(button, count)
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_mouse_double_click(raw: *mut GdbDevice, button: u8) -> i32 {
    code(guard(|| {
        let handle = unsafe { device(raw) }?;
        let button = Button::from_u8(button)?;
        handle.device.mouse().double_click(button)
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_mouse_wheel(raw: *mut GdbDevice, vertical: i32, horizontal: i32) -> i32 {
    code(guard(|| {
        unsafe { device(raw) }?
            .device
            .mouse()
            .wheel(vertical, horizontal)
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_mouse_drag(
    raw: *mut GdbDevice,
    button: u8,
    x: i32,
    y: i32,
    duration_ms: u32,
    profile: *const GdbProfile,
) -> i32 {
    code(guard(|| {
        let handle = unsafe { device(raw) }?;
        let button = Button::from_u8(button)?;
        let options = unsafe { motion_options(duration_ms, profile) };
        handle.device.mouse().drag(button, x, y, options)
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_key_down(raw: *mut GdbDevice, page: u16, usage: u16) -> i32 {
    code(guard(|| {
        unsafe { device(raw) }?
            .device
            .keyboard()
            .key_down(page, usage)
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_key_up(raw: *mut GdbDevice, page: u16, usage: u16) -> i32 {
    code(guard(|| {
        unsafe { device(raw) }?
            .device
            .keyboard()
            .key_up(page, usage)
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_key_press(raw: *mut GdbDevice, page: u16, usage: u16) -> i32 {
    code(guard(|| {
        unsafe { device(raw) }?.device.keyboard().press(page, usage)
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_key_long_press(
    raw: *mut GdbDevice,
    page: u16,
    usage: u16,
    hold_ms: u32,
) -> i32 {
    code(guard(|| {
        unsafe { device(raw) }?
            .device
            .keyboard()
            .long_press(page, usage, hold_ms)
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_keys_chord(
    raw: *mut GdbDevice,
    usages: *const u16,
    count: usize,
    hold_ms: u32,
) -> i32 {
    code(guard(|| {
        let handle = unsafe { device(raw) }?;
        if usages.is_null() || count == 0 {
            return Err(Error::invalid("chord needs at least one usage"));
        }
        let slice = unsafe { std::slice::from_raw_parts(usages, count) };
        handle.device.keyboard().chord(slice, hold_ms)
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_keys_sequence(
    raw: *mut GdbDevice,
    steps: *const GdbKeyStep,
    count: usize,
) -> i32 {
    code(guard(|| {
        let handle = unsafe { device(raw) }?;
        if steps.is_null() || count == 0 {
            return Err(Error::invalid("sequence needs at least one step"));
        }
        let slice = unsafe { std::slice::from_raw_parts(steps, count) };
        let steps: Vec<gdb_sdk::keyboard::KeyStep> = slice
            .iter()
            .map(|step| gdb_sdk::keyboard::KeyStep {
                page: step.page,
                usage: step.usage,
                hold_ms: step.hold_ms,
                gap_ms: step.gap_ms,
            })
            .collect();
        handle.device.keyboard().sequence(&steps)
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_type_text(
    raw: *mut GdbDevice,
    text: *const c_char,
    interval_ms: u32,
    hold_ms: u32,
    jitter_ms: u32,
) -> i32 {
    code(guard(|| {
        let handle = unsafe { device(raw) }?;
        if text.is_null() {
            return Err(Error::invalid("text is NULL"));
        }
        let text = unsafe { CStr::from_ptr(text) }
            .to_str()
            .map_err(|_| Error::invalid("text is not valid UTF-8"))?
            .to_string();
        handle.device.keyboard().type_text(&TypeTextConfig {
            text,
            interval_ms,
            hold_ms,
            jitter_ms,
        })
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_scheduler_new(raw: *mut GdbDevice) -> *mut GdbScheduler {
    pointer(guard(|| {
        let handle = unsafe { device(raw) }?;
        Ok(Box::new(GdbScheduler {
            scheduler: handle.device.scheduler(),
        }))
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_scheduler_free(raw: *mut GdbScheduler) {
    if !raw.is_null() {
        drop(unsafe { Box::from_raw(raw) });
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_scheduler_submit(
    raw: *mut GdbScheduler,
    delay_ms: u32,
    priority: i32,
    func: Option<TaskFn>,
    user: *mut c_void,
) -> u64 {
    match guard(|| {
        let scheduler = unsafe { scheduler(raw) }?;
        let func = func.ok_or_else(|| Error::invalid("task callback is NULL"))?;
        let user = user as usize;
        scheduler.submit(
            Duration::from_millis(u64::from(delay_ms)),
            Priority(priority),
            move |_device: &Device| task_code(func, user),
        )
    }) {
        Ok(id) => id,
        Err(error) => {
            set_last(&error);
            0
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_scheduler_every(
    raw: *mut GdbScheduler,
    interval_ms: u32,
    priority: i32,
    func: Option<TaskFn>,
    user: *mut c_void,
) -> u64 {
    match guard(|| {
        let scheduler = unsafe { scheduler(raw) }?;
        let func = func.ok_or_else(|| Error::invalid("task callback is NULL"))?;
        if interval_ms == 0 {
            return Err(Error::invalid("interval must be positive"));
        }
        let user = user as usize;
        scheduler.every(
            Duration::from_millis(u64::from(interval_ms)),
            Priority(priority),
            move |_device: &Device| task_code(func, user),
        )
    }) {
        Ok(id) => id,
        Err(error) => {
            set_last(&error);
            0
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_scheduler_cancel(raw: *mut GdbScheduler, id: TaskId) -> i32 {
    code(guard(|| {
        unsafe { scheduler(raw) }?.cancel(id);
        Ok(())
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_scheduler_pause(raw: *mut GdbScheduler) -> i32 {
    code(guard(|| {
        unsafe { scheduler(raw) }?.pause();
        Ok(())
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_scheduler_resume(raw: *mut GdbScheduler) -> i32 {
    code(guard(|| {
        unsafe { scheduler(raw) }?.resume();
        Ok(())
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_scheduler_wait_idle(raw: *mut GdbScheduler, timeout_ms: u32) -> i32 {
    code(guard(|| {
        unsafe { scheduler(raw) }?.wait_idle(Duration::from_millis(u64::from(timeout_ms.max(1))))
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_scheduler_take_errors(raw: *mut GdbScheduler) -> *mut c_char {
    json_string(guard(|| {
        let errors = unsafe { scheduler(raw) }?.take_errors();
        let body: Vec<serde_json::Value> = errors
            .into_iter()
            .map(|(task_id, error)| {
                serde_json::json!({
                    "taskId": task_id,
                    "code": error.code().as_u8(),
                    "message": error.message,
                })
            })
            .collect();
        Ok(serde_json::Value::Array(body).to_string())
    }))
}
