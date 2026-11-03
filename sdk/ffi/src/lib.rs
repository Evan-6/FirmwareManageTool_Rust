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
use gdb_sdk::broker;
use gdb_sdk::error::{Error, ErrorCode};
use gdb_sdk::event::{ConnectionState, EventHandler};
use gdb_sdk::log::{self, Level};
use gdb_sdk::manager::Manager;
use gdb_sdk::metrics::MetricsSnapshot;
use gdb_sdk::mock::MockHandle;
use gdb_sdk::motion::{MotionOptions, Movement};
use gdb_sdk::platform::Screen;
use gdb_sdk::probe::{self, Observer, Probe};
use gdb_sdk::task::{Priority, Scheduler, TaskId};
use gdb_sdk::{Button, Device, DeviceInfo, DeviceOptions, TypeTextConfig};
use input_protocol::KeyId;
use input_protocol::v3::{self, Command, InputState, SessionInfo, Status, StatusExt};
use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

mod watch;

// The failure the last call on *this* thread reported.
//
// A host in another language reads it in a second call, which is why it is a
// pair of write-then-read values rather than a return; but it is per thread, so
// two threads that fail different calls never read each other's message. A
// device that commands from several threads — heartbeat, scheduler worker,
// movement — wants `gdb_device_last_error` instead, which is the device's own
// record and survives the thread that hit the failure.
thread_local! {
    static LAST: RefCell<(i32, String)> = const { RefCell::new((0, String::new())) };
}

fn set_last(error: &Error) {
    LAST.with(|slot| {
        let mut slot = slot.borrow_mut();
        slot.0 = i32::from(error.code().as_u8());
        slot.1.clear();
        slot.1.push_str(&error.message);
    });
}

/// The success path: one store, no lock, nothing allocated.
fn clear_last() {
    LAST.with(|slot| {
        let mut slot = slot.borrow_mut();
        slot.0 = 0;
        slot.1.clear();
    });
}

fn last_error() -> (i32, String) {
    LAST.with(|slot| slot.borrow().clone())
}

/// Writes `message` into the caller's buffer as a NUL-terminated UTF-8 string,
/// truncated to fit. A NULL buffer or a zero capacity copies nothing.
fn copy_message(buffer: *mut c_char, capacity: usize, message: &str) {
    if buffer.is_null() || capacity == 0 {
        return;
    }
    let mut bytes = message.as_bytes().to_vec();
    bytes.push(0);
    let count = bytes.len().min(capacity);
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr().cast::<c_char>(), buffer, count);
        if count == capacity {
            *buffer.add(capacity - 1) = 0;
        }
    }
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
            // The C struct has no knob for it: a caller that asked for jitter
            // gets per-move variation, one that asked for none gets a repeatable
            // straight path.
            organic: if self.jitter_px > 0.0 { 0.6 } else { 0.0 },
        }
    }
}

/// One reply packet. The payload belongs to the caller until `gdb_packet_free`,
/// and `opcode == 0` means the command had no reply at all.
#[repr(C)]
pub struct GdbPacket {
    pub opcode: u8,
    pub flags: u8,
    pub session: u32,
    pub sequence: u32,
    /// Owned allocation, NULL when `len == 0`.
    pub payload: *mut u8,
    pub len: usize,
}

impl GdbPacket {
    const fn empty() -> Self {
        Self {
            opcode: 0,
            flags: 0,
            session: 0,
            sequence: 0,
            payload: ptr::null_mut(),
            len: 0,
        }
    }

    fn from_packet(packet: v3::Packet) -> Self {
        let boxed = packet.payload.into_boxed_slice();
        let len = boxed.len();
        let payload = if len == 0 {
            ptr::null_mut()
        } else {
            Box::into_raw(boxed).cast::<u8>()
        };
        Self {
            opcode: packet.opcode,
            flags: packet.flags,
            session: packet.session,
            sequence: packet.sequence,
            payload,
            len,
        }
    }
}

/// Decodes one wire command into the typed command the SDK encodes.
///
/// The SDK sends typed `Command` values, so the C ABI translates the caller's
/// `(opcode, payload)` into the matching variant: the frame that leaves is byte
/// for byte the caller's, and the protocol's own range checks still apply.
/// Opcodes the protocol does not define are `GDB_INVALID`.
fn command_from_wire(opcode: u8, payload: &[u8]) -> gdb_sdk::Result<Command> {
    let fixed = |command: Command| {
        if payload.is_empty() {
            Ok(command)
        } else {
            Err(Error::invalid(format!(
                "opcode {opcode} takes no payload, got {} bytes",
                payload.len()
            )))
        }
    };
    let pair = || -> gdb_sdk::Result<(i16, i16)> {
        if payload.len() != 4 {
            return Err(Error::invalid(format!(
                "opcode {opcode} needs 4 payload bytes, got {}",
                payload.len()
            )));
        }
        Ok((
            i16::from_le_bytes([payload[0], payload[1]]),
            i16::from_le_bytes([payload[2], payload[3]]),
        ))
    };
    Ok(match opcode {
        v3::OPEN => fixed(Command::Open)?,
        v3::CLOSE => fixed(Command::Close)?,
        v3::HEARTBEAT => fixed(Command::Heartbeat)?,
        v3::STATUS => fixed(Command::Status)?,
        v3::BARRIER => fixed(Command::Barrier)?,
        v3::RELEASE_ALL => fixed(Command::ReleaseAll)?,
        v3::STATUS_EXT => fixed(Command::StatusExt)?,
        v3::SESSION_LIST => fixed(Command::SessionList)?,
        v3::BOOTLOADER => fixed(Command::Bootloader)?,
        v3::KEY => {
            if payload.len() != 5 {
                return Err(Error::invalid(format!(
                    "opcode {opcode} needs 5 payload bytes, got {}",
                    payload.len()
                )));
            }
            let key = KeyId {
                usage_page: u16::from_le_bytes([payload[0], payload[1]]),
                usage: u16::from_le_bytes([payload[2], payload[3]]),
            };
            Command::Key(key, payload[4] != 0)
        }
        v3::SNAPSHOT => Command::Snapshot(
            InputState::decode(payload)
                .map_err(|error| Error::invalid(format!("invalid snapshot: {error}")))?,
        ),
        v3::MOUSE_MOVE => {
            let (dx, dy) = pair()?;
            Command::MouseMove(dx, dy)
        }
        v3::MOUSE_BUTTONS => {
            if payload.len() != 1 {
                return Err(Error::invalid(format!(
                    "opcode {opcode} needs 1 payload byte, got {}",
                    payload.len()
                )));
            }
            Command::MouseButtons(payload[0])
        }
        v3::WHEEL => {
            let (vertical, horizontal) = pair()?;
            Command::Wheel(vertical, horizontal)
        }
        _ => {
            return Err(Error::invalid(format!(
                "opcode {opcode} is not a command the protocol can send"
            )));
        }
    })
}

// The JSON shapes below are the binding contract: a direct handle and an
// attached one hand out the same bytes, so a caller can switch between them
// without touching its parsing.
fn info_json(info: &DeviceInfo) -> gdb_sdk::Result<String> {
    serde_json::to_string(info).map_err(|error| Error::io(error.to_string()))
}

fn status_json(status: &Status) -> String {
    serde_json::json!({
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
    .to_string()
}

fn status_ext_json(ext: &StatusExt) -> String {
    serde_json::json!({
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
    .to_string()
}

fn sessions_json(sessions: &[SessionInfo]) -> gdb_sdk::Result<String> {
    serde_json::to_string(sessions).map_err(|error| Error::io(error.to_string()))
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
type EventJsonFn = extern "C" fn(user: *mut c_void, kind: i32, json: *const c_char);

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

/// The same events, one JSON object per call instead of two loose arguments.
///
/// The payload carries what the plain callback only implies: the state value,
/// the error code with its message, and the last STATUS the device answered when
/// there is one. The device is held weakly, so installing a handler cannot keep
/// its own host alive.
struct FfiJsonHandler {
    func: EventJsonFn,
    user: usize,
    device: gdb_sdk::WeakDevice,
}

unsafe impl Send for FfiJsonHandler {}
unsafe impl Sync for FfiJsonHandler {}

impl FfiJsonHandler {
    fn emit(&self, kind: i32, body: serde_json::Value) {
        let text = CString::new(body.to_string()).unwrap_or_default();
        (self.func)(self.user as *mut c_void, kind, text.as_ptr());
    }

    /// The device's last STATUS, or JSON null when none has been read.
    fn status(&self) -> serde_json::Value {
        match self
            .device
            .upgrade()
            .and_then(|device| device.status_summary())
        {
            Some(status) => serde_json::json!({
                "received": status.received,
                "completed": status.completed,
                "pending": status.pending,
            }),
            None => serde_json::Value::Null,
        }
    }
}

impl EventHandler for FfiJsonHandler {
    fn state(&self, state: ConnectionState) {
        self.emit(
            0,
            serde_json::json!({"state": state as i32, "status": self.status()}),
        );
    }
    fn error(&self, error: &Error) {
        self.emit(
            2,
            serde_json::json!({
                "code": error.code().as_u8(),
                "message": error.message,
                "status": self.status(),
            }),
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

/// The minor revision this build implements. `gdb.h` carries the same number in
/// `GDB_ABI_VERSION_MINOR`; the test below fails when they drift apart.
const ABI_VERSION_MINOR: u32 = 9;

/// The major and the minor at run time, for a host that has to know whether the
/// library it loaded is the one its bindings were written against.
///
/// `gdb_abi_version` keeps returning the major alone. `GDB_INVALID` on a NULL
/// out pointer, and neither slot is written in that case.
#[unsafe(no_mangle)]
pub extern "C" fn gdb_abi_version_full(major: *mut u32, minor: *mut u32) -> i32 {
    code(guard(|| {
        let major = unsafe { major.as_mut() }.ok_or_else(|| Error::invalid("major is NULL"))?;
        let minor = unsafe { minor.as_mut() }.ok_or_else(|| Error::invalid("minor is NULL"))?;
        *major = gdb_abi_version();
        *minor = ABI_VERSION_MINOR;
        Ok(())
    }))
}

/// What this build can do, as JSON object of booleans.
///
/// A host that checks `GDB_ABI_VERSION_MINOR` learns the revision; this says
/// which of the abilities behind it are actually present, so a feature that is
/// additive but platform-bound (window metrics need Windows) can be skipped
/// instead of failing at the first call. A capability spelled `false` here has
/// no C symbol at all: the bindings cannot reach it in any language.
#[unsafe(no_mangle)]
pub extern "C" fn gdb_capabilities() -> *mut c_char {
    json_string(guard(|| {
        let windows = cfg!(windows);
        let body = serde_json::json!({
            "click_timed": true,
            "click_at_on": true,
            "window_metrics": windows,
            "window_client_screen": windows,
            "pursuit": true,
            "move_report": true,
            "event_json": true,
            "scheduler": true,
            "broker": windows,
            "replay": false,
            "flash": false,
        });
        Ok(body.to_string())
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_version() -> *const c_char {
    c"0.1.0".as_ptr()
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_last_error(buffer: *mut c_char, capacity: usize) -> i32 {
    let (code, message) = last_error();
    copy_message(buffer, capacity, &message);
    code
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

/// Opens the first board matching `selector`.
///
/// Every open raises the process to per-monitor DPI awareness first, because the
/// coordinates a host passes in are physical pixels and the SDK reads the
/// pointer with `GetPhysicalCursorPos`: a process left DPI-unaware would aim at
/// a scaled picture of the desktop. `gdb_platform_prepare` reports the mode.
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
        gdb_sdk::platform::enable_dpi_awareness();
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
        gdb_sdk::platform::enable_dpi_awareness();
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

/// The same board, opened for use alongside other programs: a reply that never
/// arrives is `GDB_BUSY` naming the command instead of `GDB_TIMEOUT`, and the
/// session is never reopened behind the caller's back.
#[unsafe(no_mangle)]
pub extern "C" fn gdb_open_shared(
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
        gdb_sdk::platform::enable_dpi_awareness();
        let device = Device::open_shared(
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
pub extern "C" fn gdb_open_mock_shared(
    timeout_ms: u32,
    auto_reconnect: bool,
    heartbeat: bool,
) -> *mut GdbDevice {
    pointer(guard(|| {
        gdb_sdk::platform::enable_dpi_awareness();
        let (device, handle) = Device::open_mock_shared(DeviceOptions {
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

/// The mock device on a desktop the caller describes, so the monitor arithmetic
/// can be exercised with no display attached:
/// `{"cursor":[x,y],"monitors":[{"rect":[l,t,r,b],"dpi":144,"primary":true}],"windows":[{"hwnd":…,"rect":[…],"client":[…]}]}`.
///
/// `cursor` defaults to the primary monitor's top-left, `primary` and `dpi` to
/// false and 96, and `windows` may be left out. A layout with no monitor, an
/// empty rectangle, more than one primary, a window anchor with handle 0 or a
/// client area outside its frame is `GDB_INVALID` with a message naming the
/// entry. NULL on failure, with gdb_last_error set.
#[unsafe(no_mangle)]
pub extern "C" fn gdb_open_mock_layout(
    json: *const c_char,
    timeout_ms: u32,
    auto_reconnect: bool,
    heartbeat: bool,
) -> *mut GdbDevice {
    open_mock_layout(json, timeout_ms, auto_reconnect, heartbeat, false)
}

/// [`gdb_open_mock_layout`] for a board other programs may drive too: a taken
/// reply is `GDB_BUSY` and the session is never reopened on its own.
#[unsafe(no_mangle)]
pub extern "C" fn gdb_open_mock_layout_shared(
    json: *const c_char,
    timeout_ms: u32,
    auto_reconnect: bool,
    heartbeat: bool,
) -> *mut GdbDevice {
    open_mock_layout(json, timeout_ms, auto_reconnect, heartbeat, true)
}

fn open_mock_layout(
    json: *const c_char,
    timeout_ms: u32,
    auto_reconnect: bool,
    heartbeat: bool,
    shared: bool,
) -> *mut GdbDevice {
    let json = if json.is_null() {
        None
    } else {
        Some(
            unsafe { CStr::from_ptr(json) }
                .to_string_lossy()
                .into_owned(),
        )
    };
    pointer(guard(|| {
        gdb_sdk::platform::enable_dpi_awareness();
        let json = json
            .as_deref()
            .ok_or_else(|| Error::invalid("gdb_open_mock_layout needs the layout JSON"))?;
        let options = DeviceOptions {
            auto_reconnect,
            timeout_ms: timeout_ms.max(100),
            heartbeat,
        };
        let (device, handle) = Manager::open_mock_layout_with_handle(json, options)?;
        if shared {
            device.set_ownership(gdb_sdk::Ownership::Shared);
        }
        Ok(Box::new(GdbDevice {
            device,
            _mock: Some(handle),
        }))
    }))
}

/// The monitors this handle's absolute moves are anchored to, as the array
/// `gdb_monitors_json` returns: the layout a mock device was opened with, or the
/// system's own list for a real board. Index `i` is what a monitor anchor takes.
#[unsafe(no_mangle)]
pub extern "C" fn gdb_device_monitors_json(raw: *const GdbDevice) -> *mut c_char {
    json_string(guard(|| {
        let handle = unsafe { device(raw) }?;
        handle.device.monitors_json()
    }))
}

/// Frames the reply loop looked past because they answered another session or
/// another sequence. 0 on a NULL handle, with gdb_last_error set.
#[unsafe(no_mangle)]
pub extern "C" fn gdb_stolen_frames(raw: *const GdbDevice) -> u64 {
    match guard(|| Ok(unsafe { device(raw) }?.device.stolen_frames())) {
        Ok(count) => count,
        Err(error) => {
            set_last(&error);
            0
        }
    }
}

/// The latest failure the device recorded, whoever called it, as the message
/// `gdb_last_error` would have carried on that thread plus the structure:
/// which opcode failed, under which session, and the last STATUS the device
/// answered when one has been read.
///
/// Returns the code and fills `buffer`; 0 means the device has not failed yet,
/// and a NULL handle is `GDB_INVALID`. Unlike `gdb_last_error`, a success does
/// not clear it: a command that failed on the heartbeat or a scheduler worker is
/// still readable here from the main thread.
#[unsafe(no_mangle)]
pub extern "C" fn gdb_device_last_error(
    raw: *const GdbDevice,
    buffer: *mut c_char,
    capacity: usize,
) -> i32 {
    match guard(|| Ok(unsafe { device(raw) }?.device.last_error())) {
        Ok(Some(error)) => {
            copy_message(buffer, capacity, &error.describe());
            i32::from(error.code)
        }
        Ok(None) => {
            copy_message(buffer, capacity, "");
            0
        }
        Err(error) => {
            set_last(&error);
            copy_message(buffer, capacity, &error.message);
            i32::from(error.code().as_u8())
        }
    }
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
        info_json(&info)
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_status(raw: *const GdbDevice) -> *mut c_char {
    json_string(guard(|| {
        let status = unsafe { device(raw) }?.device.status()?;
        Ok(status_json(&status))
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_status_ext(raw: *const GdbDevice) -> *mut c_char {
    json_string(guard(|| {
        let ext = unsafe { device(raw) }?.device.status_ext()?;
        Ok(status_ext_json(&ext))
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_sessions(raw: *const GdbDevice) -> *mut c_char {
    json_string(guard(|| {
        let sessions = unsafe { device(raw) }?.device.sessions()?;
        sessions_json(&sessions)
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

/// Sends one protocol command by its wire opcode and payload.
///
/// This is `Device::command`: the same session, sequence numbers and reconnect
/// path as every other call, but the SDK's bookkeeping (held buttons, the mock
/// cursor) is not updated. A control command leaves its reply in `out`, an
/// input command leaves `out` zeroed. `out` may be NULL when the reply is not
/// wanted, and its payload must be released with `gdb_packet_free`.
#[unsafe(no_mangle)]
pub extern "C" fn gdb_command(
    raw: *mut GdbDevice,
    opcode: u8,
    payload: *const u8,
    len: usize,
    out: *mut GdbPacket,
) -> i32 {
    code(guard(|| {
        let handle = unsafe { device(raw) }?;
        let bytes = unsafe { payload_bytes(payload, len) }?;
        let reply = handle.device.command(&command_from_wire(opcode, bytes)?)?;
        store_packet(out, reply);
        Ok(())
    }))
}

/// Releases a packet produced by `gdb_command`; safe on a zeroed packet.
#[unsafe(no_mangle)]
pub extern "C" fn gdb_packet_free(raw: *mut GdbPacket) {
    let Some(slot) = (unsafe { raw.as_mut() }) else {
        return;
    };
    let (payload, len) = (slot.payload, slot.len);
    *slot = GdbPacket::empty();
    if !payload.is_null() && len > 0 {
        unsafe { drop(Box::from_raw(ptr::slice_from_raw_parts_mut(payload, len))) };
    }
}

/// One relative move of any size, split at the ±1024 report limit and sent with
/// no barrier: `Device::mouse().move_raw`.
#[unsafe(no_mangle)]
pub extern "C" fn gdb_move_raw(raw: *mut GdbDevice, dx: i32, dy: i32) -> i32 {
    code(guard(|| {
        unsafe { device(raw) }?.device.mouse().move_raw(dx, dy)
    }))
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

/// The same events as one JSON object per call: `{"state":2,"status":{…}}` for a
/// state change and `{"code":2,"message":"…","status":{…}}` for an error, with
/// `status` null until a STATUS has been read.
///
/// It replaces the plain handler — one slot, one callback — and passing NULL
/// clears it. kind is the same `gdb_event_kind`, and the JSON pointer is valid
/// only for the duration of the call, so copy what outlives it.
#[unsafe(no_mangle)]
pub extern "C" fn gdb_set_event_handler_json(
    raw: *mut GdbDevice,
    func: Option<EventJsonFn>,
    user: *mut c_void,
) -> i32 {
    code(guard(|| {
        let handle = unsafe { device(raw) }?;
        match func {
            Some(func) => handle
                .device
                .set_event_handler(Some(Arc::new(FfiJsonHandler {
                    func,
                    user: user as usize,
                    device: handle.device.downgrade(),
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

/// A click with the press length and the gap the caller asked for. A zero
/// `hold_ms` or `gap_ms` draws that half the way `gdb_mouse_click` does;
/// `jitter_ms` spreads each half by up to that much and 0 is exact.
#[unsafe(no_mangle)]
pub extern "C" fn gdb_mouse_click_timed(
    raw: *mut GdbDevice,
    button: u8,
    count: u8,
    hold_ms: u32,
    gap_ms: u32,
    jitter_ms: u32,
) -> i32 {
    code(guard(|| {
        let handle = unsafe { device(raw) }?;
        let button = Button::from_u8(button)?;
        handle
            .device
            .mouse()
            .click_timed(button, count, hold_ms, gap_ms, jitter_ms)
    }))
}

/// Aim and click in one call: an absolute move clamped into `screen`, then a
/// click with the caller's timing.
#[unsafe(no_mangle)]
pub extern "C" fn gdb_mouse_click_at_on(
    raw: *mut GdbDevice,
    x: i32,
    y: i32,
    screen: GdbScreen,
    button: u8,
    count: u8,
    hold_ms: u32,
    gap_ms: u32,
    jitter_ms: u32,
) -> i32 {
    code(guard(|| {
        let handle = unsafe { device(raw) }?;
        let button = Button::from_u8(button)?;
        let target = screen_from(screen.kind, screen.arg)?;
        handle
            .device
            .mouse()
            .click_at(x, y, target, button, count, hold_ms, gap_ms, jitter_ms)
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
        handle.device.keyboard().type_text(&TypeTextConfig::paced(
            text,
            interval_ms,
            hold_ms,
            jitter_ms,
        ))
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
pub extern "C" fn gdb_scheduler_submit_at(
    raw: *mut GdbScheduler,
    due_ms: u64,
    priority: i32,
    func: Option<TaskFn>,
    user: *mut c_void,
) -> u64 {
    match guard(|| {
        let scheduler = unsafe { scheduler(raw) }?;
        let func = func.ok_or_else(|| Error::invalid("task callback is NULL"))?;
        let user = user as usize;
        scheduler.submit_at(due_ms, Priority(priority), move |_device: &Device| {
            task_code(func, user)
        })
    }) {
        Ok(id) => id,
        Err(error) => {
            set_last(&error);
            0
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_scheduler_elapsed_ms(raw: *mut GdbScheduler) -> u64 {
    match guard(|| {
        let scheduler = unsafe { scheduler(raw) }?;
        Ok(scheduler.elapsed_ms())
    }) {
        Ok(ms) => ms,
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

// ------------------------------------------------------------------ presence
//
// What a program may read while another program owns the board. Feature 14 is a
// control transfer: it never touches the interrupt IN queue, so this reads the
// identity with no session and no packet. No OPEN either, so no owner slot is
// taken and the program that holds one keeps it.
//
// `gdb_probe` returns the object `gdb_device_info` returns and `gdb_probe_all`
// an array of them; both are freed with `gdb_string_free`.
//
// The observer handle is that same read on a timer. It has no command that can
// succeed: each one returns GDB_UNSUPPORTED with a message naming the reason, so
// a host that swaps a device handle for an observer gets a typed refusal instead
// of a write it cannot take back.

pub struct GdbObserver {
    observer: Mutex<Observer>,
}

fn selector_text(selector: *const c_char) -> Option<String> {
    if selector.is_null() {
        return None;
    }
    Some(
        unsafe { CStr::from_ptr(selector) }
            .to_string_lossy()
            .into_owned(),
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_probe(selector: *const c_char) -> *mut c_char {
    let wanted = selector_text(selector);
    json_string(guard(|| {
        let probe = probe::read_selector(wanted.as_deref())?;
        info_json(&probe.info())
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_probe_all() -> *mut c_char {
    json_string(guard(|| {
        let infos: Vec<DeviceInfo> = probe::all()?.iter().map(Probe::info).collect();
        serde_json::to_string(&infos).map_err(|error| Error::io(error.to_string()))
    }))
}

unsafe fn observer<'a>(raw: *const GdbObserver) -> gdb_sdk::Result<MutexGuard<'a, Observer>> {
    let handle =
        unsafe { raw.as_ref() }.ok_or_else(|| Error::invalid("observer handle is NULL"))?;
    Ok(handle
        .observer
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_observer_start(
    selector: *const c_char,
    interval_ms: u32,
) -> *mut GdbObserver {
    let wanted = selector_text(selector);
    pointer(guard(|| {
        let interval = Duration::from_millis(u64::from(interval_ms.max(1)));
        let observer = Observer::start_selector(wanted.as_deref(), interval, |_| {})?;
        Ok(Box::new(GdbObserver {
            observer: Mutex::new(observer),
        }))
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_observer_stop(raw: *mut GdbObserver) {
    if raw.is_null() {
        return;
    }
    let handle = unsafe { &*raw };
    handle
        .observer
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .stop();
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_observer_free(raw: *mut GdbObserver) {
    if raw.is_null() {
        return;
    }
    // Observer::drop stops the thread; nothing is sent on the way out.
    drop(unsafe { Box::from_raw(raw) });
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_observer_info(raw: *const GdbObserver) -> *mut c_char {
    json_string(guard(|| {
        let observer = unsafe { observer(raw) }?;
        info_json(&observer.info())
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_observer_state(raw: *const GdbObserver) -> i32 {
    match guard(|| Ok(unsafe { observer(raw) }?.state() as i32)) {
        Ok(state) => state,
        Err(error) => {
            set_last(&error);
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_observer_status(raw: *const GdbObserver) -> i32 {
    code(guard(|| unsafe { observer(raw) }?.status().map(|_| ())))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_observer_barrier(raw: *const GdbObserver) -> i32 {
    code(guard(|| unsafe { observer(raw) }?.barrier()))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_observer_release_all(raw: *const GdbObserver) -> i32 {
    code(guard(|| unsafe { observer(raw) }?.release_all()))
}

/// The escape hatch, refused: an observer never sends a frame, so the opcode is
/// not even parsed and `out` is left zeroed.
#[unsafe(no_mangle)]
pub extern "C" fn gdb_observer_command(
    raw: *const GdbObserver,
    _opcode: u8,
    _payload: *const u8,
    _len: usize,
    out: *mut GdbPacket,
) -> i32 {
    code(guard(|| {
        let observer = unsafe { observer(raw) }?;
        if !out.is_null() {
            unsafe { *out = GdbPacket::empty() };
        }
        Err(observer.refuse())
    }))
}

// ------------------------------------------------------------------- remote
//
// A second surface, deliberately apart from the symbols above: a broker process
// (`gdb::broker::Server`) owns the vendor collection and this handle is a
// client of it, one JSON object per line over `\\.\pipe\gdbox-<name>`. Only one
// process may read the collection, so this is how a second DLL client works at
// all. The direct handle keeps its own shape, so a program may use both.
//
// What an attached handle does not offer: raw HID reports, the capability scan,
// host-side motion workers (`start_move_to`, drags, `type_text`), flashing and
// the bootloader. Those need the collection itself, which the broker holds.

/// Names the executable `gdb_remote_spawn` starts: a host built on this SDK,
/// which serves `--gdbox-broker <name>` and otherwise does its own job.
pub const BROKER_EXE_ENV: &str = "GDBOX_BROKER_EXE";

pub struct GdbRemote {
    client: broker::Client,
}

/// `NULL` and `""` both mean the default pipe, as on the command line.
fn remote_name(name: *const c_char) -> gdb_sdk::Result<String> {
    if name.is_null() {
        return Ok(broker::DEFAULT_NAME.to_string());
    }
    let text = unsafe { CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned();
    if text.is_empty() {
        Ok(broker::DEFAULT_NAME.to_string())
    } else {
        Ok(text)
    }
}

unsafe fn remote<'a>(raw: *const GdbRemote) -> gdb_sdk::Result<&'a GdbRemote> {
    unsafe { raw.as_ref() }.ok_or_else(|| Error::invalid("remote handle is NULL"))
}

unsafe fn payload_bytes<'a>(payload: *const u8, len: usize) -> gdb_sdk::Result<&'a [u8]> {
    if len == 0 {
        Ok(&[])
    } else if payload.is_null() {
        Err(Error::invalid("payload is NULL but len is not zero"))
    } else {
        Ok(unsafe { std::slice::from_raw_parts(payload, len) })
    }
}

/// Writes the reply into the caller's slot, if it has one.
fn store_packet(out: *mut GdbPacket, reply: Option<v3::Packet>) {
    if out.is_null() {
        return;
    }
    let slot = unsafe { &mut *out };
    *slot = match reply {
        Some(packet) => GdbPacket::from_packet(packet),
        None => GdbPacket::empty(),
    };
}

/// Attaches to the broker on `\\.\pipe\gdbox-<name>` and stores the handle in
/// `*out`, which is zeroed first so a failure never leaves a stale pointer.
#[unsafe(no_mangle)]
pub extern "C" fn gdb_attach(name: *const c_char, out: *mut *mut GdbRemote) -> i32 {
    code(guard(|| {
        if out.is_null() {
            return Err(Error::invalid("out pointer is NULL"));
        }
        unsafe { *out = ptr::null_mut() };
        let name = remote_name(name)?;
        let client = attach_client(&name)?;
        unsafe { *out = Box::into_raw(Box::new(GdbRemote { client })) };
        Ok(())
    }))
}

/// A listening broker always has a spare instance, so one connect is enough; the
/// bounded wait only distinguishes "nothing is listening" from "the broker is
/// coming back".
fn attach_client(name: &str) -> gdb_sdk::Result<broker::Client> {
    broker::Client::connect_timeout(name, Duration::from_millis(500))
}

/// Starts a broker when none is listening on `name`, then returns; attach
/// afterwards. The executable comes from `GDBOX_BROKER_EXE`; without it there is
/// nothing to start, so this is `GDB_UNSUPPORTED` (a broker already listening is
/// `GDB_OK`). The child is detached and outlives the caller.
#[unsafe(no_mangle)]
pub extern "C" fn gdb_remote_spawn(name: *const c_char) -> i32 {
    code(guard(|| {
        let name = remote_name(name)?;
        if broker::Client::available(&name) {
            return Ok(());
        }
        let exe = std::env::var_os(BROKER_EXE_ENV)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                Error::unsupported(format!(
                    "no broker is listening on {name} and {BROKER_EXE_ENV} names no executable to \
                     start; point it at a host that serves `--gdbox-broker {name}`"
                ))
            })?;
        // The child is detached: dropping the client only ends this probe.
        drop(broker::Client::connect_or_spawn(&name, &exe)?);
        Ok(())
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_remote_free(raw: *mut GdbRemote) {
    if raw.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(raw) });
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_remote_info(raw: *const GdbRemote) -> *mut c_char {
    json_string(guard(|| {
        let info = unsafe { remote(raw) }?.client.info()?;
        info_json(&info)
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_remote_status(raw: *const GdbRemote) -> *mut c_char {
    json_string(guard(|| {
        let status = unsafe { remote(raw) }?.client.status()?;
        Ok(status_json(&status))
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_remote_status_ext(raw: *const GdbRemote) -> *mut c_char {
    json_string(guard(|| {
        let ext = unsafe { remote(raw) }?.client.status_ext()?;
        Ok(status_ext_json(&ext))
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_remote_sessions(raw: *const GdbRemote) -> *mut c_char {
    json_string(guard(|| {
        let sessions = unsafe { remote(raw) }?.client.sessions()?;
        sessions_json(&sessions)
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_remote_key(raw: *mut GdbRemote, page: u16, usage: u16, down: bool) -> i32 {
    code(guard(|| {
        unsafe { remote(raw) }?.client.key(page, usage, down)
    }))
}

/// One MOUSE_MOVE report; the deltas must fit the protocol's ±1024.
#[unsafe(no_mangle)]
pub extern "C" fn gdb_remote_mouse_move(raw: *mut GdbRemote, dx: i16, dy: i16) -> i32 {
    code(guard(|| unsafe { remote(raw) }?.client.mouse_move(dx, dy)))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_remote_move_by(
    raw: *mut GdbRemote,
    dx: i32,
    dy: i32,
    duration_ms: u32,
    profile: *const GdbProfile,
) -> i32 {
    code(guard(|| {
        let handle = unsafe { remote(raw) }?;
        let options = unsafe { motion_options(duration_ms, profile) };
        handle.client.move_by(dx, dy, options)
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_remote_move_raw(raw: *mut GdbRemote, dx: i32, dy: i32) -> i32 {
    code(guard(|| unsafe { remote(raw) }?.client.move_raw(dx, dy)))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_remote_mouse_buttons(raw: *mut GdbRemote, bits: u8) -> i32 {
    code(guard(|| unsafe { remote(raw) }?.client.mouse_buttons(bits)))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_remote_wheel(raw: *mut GdbRemote, vertical: i32, horizontal: i32) -> i32 {
    code(guard(|| {
        unsafe { remote(raw) }?.client.wheel(vertical, horizontal)
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_remote_barrier(raw: *mut GdbRemote) -> i32 {
    code(guard(|| unsafe { remote(raw) }?.client.barrier()))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_remote_release_all(raw: *mut GdbRemote) -> i32 {
    code(guard(|| unsafe { remote(raw) }?.client.release_all()))
}

/// `gdb_command` over the pipe: the broker sends the same frame and hands back
/// the device's reply, and `out` is filled exactly as the direct call fills it.
#[unsafe(no_mangle)]
pub extern "C" fn gdb_remote_command(
    raw: *mut GdbRemote,
    opcode: u8,
    payload: *const u8,
    len: usize,
    out: *mut GdbPacket,
) -> i32 {
    code(guard(|| {
        let handle = unsafe { remote(raw) }?;
        let bytes = unsafe { payload_bytes(payload, len) }?;
        let reply = handle.client.command(&command_from_wire(opcode, bytes)?)?;
        store_packet(out, reply);
        Ok(())
    }))
}

/// Hangs up on the broker; the handle stays valid and later calls report
/// `GDB_DISCONNECTED` until it is freed.
#[unsafe(no_mangle)]
pub extern "C" fn gdb_remote_close(raw: *mut GdbRemote) -> i32 {
    code(guard(|| {
        unsafe { remote(raw) }?.client.close();
        Ok(())
    }))
}

/// Counters a host can display without a copy of the SDK.
#[repr(C)]
pub struct GdbMetrics {
    pub frames_sent: u64,
    pub frames_received: u64,
    pub retries: u64,
    pub stolen_frames: u64,
    pub reconnects: u64,
    pub lease_faults: u64,
    pub timeouts: u64,
    pub latency_count: u64,
    pub latency_min_us: u64,
    pub latency_p50_us: u64,
    pub latency_p95_us: u64,
    pub latency_max_us: u64,
}

impl From<MetricsSnapshot> for GdbMetrics {
    fn from(m: MetricsSnapshot) -> Self {
        Self {
            frames_sent: m.frames_sent,
            frames_received: m.frames_received,
            retries: m.retries,
            stolen_frames: m.stolen_frames,
            reconnects: m.reconnects,
            lease_faults: m.lease_faults,
            timeouts: m.timeouts,
            latency_count: m.latency.count,
            latency_min_us: m.latency.min_us,
            latency_p50_us: m.latency.p50_us,
            latency_p95_us: m.latency.p95_us,
            latency_max_us: m.latency.max_us,
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_metrics_get(raw: *const GdbDevice, out: *mut GdbMetrics) -> i32 {
    code(guard(|| {
        let handle = unsafe { device(raw) }?;
        let target = unsafe { out.as_mut() }.ok_or_else(|| Error::invalid("out is NULL"))?;
        *target = GdbMetrics::from(handle.device.metrics());
        Ok(())
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_metrics_reset(raw: *mut GdbDevice) -> i32 {
    code(guard(|| {
        let handle = unsafe { device(raw) }?;
        handle.device.reset_metrics();
        Ok(())
    }))
}

/// Clamped absolute moves, laid out as gdb.h's gdb_clamps.
#[repr(C)]
pub struct GdbClamps {
    pub clamped_total: u64,
    pub requested_x: i32,
    pub requested_y: i32,
    pub applied_x: i32,
    pub applied_y: i32,
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_metrics_clamps(raw: *const GdbDevice, out: *mut GdbClamps) -> i32 {
    code(guard(|| {
        let handle = unsafe { device(raw) }?;
        let target = unsafe { out.as_mut() }.ok_or_else(|| Error::invalid("out is NULL"))?;
        let metrics = handle.device.metrics();
        let clamp = metrics.last_clamp.unwrap_or_default();
        *target = GdbClamps {
            clamped_total: metrics.clamped_total,
            requested_x: clamp.requested_x,
            requested_y: clamp.requested_y,
            applied_x: clamp.applied_x,
            applied_y: clamp.applied_y,
        };
        Ok(())
    }))
}

/// Every monitor, in the order the screen kinds index them.
#[unsafe(no_mangle)]
pub extern "C" fn gdb_monitors_json() -> *mut c_char {
    json_string(guard(gdb_sdk::platform::monitors_json))
}

/// Makes the process DPI aware (per-monitor v2 where the system allows it) and
/// reports the mode it ended up in, as
/// `{"supported":true,"dpiAwareness":"perMonitorAware","value":2,"perMonitor":true}`.
///
/// A host converts a screenshot coordinate into a screen coordinate; both sides
/// have to agree on the scale factor, and the factor is decided by this process
/// wide setting. Calling it is safe more than once and safe beside an open
/// device. It never fails on a platform without DPI awareness: `supported` is
/// false there and the coordinates are used as they are.
#[unsafe(no_mangle)]
pub extern "C" fn gdb_platform_prepare() -> *mut c_char {
    json_string(guard(gdb_sdk::platform::prepare_json))
}

/// One window as the system reports it, as JSON:
/// `{"rect":{"left","top","right","bottom"},"clientOrigin":{"x","y"},
///   "clientSize":{"width","height"},"dpi":96,"visible":true,
///   "minimized":false,"foreground":false}`.
///
/// `rect` is the frame a window anchor clamps into; `clientOrigin` and
/// `clientSize` describe the area inside it, which is what a window-client
/// anchor uses and what a capture shows. `dpi` is the effective DPI of the
/// window's display (96 = 100%, 0 when the system will not say). GDB_UNSUPPORTED
/// where the platform has no window APIs and GDB_STALE_WINDOW for a handle that
/// does not resolve; the result is freed with gdb_string_free.
#[unsafe(no_mangle)]
pub extern "C" fn gdb_window_metrics(hwnd: i64) -> *mut c_char {
    json_string(guard(|| {
        gdb_sdk::platform::window_metrics_json(hwnd as isize)
    }))
}

/// Screen target, laid out as gdb.h's gdb_screen.
#[repr(C)]
pub struct GdbScreen {
    pub kind: i32,
    pub arg: i64,
}

/// 0 primary, 1 monitor (arg = index), 2 window (arg = HWND), 3 the monitor
/// containing a point (arg = x in the high 32 bits, y in the low 32), 4 window
/// client (arg = HWND).
fn screen_from(kind: i32, arg: i64) -> gdb_sdk::Result<Screen> {
    match kind {
        0 => Ok(Screen::Primary),
        1 => Ok(Screen::Monitor(arg as usize)),
        2 => Ok(Screen::Window(arg as isize)),
        3 => Ok(Screen::Containing(
            (arg as u64 >> 32) as u32 as i32,
            arg as u32 as i32,
        )),
        4 => Ok(Screen::WindowClient(arg as isize)),
        other => Err(Error::invalid(format!(
            "unknown screen kind {other}: 0 is primary, 1 a monitor index, 2 a window, \
             3 the monitor containing a packed point, 4 a window's client area"
        ))),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_mouse_move_to_on(
    raw: *mut GdbDevice,
    x: i32,
    y: i32,
    screen: GdbScreen,
    duration_ms: u32,
    profile: *const GdbProfile,
) -> i32 {
    code(guard(|| {
        let handle = unsafe { device(raw) }?;
        let options = unsafe { motion_options(duration_ms, profile) };
        let options = MotionOptions {
            screen: screen_from(screen.kind, screen.arg)?,
            ..options
        };
        handle.device.mouse().move_to(x, y, options)
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_mouse_move_path_on(
    raw: *mut GdbDevice,
    points: *const GdbPoint,
    count: usize,
    screen: GdbScreen,
    duration_ms: u32,
    profile: *const GdbProfile,
) -> i32 {
    code(guard(|| {
        let handle = unsafe { device(raw) }?;
        if points.is_null() || count == 0 {
            return Err(Error::invalid("path needs at least one point"));
        }
        let slice = unsafe { std::slice::from_raw_parts(points, count) };
        let list: Vec<(i32, i32)> = slice.iter().map(|point| (point.x, point.y)).collect();
        let options = unsafe { motion_options(duration_ms, profile) };
        let options = MotionOptions {
            screen: screen_from(screen.kind, screen.arg)?,
            ..options
        };
        handle.device.mouse().move_path(&list, options)
    }))
}

/// Where an absolute move was asked to go, where the pointer was measured when
/// it finished, whether the anchor had to pull the point back, and how long it
/// took. Laid out as gdb.h's `gdb_move_report`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GdbMoveReport {
    pub requested_x: i32,
    pub requested_y: i32,
    pub applied_x: i32,
    pub applied_y: i32,
    pub clamped: bool,
    pub elapsed_us: u64,
}

/// An absolute move that also reports where the pointer ended.
///
/// `applied_*` is a measurement, taken after the move converged, never the
/// requested point echoed back: a host can compute the remaining error itself.
/// `clamped` is true when the anchor rectangle pulled the destination inside it,
/// which is what a wrong anchor looks like. `out` must not be NULL and is
/// untouched when the call fails; the move is `gdb_mouse_move_to_on`'s.
#[unsafe(no_mangle)]
pub extern "C" fn gdb_mouse_move_to_on_report(
    raw: *mut GdbDevice,
    x: i32,
    y: i32,
    screen: GdbScreen,
    duration_ms: u32,
    profile: *const GdbProfile,
    out: *mut GdbMoveReport,
) -> i32 {
    code(guard(|| {
        let handle = unsafe { device(raw) }?;
        let target = unsafe { out.as_mut() }.ok_or_else(|| Error::invalid("out is NULL"))?;
        let options = unsafe { motion_options(duration_ms, profile) };
        let options = MotionOptions {
            screen: screen_from(screen.kind, screen.arg)?,
            ..options
        };
        let report = handle.device.mouse().move_to_report(x, y, options)?;
        *target = GdbMoveReport {
            requested_x: report.requested_x,
            requested_y: report.requested_y,
            applied_x: report.applied_x,
            applied_y: report.applied_y,
            clamped: report.clamped,
            elapsed_us: report.elapsed_us,
        };
        Ok(())
    }))
}

/// An absolute move in pursuit mode: read the pointer, send what is left,
/// repeat, with no trajectory and no barrier per step.
///
/// This is the mode for a caller that retargets every frame; the device's own
/// report rate sets the speed, so `duration_ms` and the human profile play no
/// part. `MotionOptions::pursuit()` in Rust, and the same clamping and error
/// codes as gdb_mouse_move_to_on.
#[unsafe(no_mangle)]
pub extern "C" fn gdb_mouse_move_to_on_pursuit(
    raw: *mut GdbDevice,
    x: i32,
    y: i32,
    screen: GdbScreen,
) -> i32 {
    code(guard(|| {
        let handle = unsafe { device(raw) }?;
        let options = MotionOptions::pursuit().with_screen(screen_from(screen.kind, screen.arg)?);
        handle.device.mouse().move_to(x, y, options)
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_mouse_start_move_to_on(
    raw: *mut GdbDevice,
    x: i32,
    y: i32,
    screen: GdbScreen,
    duration_ms: u32,
    profile: *const GdbProfile,
) -> *mut GdbMovement {
    let outcome = guard(|| {
        let handle = unsafe { device(raw) }?;
        let options = unsafe { motion_options(duration_ms, profile) };
        let options = MotionOptions {
            screen: screen_from(screen.kind, screen.arg)?,
            ..options
        };
        Ok(Box::into_raw(Box::new(GdbMovement {
            movement: Some(handle.device.mouse().start_move_to(x, y, options)),
        })))
    });
    match outcome {
        Ok(pointer) => pointer,
        Err(error) => {
            set_last(&error);
            std::ptr::null_mut()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::thread;
    use std::time::Instant;

    fn open_mock() -> *mut GdbDevice {
        let raw = gdb_open_mock(1000, false, false);
        assert!(!raw.is_null(), "the mock device did not open");
        raw
    }

    /// The minor `gdb.h` declares, read from the header the DLL was built with.
    fn header_minor() -> u32 {
        let header = include_str!("../include/gdb.h");
        let marker = "#define GDB_ABI_VERSION_MINOR ";
        let start = header.find(marker).expect("gdb.h has no minor");
        header[start + marker.len()..]
            .chars()
            .take_while(|character| character.is_ascii_digit())
            .collect::<String>()
            .parse()
            .expect("the minor is not a number")
    }

    fn writes(raw: *mut GdbDevice) -> Vec<v3::Packet> {
        let handle = unsafe { &*raw };
        handle
            ._mock
            .as_ref()
            .expect("mock device without a handle")
            .writes()
    }

    #[test]
    fn command_writes_the_exact_frame_and_returns_the_reply() {
        let raw = open_mock();
        let frame = [0x2c, 0x01, 0xd8, 0xff]; // MOUSE_MOVE dx=300 dy=-40
        let mut reply = GdbPacket::empty();
        let result = gdb_command(raw, v3::MOUSE_MOVE, frame.as_ptr(), frame.len(), &mut reply);
        assert_eq!(result, 0, "gdb_command rejected a valid frame");
        assert_eq!(reply.opcode, 0, "an input command has no reply");

        let sent = writes(raw);
        let last = sent.last().expect("no frame left the session");
        assert_eq!(last.opcode, v3::MOUSE_MOVE);
        assert_eq!(last.payload, frame.to_vec());

        // A control command hands back the device's own reply packet.
        let result = gdb_command(raw, v3::STATUS, ptr::null(), 0, &mut reply);
        assert_eq!(result, 0);
        assert_eq!(reply.opcode, v3::STATUS | 0x80);
        assert_eq!(reply.len, 51, "STATUS is 51 payload bytes");
        assert_eq!(unsafe { *reply.payload }, 0, "STATUS result byte");
        gdb_packet_free(&mut reply);
        assert!(reply.payload.is_null() && reply.len == 0);

        // An opcode the protocol does not define is the caller's mistake.
        let result = gdb_command(raw, 0x7f, ptr::null(), 0, &mut reply);
        assert_eq!(result, i32::from(ErrorCode::Invalid.as_u8()));
        gdb_device_free(raw);
    }

    #[test]
    fn shared_opens_the_same_board_and_reports_the_stolen_frames() {
        let raw = gdb_open_mock_shared(1000, false, false);
        assert!(!raw.is_null(), "the shared mock device did not open");
        assert_eq!(gdb_device_state(raw), 2, "connected");
        assert_eq!(gdb_stolen_frames(raw), 0);
        let status = json_of(gdb_status(raw));
        assert_eq!(
            status["pending"], 0,
            "STATUS still answers on a shared handle"
        );

        // A NULL handle is the caller's contract: no count, an error to read.
        assert_eq!(gdb_stolen_frames(ptr::null()), 0);
        assert_eq!(
            gdb_last_error(ptr::null_mut(), 0),
            i32::from(ErrorCode::Invalid.as_u8())
        );
        gdb_device_free(raw);
    }

    /// A caller-supplied desktop: the monitors, the negative second screen, the
    /// window, and the same clamping the real one does, all over the C ABI.
    #[test]
    fn a_mock_layout_places_the_screens_and_clamps_like_the_real_one() {
        const LAYOUT: &str = r#"{
            "cursor": [100, 100],
            "monitors": [
                {"rect": [0, 0, 1920, 1080], "dpi": 96, "primary": true},
                {"rect": [-1280, 0, 0, 720], "dpi": 144}
            ],
            "windows": [
                {"hwnd": 4660, "rect": [-1278, -22, -30, 710], "client": [-1270, 9, -38, 679]}
            ]
        }"#;
        let json = c_text(LAYOUT);
        let raw = gdb_open_mock_layout(json.as_ptr(), 1000, false, false);
        assert!(!raw.is_null(), "{}", message());

        // The layout the caller described is the layout the handle reports.
        let monitors = json_of(gdb_device_monitors_json(raw));
        assert_eq!(monitors[0]["primary"], true);
        assert_eq!(monitors[1]["left"], -1280);
        assert_eq!(monitors[1]["dpi"], 144);

        // A containing anchor follows the point onto the negative screen.
        let containing = GdbScreen {
            kind: 3,
            arg: (((-500i32) as u32 as u64) << 32 | (300i32 as u32 as u64)) as i64,
        };
        let mut report = GdbMoveReport {
            requested_x: 0,
            requested_y: 0,
            applied_x: 0,
            applied_y: 0,
            clamped: false,
            elapsed_us: 0,
        };
        assert_eq!(
            gdb_mouse_move_to_on_report(raw, -500, 300, containing, 60, ptr::null(), &mut report),
            0,
            "{}",
            message()
        );
        assert!(!report.clamped, "the left screen holds the point");
        assert!(
            (report.applied_x + 500).abs() <= 1 && (report.applied_y - 300).abs() <= 1,
            "landed at ({}, {})",
            report.applied_x,
            report.applied_y
        );

        // Anchored on the primary monitor the same point is outside the rectangle,
        // so it is clamped to the edge and the report says so.
        let primary = GdbScreen { kind: 0, arg: 0 };
        assert_eq!(
            gdb_mouse_move_to_on_report(raw, -500, 300, primary, 60, ptr::null(), &mut report),
            0,
            "{}",
            message()
        );
        assert!(
            report.clamped,
            "the point is on the primary monitor's left edge"
        );
        assert!(
            report.applied_x.abs() <= 1 && (report.applied_y - 300).abs() <= 1,
            "clamped to (0, 300), not ({}, {})",
            report.applied_x,
            report.applied_y
        );

        // The client area of the declared window, not its frame.
        let client = GdbScreen { kind: 4, arg: 4660 };
        assert_eq!(
            gdb_mouse_move_to_on_report(raw, -1275, 0, client, 60, ptr::null(), &mut report),
            0,
            "{}",
            message()
        );
        assert!(report.clamped);
        assert!(
            (report.applied_x + 1270).abs() <= 1 && (report.applied_y - 9).abs() <= 1,
            "the client area's origin, not the frame's: ({}, {})",
            report.applied_x,
            report.applied_y
        );

        // A window the layout does not declare is stale, not a silent zero.
        let missing = GdbScreen {
            kind: 4,
            arg: 0x9999,
        };
        assert_eq!(
            gdb_mouse_move_to_on(raw, -600, 300, missing, 60, ptr::null()),
            i32::from(ErrorCode::StaleWindow.as_u8())
        );
        assert!(
            message().contains("declares 1 other window"),
            "{}",
            message()
        );
        gdb_device_free(raw);

        // A layout no desktop could have is refused before a device exists.
        let broken = c_text(r#"{"monitors":[{"rect":[0,0,0,720]}]}"#);
        assert!(gdb_open_mock_layout(broken.as_ptr(), 1000, false, false).is_null());
        assert_eq!(last_code(), i32::from(ErrorCode::Invalid.as_u8()));
        assert!(message().contains("empty rectangle"), "{}", message());
        assert!(gdb_open_mock_layout(ptr::null(), 1000, false, false).is_null());
        assert_eq!(last_code(), i32::from(ErrorCode::Invalid.as_u8()));

        // The shared flavour opens the same layout through the same parser.
        let shared = gdb_open_mock_layout_shared(json.as_ptr(), 1000, false, false);
        assert!(!shared.is_null(), "{}", message());
        assert_eq!(gdb_device_state(shared), 2);
        assert_eq!(json_of(gdb_device_monitors_json(shared))[1]["left"], -1280);
        gdb_device_free(shared);
    }

    #[test]
    fn a_timed_click_and_a_click_at_reach_the_mock() {
        let raw = open_mock();
        let started = Instant::now();
        assert_eq!(
            gdb_mouse_click_timed(raw, 1, 2, 30, 0, 0),
            0,
            "{}",
            message()
        );
        assert!(
            started.elapsed() >= Duration::from_millis(30),
            "a 30 ms hold returned early"
        );
        let states: Vec<u8> = writes(raw)
            .iter()
            .filter(|packet| packet.opcode == v3::MOUSE_BUTTONS)
            .map(|packet| packet.payload[0])
            .collect();
        assert_eq!(states, vec![1, 0, 1, 0], "two press/release pairs");
        assert_eq!(unsafe { &*raw }._mock.as_ref().unwrap().held().buttons, 0);

        // Aim-and-click: the move goes out before either click packet.
        let before = writes(raw).len();
        let screen = GdbScreen { kind: 0, arg: 0 };
        assert_eq!(
            gdb_mouse_click_at_on(raw, 300, 200, screen, 2, 1, 1, 0, 0),
            0,
            "{}",
            message()
        );
        let sent = writes(raw);
        let tail: Vec<u8> = sent[before..].iter().map(|packet| packet.opcode).collect();
        assert!(
            tail.iter().position(|opcode| *opcode == v3::MOUSE_MOVE)
                < tail.iter().position(|opcode| *opcode == v3::MOUSE_BUTTONS),
            "the click must come after the move: {tail:?}"
        );
        let cursor = sent[before..]
            .iter()
            .filter(|packet| packet.opcode == v3::MOUSE_MOVE)
            .fold((0i32, 0i32), |sum, packet| {
                (
                    sum.0 + i32::from(i16::from_le_bytes([packet.payload[0], packet.payload[1]])),
                    sum.1 + i32::from(i16::from_le_bytes([packet.payload[2], packet.payload[3]])),
                )
            });
        let landed = (960 + cursor.0, 540 + cursor.1);
        assert!(
            (landed.0 - 300).abs() <= 1 && (landed.1 - 200).abs() <= 1,
            "the click landed at {landed:?}"
        );

        // Caller mistakes are typed, and a bad screen kind is refused before
        // either half runs.
        assert_eq!(
            gdb_mouse_click_timed(raw, 1, 0, 10, 10, 0),
            i32::from(ErrorCode::Invalid.as_u8())
        );
        assert_eq!(
            gdb_mouse_click_at_on(raw, 10, 10, GdbScreen { kind: 9, arg: 0 }, 1, 1, 1, 0, 0),
            i32::from(ErrorCode::Invalid.as_u8())
        );
        assert_eq!(
            gdb_mouse_click_timed(raw, 9, 1, 10, 10, 0),
            i32::from(ErrorCode::Invalid.as_u8())
        );
        gdb_device_free(raw);
    }

    #[test]
    fn move_raw_splits_at_the_report_limit_without_a_barrier() {
        let raw = open_mock();
        assert_eq!(gdb_move_raw(raw, 2400, -1200), 0);

        let sent = writes(raw);
        let moves: Vec<Vec<u8>> = sent
            .iter()
            .filter(|packet| packet.opcode == v3::MOUSE_MOVE)
            .map(|packet| packet.payload.clone())
            .collect();
        assert_eq!(
            moves,
            vec![
                vec![0x00, 0x04, 0x00, 0xfc], // (1024, -1024)
                vec![0x00, 0x04, 0x50, 0xff], // (1024, -176)
                vec![0x60, 0x01, 0x00, 0x00], // (352, 0)
            ]
        );
        assert!(
            sent.iter().all(|packet| packet.opcode != v3::BARRIER),
            "move_raw must not flush the queue"
        );
        gdb_device_free(raw);
    }

    static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());

    /// The log sink is process-wide, so the two tests that touch it may not run
    /// at the same time.
    static LOG_LOCK: Mutex<()> = Mutex::new(());

    extern "C" fn capture(_user: *mut c_void, level: i32, message: *const c_char) {
        let text = unsafe { CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned();
        LINES
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(format!("{level}:{text}"));
    }

    #[test]
    fn log_sink_receives_a_line() {
        let _serialised = LOG_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        gdb_set_log_level(Level::Trace as u8);
        gdb_set_log_sink(Some(capture), ptr::null_mut());

        gdb_device_free(open_mock());
        gdb_set_log_sink(None, ptr::null_mut());
        gdb_set_log_level(Level::Off as u8);

        let lines = LINES.lock().unwrap_or_else(|error| error.into_inner());
        assert!(!lines.is_empty(), "the sink received nothing");
        assert!(
            lines.iter().any(|line| line.contains("opcode=")),
            "no packet line reached the sink: {lines:?}"
        );
    }

    // ---------------------------------------------------------- presence

    /// A probe is the same object `gdb_device_info` returns, because both are
    /// `info_json(&DeviceInfo)`. The mock supplies the identity, so the field
    /// set is proven without hardware.
    #[test]
    fn probe_json_is_the_device_info_shape() {
        let (transport, _handle) = gdb_sdk::mock::MockTransport::new();
        let probe = probe::read_from(Box::new(transport), "MOCK", "GDBOX mock")
            .expect("the mock answers Feature 14");
        let from_probe: serde_json::Value =
            serde_json::from_str(&info_json(&probe.info()).unwrap()).unwrap();

        let direct = open_mock();
        let from_device = json_of(gdb_device_info(direct));
        assert_eq!(fields(&from_probe), fields(&from_device));
        assert_eq!(
            from_probe, from_device,
            "a probe must serialize exactly like the device info"
        );
        gdb_device_free(direct);
    }

    #[test]
    fn probe_reports_a_missing_selector_as_a_typed_error() {
        let selector = c_text("ffi-no-such-board");
        assert!(gdb_probe(selector.as_ptr()).is_null());
        assert!(
            matches!(last_code(), 3 | 8),
            "want Disconnected or Unsupported, got {}",
            last_code()
        );
        assert!(!message().is_empty(), "a failure needs a message");
    }

    #[test]
    fn probe_all_returns_a_json_array() {
        let value = json_of(gdb_probe_all());
        let entries = value.as_array().expect("an array, even when empty");
        if let Some(first) = entries.first() {
            let direct = open_mock();
            assert_eq!(fields(first), fields(&json_of(gdb_device_info(direct))));
            gdb_device_free(direct);
        }
    }

    #[test]
    fn observer_symbols_reject_a_null_handle() {
        assert!(gdb_observer_info(ptr::null()).is_null());
        assert_eq!(last_code(), i32::from(ErrorCode::Invalid.as_u8()));
        assert_eq!(gdb_observer_state(ptr::null()), -1);
        assert_eq!(
            gdb_observer_status(ptr::null()),
            i32::from(ErrorCode::Invalid.as_u8())
        );
        gdb_observer_stop(ptr::null_mut());
        gdb_observer_free(ptr::null_mut());
    }

    #[test]
    fn observer_start_without_a_board_is_a_typed_error() {
        let selector = c_text("ffi-no-such-board");
        assert!(gdb_observer_start(selector.as_ptr(), 100).is_null());
        assert!(
            matches!(last_code(), 3 | 8),
            "want Disconnected or Unsupported, got {}",
            last_code()
        );
    }

    /// The live half: present only when a board is plugged in. It reads Feature
    /// 14 (a control transfer), never a packet — the refusals prove it.
    #[test]
    fn an_observer_on_a_present_board_refuses_every_command() {
        let present = gdb_probe(ptr::null());
        if present.is_null() {
            return;
        }
        gdb_string_free(present);

        let observer = gdb_observer_start(ptr::null(), 100);
        assert!(!observer.is_null(), "{}", message());

        let info = json_of(gdb_observer_info(observer));
        let direct = open_mock();
        assert_eq!(fields(&info), fields(&json_of(gdb_device_info(direct))));
        gdb_device_free(direct);
        assert_eq!(info["board"], 2);
        assert_eq!(
            gdb_observer_state(observer),
            2,
            "connected while it answers"
        );

        let unsupported = i32::from(ErrorCode::Unsupported.as_u8());
        assert_eq!(gdb_observer_status(observer), unsupported);
        assert!(message().contains("Feature 14"), "{}", message());
        assert_eq!(gdb_observer_barrier(observer), unsupported);
        assert_eq!(gdb_observer_release_all(observer), unsupported);

        let mut packet = GdbPacket::empty();
        assert_eq!(
            gdb_observer_command(observer, v3::STATUS, ptr::null(), 0, &mut packet),
            unsupported
        );
        assert!(packet.payload.is_null() && packet.len == 0);

        gdb_observer_stop(observer);
        gdb_observer_free(observer);
    }

    // ------------------------------------------------------------- remote

    /// A name no other test and no other process uses.
    fn unique_pipe(stem: &str) -> String {
        static NEXT: AtomicU32 = AtomicU32::new(1);
        format!(
            "ffi-{}-{}-{stem}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )
    }

    fn c_text(text: &str) -> CString {
        CString::new(text).expect("the test name has no NUL")
    }

    fn message() -> String {
        let mut buffer = [0 as c_char; 512];
        gdb_last_error(buffer.as_mut_ptr(), buffer.len());
        unsafe { CStr::from_ptr(buffer.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    }

    fn last_code() -> i32 {
        gdb_last_error(ptr::null_mut(), 0)
    }

    fn json_of(raw: *mut c_char) -> serde_json::Value {
        assert!(!raw.is_null(), "no JSON came back: {}", message());
        let text = unsafe { CStr::from_ptr(raw) }
            .to_string_lossy()
            .into_owned();
        gdb_string_free(raw);
        serde_json::from_str(&text).expect("the reply is not JSON")
    }

    /// Top-level field names, so a test can prove two replies parse alike.
    fn fields(value: &serde_json::Value) -> Vec<String> {
        let mut names: Vec<String> = value
            .as_object()
            .expect("an object")
            .keys()
            .cloned()
            .collect();
        names.sort();
        names
    }

    /// Attaches to a real broker on a real pipe with a mock device behind it.
    ///
    /// One test on purpose: the pipe, the two clients, the typed errors and the
    /// dead broker are one story, and a second broker in the same process would
    /// only add another name to clean up.
    #[test]
    fn attached_clients_share_one_device_and_a_dead_broker_is_reported() {
        let _serialised = LOG_LOCK.lock().unwrap_or_else(|error| error.into_inner());

        let name = unique_pipe("attach");
        let (device, mock) = Manager::open_mock_with_handle(DeviceOptions::default()).unwrap();
        let server = broker::Server::new(&name, broker::Source::Device(device)).unwrap();
        let pipe = c_text(&name);

        // Attached calls log through the same process-wide sink as direct ones.
        gdb_set_log_level(Level::Info as u8);
        gdb_set_log_sink(Some(capture), ptr::null_mut());

        // A broker is already serving, so spawning is a no-op success.
        assert_eq!(gdb_remote_spawn(pipe.as_ptr()), 0, "{}", message());

        let mut first: *mut GdbRemote = ptr::null_mut();
        assert_eq!(gdb_attach(pipe.as_ptr(), &mut first), 0, "{}", message());
        assert!(!first.is_null(), "attach returned GDB_OK without a handle");

        // A second client attaches while the first still holds the pipe.
        let mut second: *mut GdbRemote = ptr::null_mut();
        assert_eq!(
            gdb_attach(pipe.as_ptr(), &mut second),
            0,
            "a second attach must work while the first is attached: {}",
            message()
        );
        assert!(!second.is_null());

        // The JSON is the direct call's JSON, field for field.
        let direct = open_mock();
        let info = json_of(gdb_remote_info(first));
        assert_eq!(info["serial"], "MOCK");
        assert_eq!(info["firmware"], "3.2.0");
        assert_eq!(info["board"], 2);
        assert_eq!(fields(&info), fields(&json_of(gdb_device_info(direct))));

        let status = json_of(gdb_remote_status(second));
        assert_eq!(status["leaseMs"], 2000);
        assert_eq!(status["pending"], 0);
        assert_eq!(
            fields(&status),
            fields(&json_of(gdb_status(direct))),
            "attached and direct STATUS must parse alike"
        );

        let ext = json_of(gdb_remote_status_ext(first));
        assert_eq!(ext["sessions"], 1);
        assert_eq!(fields(&ext), fields(&json_of(gdb_status_ext(direct))));

        let sessions = json_of(gdb_remote_sessions(second));
        assert_eq!(sessions.as_array().map(Vec::len), Some(1));
        assert_eq!(
            fields(&sessions[0]),
            fields(&json_of(gdb_sessions(direct))[0])
        );
        gdb_device_free(direct);

        // Both handles drive the same device session.
        assert_eq!(gdb_remote_key(second, 7, 0x04, true), 0, "{}", message());
        assert_eq!(gdb_remote_mouse_move(first, 10, -4), 0, "{}", message());
        assert_eq!(gdb_remote_move_raw(second, 800, -300), 0, "{}", message());
        assert_eq!(
            gdb_remote_move_by(first, 60, -30, 10, ptr::null()),
            0,
            "{}",
            message()
        );
        assert_eq!(gdb_remote_mouse_buttons(second, 1), 0, "{}", message());
        assert_eq!(gdb_remote_wheel(first, -120, 0), 0, "{}", message());
        assert_eq!(gdb_remote_barrier(first), 0, "{}", message());

        let writes = mock.writes();
        assert!(
            writes.iter().any(|packet| packet.opcode == v3::KEY
                && packet.payload.starts_with(&[0x07, 0x00, 0x04, 0x00])),
            "the broker never saw the key from the second client"
        );
        assert!(
            writes.iter().any(|packet| packet.opcode == v3::MOUSE_MOVE
                && packet.payload == [0x0a, 0x00, 0xfc, 0xff]),
            "the broker never saw the move from the first client"
        );

        assert_eq!(gdb_remote_release_all(second), 0, "{}", message());
        assert_eq!(mock.held().buttons, 0, "release_all must reach the device");

        // Errors arrive as the same codes the direct calls use.
        let mut reply = GdbPacket::empty();
        assert_eq!(
            gdb_remote_command(first, 0x7f, ptr::null(), 0, &mut reply),
            i32::from(ErrorCode::Invalid.as_u8())
        );
        assert!(message().contains("opcode 127"), "{}", message());
        assert_eq!(
            gdb_remote_command(first, v3::MOUSE_MOVE, [0x01].as_ptr(), 1, &mut reply),
            i32::from(ErrorCode::Invalid.as_u8()),
            "a truncated payload is the caller's mistake"
        );
        assert_eq!(
            gdb_remote_command(first, v3::STATUS, ptr::null(), 0, &mut reply),
            0,
            "{}",
            message()
        );
        assert_eq!(reply.opcode, v3::STATUS | 0x80);
        assert_eq!(reply.len, 51);
        gdb_packet_free(&mut reply);

        // close() hangs that one client up; the other keeps working.
        assert_eq!(gdb_remote_close(second), 0, "{}", message());
        assert_eq!(
            gdb_remote_key(second, 7, 0x04, false),
            i32::from(ErrorCode::Disconnected.as_u8())
        );
        gdb_remote_free(second);
        assert_eq!(gdb_remote_barrier(first), 0, "{}", message());

        // A dead broker is GDB_DISCONNECTED, not a timeout or a crash.
        server.shutdown();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !unsafe { &*first }.client.is_closed() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            unsafe { &*first }.client.is_closed(),
            "the client never noticed the broker leaving"
        );
        assert!(
            gdb_remote_status(first).is_null(),
            "a dead broker must not answer"
        );
        assert_eq!(last_code(), i32::from(ErrorCode::Disconnected.as_u8()));
        assert!(message().contains("broker"), "{}", message());
        assert_eq!(
            gdb_remote_move_raw(first, 10, 10),
            i32::from(ErrorCode::Disconnected.as_u8())
        );
        gdb_remote_free(first);

        let lines = LINES.lock().unwrap_or_else(|error| error.into_inner());
        assert!(
            lines
                .iter()
                .any(|line| line.contains("broker: client attached")),
            "an attached call must reach the process-wide sink: {lines:?}"
        );
        gdb_set_log_sink(None, ptr::null_mut());
        gdb_set_log_level(Level::Off as u8);
    }

    #[test]
    fn attach_reports_a_missing_broker_and_a_bad_name() {
        let missing = c_text(&unique_pipe("nobody-listening"));

        // The out pointer is cleared before the call can fail, so a caller that
        // ignores the code never frees a stale handle.
        let mut handle: *mut GdbRemote = ptr::null_mut();
        assert_eq!(
            gdb_attach(missing.as_ptr(), &mut handle),
            i32::from(ErrorCode::Disconnected.as_u8())
        );
        assert!(handle.is_null());
        assert!(!message().is_empty(), "a failure needs a message");

        // A path separator would escape the pipe namespace.
        let bad = c_text(r"nested/name");
        let mut handle = ptr::null_mut();
        assert_eq!(
            gdb_attach(bad.as_ptr(), &mut handle),
            i32::from(ErrorCode::Invalid.as_u8()),
            "{}",
            message()
        );

        // Caller mistakes, not crashes.
        assert_eq!(
            gdb_attach(missing.as_ptr(), ptr::null_mut()),
            i32::from(ErrorCode::Invalid.as_u8())
        );
        assert_eq!(
            gdb_remote_barrier(ptr::null_mut()),
            i32::from(ErrorCode::Invalid.as_u8())
        );
        gdb_remote_free(ptr::null_mut());
    }

    /// The runtime revision, the capability list and the pursuit move: the three
    /// things a host reads before it trusts a symbol it has not called yet.
    /// The move report: a measurement of where the pointer ended, and the clamp
    /// flag that says the anchor pulled the destination back.
    #[test]
    fn the_move_report_is_a_measurement_not_an_echo() {
        let raw = open_mock();
        let mut report = GdbMoveReport {
            requested_x: 0,
            requested_y: 0,
            applied_x: 0,
            applied_y: 0,
            clamped: true,
            elapsed_us: 0,
        };
        assert_eq!(
            gdb_mouse_move_to_on_report(
                raw,
                300,
                200,
                GdbScreen { kind: 0, arg: 0 },
                20,
                ptr::null(),
                &mut report
            ),
            0,
            "{}",
            message()
        );
        assert_eq!((report.requested_x, report.requested_y), (300, 200));
        assert!(
            (report.applied_x - 300).abs() <= 1 && (report.applied_y - 200).abs() <= 1,
            "the applied point is where the pointer was measured: {report:?}",
        );
        assert!(!report.clamped, "a point on the anchor is not clamped");
        assert!(report.elapsed_us > 0, "a move takes time");

        // A caller mistake is typed and the caller's struct is not touched.
        let mut slot = GdbMoveReport {
            requested_x: 7,
            requested_y: 7,
            applied_x: 7,
            applied_y: 7,
            clamped: true,
            elapsed_us: 7,
        };
        assert_eq!(
            gdb_mouse_move_to_on_report(
                raw,
                10,
                10,
                GdbScreen { kind: 0, arg: 0 },
                0,
                ptr::null(),
                ptr::null_mut()
            ),
            i32::from(ErrorCode::Invalid.as_u8()),
            "out is required"
        );
        assert_eq!(
            gdb_mouse_move_to_on_report(
                raw,
                5000,
                5000,
                GdbScreen { kind: 1, arg: 1 },
                0,
                ptr::null(),
                &mut slot
            ),
            i32::from(ErrorCode::InvalidScreen.as_u8())
        );
        assert_eq!(
            (slot.applied_x, slot.elapsed_us, slot.clamped),
            (7, 7, true),
            "a failed move must leave the caller's struct alone"
        );
        gdb_device_free(raw);
    }

    static JSON_EVENTS: Mutex<Vec<(i32, String)>> = Mutex::new(Vec::new());

    extern "C" fn capture_json(_user: *mut c_void, kind: i32, json: *const c_char) {
        let text = unsafe { CStr::from_ptr(json) }
            .to_string_lossy()
            .into_owned();
        JSON_EVENTS
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push((kind, text));
    }

    /// A state change and an error, each as one JSON object with the fields the
    /// plain callback only implies, and the last STATUS the device answered.
    #[test]
    fn json_events_carry_the_state_the_error_and_the_last_status() {
        let _serialised = LOG_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        JSON_EVENTS
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();

        let raw = gdb_open_mock(100, false, false);
        assert!(!raw.is_null());
        // A STATUS so the payload has a summary to carry.
        let status = gdb_status(raw);
        assert!(!status.is_null(), "{}", message());
        gdb_string_free(status);

        assert_eq!(
            gdb_set_event_handler_json(raw, Some(capture_json), ptr::null_mut()),
            0,
            "{}",
            message()
        );

        // An error event: silence the device and let the command time out.
        unsafe { &*raw }
            ._mock
            .as_ref()
            .expect("mock handle")
            .set_silent(true);
        assert!(gdb_status(raw).is_null());
        // A state event: closing the device reports Disconnected.
        gdb_close(raw);
        gdb_device_free(raw);
        assert_eq!(
            gdb_set_event_handler_json(ptr::null_mut(), None, ptr::null_mut()),
            i32::from(ErrorCode::Invalid.as_u8()),
            "a NULL handle is the caller's mistake"
        );

        let events = JSON_EVENTS
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let error = events
            .iter()
            .find(|(kind, _)| *kind == 2)
            .map(|(_, json)| serde_json::from_str::<serde_json::Value>(json).expect("error JSON"))
            .expect("an error event");
        assert_eq!(error["code"], i32::from(ErrorCode::Timeout.as_u8()));
        assert!(
            error["message"]
                .as_str()
                .expect("a message")
                .contains("opcode 6"),
            "{error}"
        );
        assert!(
            error["status"]["received"].as_u64().is_some(),
            "the last STATUS the device answered rides along: {error}"
        );

        let state = events
            .iter()
            .find(|(kind, _)| *kind == 0)
            .map(|(_, json)| serde_json::from_str::<serde_json::Value>(json).expect("state JSON"))
            .expect("a state event");
        assert_eq!(state["state"], 0, "closed is Disconnected: {state}");
        assert!(state.get("status").is_some());
    }

    #[test]
    fn the_runtime_revision_and_the_capability_list_describe_this_build() {
        let mut major = 0u32;
        let mut minor = 0u32;
        assert_eq!(gdb_abi_version_full(&mut major, &mut minor), 0);
        assert_eq!(major, 1);
        assert_eq!(
            minor,
            header_minor(),
            "the runtime minor and gdb.h disagree"
        );
        assert_eq!(
            gdb_abi_version_full(ptr::null_mut(), &mut minor),
            i32::from(ErrorCode::Invalid.as_u8())
        );

        let capabilities = json_of(gdb_capabilities());
        for name in [
            "click_timed",
            "click_at_on",
            "window_metrics",
            "window_client_screen",
            "pursuit",
            "move_report",
            "event_json",
            "scheduler",
            "broker",
            "replay",
            "flash",
        ] {
            assert!(
                capabilities
                    .get(name)
                    .and_then(|value| value.as_bool())
                    .is_some(),
                "the capability list has no boolean {name}: {capabilities}"
            );
        }
        assert_eq!(capabilities["click_timed"], serde_json::Value::Bool(true));
        assert_eq!(capabilities["pursuit"], serde_json::Value::Bool(true));
        assert_eq!(
            capabilities["window_metrics"],
            serde_json::Value::Bool(cfg!(windows))
        );
        assert_eq!(
            capabilities["replay"],
            serde_json::Value::Bool(false),
            "replay has no C symbol, so a binding cannot reach it"
        );

        // Pursuit: the same clamping, no trajectory and no barrier per step.
        let raw = open_mock();
        let before = writes(raw).len();
        assert_eq!(
            gdb_mouse_move_to_on_pursuit(raw, 1400, 300, GdbScreen { kind: 0, arg: 0 }),
            0,
            "{}",
            message()
        );
        let sent = writes(raw);
        let tail: Vec<u8> = sent[before..].iter().map(|packet| packet.opcode).collect();
        assert!(
            !tail.contains(&v3::BARRIER),
            "pursuit must not wait for a barrier per step: {tail:?}"
        );
        assert!(
            tail.iter().all(|opcode| *opcode == v3::MOUSE_MOVE),
            "pursuit sends moves and nothing else: {tail:?}"
        );
        assert_eq!(
            gdb_mouse_move_to_on_pursuit(raw, 10, 10, GdbScreen { kind: 9, arg: 0 }),
            i32::from(ErrorCode::Invalid.as_u8()),
            "an unknown anchor is refused before anything is sent"
        );
        gdb_device_free(raw);
    }

    #[test]
    fn the_last_error_is_per_thread_and_the_device_keeps_its_own() {
        let raw = open_mock();
        let shared = raw as usize;

        let invalid = thread::spawn(move || {
            let raw = shared as *mut GdbDevice;
            assert_eq!(
                gdb_command(raw, 0x7f, ptr::null(), 0, ptr::null_mut()),
                i32::from(ErrorCode::Invalid.as_u8())
            );
            (last_code(), message())
        });
        let screen = thread::spawn(move || {
            let raw = shared as *mut GdbDevice;
            assert_eq!(
                gdb_mouse_move_to_on(raw, 10, 10, GdbScreen { kind: 1, arg: 1 }, 0, ptr::null()),
                i32::from(ErrorCode::InvalidScreen.as_u8())
            );
            (last_code(), message())
        });

        let (invalid_code, invalid_text) = invalid.join().expect("thread");
        let (screen_code, screen_text) = screen.join().expect("thread");
        assert_eq!(invalid_code, i32::from(ErrorCode::Invalid.as_u8()));
        assert!(
            invalid_text.contains("opcode 127") && !invalid_text.contains("monitor index"),
            "the Invalid thread read somebody else's message: {invalid_text}"
        );
        assert_eq!(screen_code, i32::from(ErrorCode::InvalidScreen.as_u8()));
        assert!(
            screen_text.contains("monitor index 1") && !screen_text.contains("opcode 127"),
            "the InvalidScreen thread read somebody else's message: {screen_text}"
        );

        // This thread never failed, so its slot is empty whatever the others did.
        assert_eq!(last_code(), 0, "a thread that did not fail has no error");

        // The device, however, remembers the last failure from any thread.
        let mut buffer = [0 as c_char; 512];
        let code = gdb_device_last_error(raw, buffer.as_mut_ptr(), buffer.len());
        assert!(
            code == i32::from(ErrorCode::Invalid.as_u8())
                || code == i32::from(ErrorCode::InvalidScreen.as_u8()),
            "the device recorded no failure: {code}"
        );
        let text = unsafe { CStr::from_ptr(buffer.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        assert!(
            text.contains("session="),
            "a device error must name the session: {text}"
        );
        gdb_device_free(raw);
    }

    /// A failure below the session (an anchor that does not resolve) and one at
    /// the wire (a command with no reply) both land in the device's record, the
    /// second with the opcode and the last STATUS the device answered.
    #[test]
    fn a_device_names_the_opcode_and_the_last_status_it_answered() {
        let raw = gdb_open_mock(100, false, false);
        assert!(!raw.is_null());

        // A status the device answered is what a later failure reports.
        let answer = gdb_status(raw);
        assert!(!answer.is_null(), "STATUS must answer: {}", message());
        gdb_string_free(answer);
        assert_eq!(last_code(), 0);

        // Silence it: STATUS now waits out the 100 ms timeout.
        unsafe { &*raw }
            ._mock
            .as_ref()
            .expect("mock handle")
            .set_silent(true);
        assert!(gdb_status(raw).is_null());
        assert_eq!(last_code(), i32::from(ErrorCode::Timeout.as_u8()));

        let mut buffer = [0 as c_char; 512];
        assert_eq!(
            gdb_device_last_error(raw, buffer.as_mut_ptr(), buffer.len()),
            i32::from(ErrorCode::Timeout.as_u8())
        );
        let text = unsafe { CStr::from_ptr(buffer.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        assert!(text.contains("opcode=STATUS"), "{text}");
        assert!(text.contains("session="), "{text}");
        assert!(
            text.contains("status=received:"),
            "the last STATUS the device answered belongs in the record: {text}"
        );

        // A caller mistake is typed and sets the thread-local slot too.
        assert_eq!(
            gdb_device_last_error(ptr::null(), ptr::null_mut(), 0),
            i32::from(ErrorCode::Invalid.as_u8())
        );
        gdb_device_free(raw);
    }

    #[test]
    fn the_window_client_anchor_and_the_platform_reads_are_shaped_for_a_host() {
        // kind 4 carries the handle the way kind 2 does, and it is a different
        // anchor: the client area, not the frame.
        assert_eq!(
            screen_from(4, 0x1234).expect("window client"),
            Screen::WindowClient(0x1234)
        );
        assert_eq!(
            screen_from(2, 0x1234).expect("window"),
            Screen::Window(0x1234),
            "kind 2 must keep meaning the whole frame"
        );

        // A mock has no windows, so the anchor is a stale window, not a panic.
        let raw = open_mock();
        assert_eq!(
            gdb_mouse_move_to_on(
                raw,
                10,
                10,
                GdbScreen {
                    kind: 4,
                    arg: 0x1234
                },
                0,
                ptr::null()
            ),
            i32::from(ErrorCode::StaleWindow.as_u8())
        );
        assert!(message().contains("client area"), "{}", message());
        gdb_device_free(raw);

        // The DPI mode: one object, every field named, and the platform says
        // whether it has the concept at all.
        let prepared = json_of(gdb_platform_prepare());
        for field in ["supported", "dpiAwareness", "value", "perMonitor"] {
            assert!(prepared.get(field).is_some(), "prepare without {field}");
        }
        assert_eq!(prepared["supported"].as_bool(), Some(cfg!(windows)));
        assert!(prepared["dpiAwareness"].is_string());
        assert!(prepared["perMonitor"].is_boolean());

        // A handle the system does not know: a typed refusal, never a zeroed
        // answer that a caller would read as "the window is at (0, 0)".
        assert!(gdb_window_metrics(0).is_null());
        assert_eq!(
            last_code(),
            i32::from(if cfg!(windows) {
                ErrorCode::StaleWindow.as_u8()
            } else {
                ErrorCode::Unsupported.as_u8()
            })
        );
    }

    #[test]
    fn the_monitor_listing_and_the_containing_anchor_keep_their_shape() {
        // kind 3 packs both coordinates: x in the high half, y in the low half.
        let packed = (1400u64 << 32) | u64::from(-300i32 as u32);
        assert_eq!(
            screen_from(3, packed as i64).expect("containing"),
            Screen::Containing(1400, -300)
        );
        assert_eq!(screen_from(9, 0).unwrap_err().code(), ErrorCode::Invalid);

        #[cfg(windows)]
        {
            let listing = json_of(gdb_monitors_json());
            let monitors = listing.as_array().expect("an array");
            assert!(!monitors.is_empty(), "the system reports no monitors");
            for monitor in monitors {
                for field in ["left", "top", "right", "bottom", "primary", "dpi"] {
                    assert!(monitor.get(field).is_some(), "a monitor without {field}");
                }
            }
            assert!(
                monitors.iter().filter(|m| m["primary"] == true).count() == 1,
                "exactly one monitor is the primary one"
            );
        }

        // The clamp readout starts empty, and a caller mistake is typed.
        let raw = open_mock();
        let mut clamps = GdbClamps {
            clamped_total: 1,
            requested_x: 7,
            requested_y: 7,
            applied_x: 7,
            applied_y: 7,
        };
        assert_eq!(gdb_metrics_clamps(raw, &mut clamps), 0, "{}", message());
        assert_eq!(clamps.clamped_total, 0);
        assert_eq!((clamps.applied_x, clamps.applied_y), (0, 0));
        assert_eq!(
            gdb_metrics_clamps(ptr::null(), &mut clamps),
            i32::from(ErrorCode::Invalid.as_u8())
        );
        gdb_device_free(raw);
    }
}
