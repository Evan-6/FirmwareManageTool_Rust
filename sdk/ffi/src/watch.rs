//! The arrival watch over the C ABI.
//!
//! [`gdb_sdk::watch::Watch`] owns one thread that enumerates the HID table; this
//! module hands that thread a C callback and marshals every event as one JSON
//! object. Nothing is sent on the wire, so a watch can run beside a session that
//! owns the board.
//!
//! Lifetime rules, which `gdb.h` states for the caller and every binding repeats:
//! the callback runs on the watch thread, must not call `gdb_watch_stop` or
//! `gdb_watch_free` on the same handle (the stop joins that very thread), must
//! not block (it is the only thread polling), and a panic on this side is caught
//! so the thread survives it.

use crate::{guard, json_string, set_last};
use gdb_sdk::error::Error;
use gdb_sdk::watch::{Inventory, SystemInventory, Watch, WatchEvent};
use std::ffi::{CString, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::Mutex;
use std::time::Duration;

/// A vendor HID device is now present; the JSON is its entry.
pub const WATCH_ARRIVED: i32 = 0;
/// A vendor HID device is gone; the JSON is its entry.
pub const WATCH_REMOVED: i32 = 1;
/// An RP2040 bootloader drive appeared; the JSON is `{"path":"E:\\"}`.
pub const WATCH_BOOTLOADER: i32 = 2;

type WatchFn = extern "C" fn(user: *mut c_void, kind: i32, json: *const c_char);

/// A running watch. The handle owns the thread: `gdb_watch_stop` joins it and
/// leaves the handle freeable, `gdb_watch_free` does both.
pub struct GdbWatch {
    watch: Mutex<Option<Watch>>,
}

impl GdbWatch {
    fn stop_thread(&self) {
        let mut slot = self
            .watch
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if let Some(mut watch) = slot.take() {
            watch.stop();
        }
    }
}

/// One event as the pair the C callback takes: the kind, and the JSON body.
fn event_json(event: &WatchEvent) -> (i32, String) {
    let fallback = || "{}".to_string();
    match event {
        WatchEvent::Arrived(entry) => (
            WATCH_ARRIVED,
            serde_json::to_string(entry).unwrap_or_else(|_| fallback()),
        ),
        WatchEvent::Removed(entry) => (
            WATCH_REMOVED,
            serde_json::to_string(entry).unwrap_or_else(|_| fallback()),
        ),
        WatchEvent::BootloaderArrived(path) => (
            WATCH_BOOTLOADER,
            serde_json::json!({ "path": path.to_string_lossy() }).to_string(),
        ),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn gdb_watch_start(
    interval_ms: u32,
    func: Option<WatchFn>,
    user: *mut c_void,
) -> *mut GdbWatch {
    let outcome = guard(|| {
        let func = func.ok_or_else(|| Error::invalid("watch callback is NULL"))?;
        let user = user as usize;
        let interval = Duration::from_millis(u64::from(interval_ms.max(1)));
        let watch = Watch::start(interval, move |_devices, event| {
            let (kind, json) = event_json(&event);
            // An interior NUL would truncate the object; the callback gets the
            // prefix rather than nothing at all.
            let text = CString::new(json).unwrap_or_default();
            let _ = catch_unwind(AssertUnwindSafe(|| {
                func(user as *mut c_void, kind, text.as_ptr());
            }));
        })?;
        Ok(Box::new(GdbWatch {
            watch: Mutex::new(Some(watch)),
        }))
    });
    match outcome {
        Ok(handle) => Box::into_raw(handle),
        Err(error) => {
            set_last(&error);
            ptr::null_mut()
        }
    }
}

/// Stops the thread and waits for it; safe on an already stopped or NULL handle.
#[unsafe(no_mangle)]
pub extern "C" fn gdb_watch_stop(raw: *mut GdbWatch) {
    if let Some(handle) = unsafe { raw.as_ref() } {
        handle.stop_thread();
    }
}

/// Stops the thread if it is still running, then releases the handle.
#[unsafe(no_mangle)]
pub extern "C" fn gdb_watch_free(raw: *mut GdbWatch) {
    if raw.is_null() {
        return;
    }
    let handle = unsafe { Box::from_raw(raw) };
    handle.stop_thread();
}

/// One enumeration on the calling thread: `{"devices":[...],"volumes":[...]}`.
///
/// It reads no report, so it is safe beside a session; the watch thread uses the
/// same inventory, and this is what a program reads when it wants the bus now
/// rather than at the next change.
#[unsafe(no_mangle)]
pub extern "C" fn gdb_watch_listing(raw: *const GdbWatch) -> *mut c_char {
    json_string(guard(|| {
        if unsafe { raw.as_ref() }.is_none() {
            return Err(Error::invalid("watch handle is NULL"));
        }
        let listing = SystemInventory.listing()?;
        let volumes: Vec<String> = listing
            .volumes
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        Ok(serde_json::json!({ "devices": listing.devices, "volumes": volumes }).to_string())
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gdb_sdk::error::ErrorCode;
    use gdb_sdk::manager::DeviceEntry;

    fn last_code() -> i32 {
        crate::gdb_last_error(ptr::null_mut(), 0)
    }

    /// The caller's mistakes are codes, not crashes, and neither handle needs a
    /// bus to be checked.
    #[test]
    fn a_null_callback_and_a_null_handle_are_typed_errors() {
        assert!(gdb_watch_start(100, None, ptr::null_mut()).is_null());
        assert_eq!(last_code(), i32::from(ErrorCode::Invalid.as_u8()));
        assert!(gdb_watch_listing(ptr::null()).is_null());
        assert_eq!(last_code(), i32::from(ErrorCode::Invalid.as_u8()));
        gdb_watch_stop(ptr::null_mut());
        gdb_watch_free(ptr::null_mut());
    }

    /// The callback contract, without a bus: three kinds, one JSON object each.
    #[test]
    fn each_event_is_one_json_object() {
        let entry = DeviceEntry {
            path: r"\\?\HID#VID_1209&PID_0B0B".to_string(),
            serial: "4250304B38353817".to_string(),
            product: "GDBOX KB8".to_string(),
            manufacturer: "GDBOX".to_string(),
            vid: 0x1209,
            pid: 0x0B0B,
        };
        for (event, kind) in [
            (WatchEvent::Arrived(entry.clone()), WATCH_ARRIVED),
            (WatchEvent::Removed(entry.clone()), WATCH_REMOVED),
        ] {
            let (actual, json) = event_json(&event);
            assert_eq!(actual, kind);
            let value: serde_json::Value =
                serde_json::from_str(&json).expect("a device entry is an object");
            assert_eq!(value["serial"], "4250304B38353817");
            assert_eq!(value["vid"], 0x1209);
            assert_eq!(value["pid"], 0x0B0B);
            assert_eq!(value["path"], r"\\?\HID#VID_1209&PID_0B0B");
        }

        let (kind, json) = event_json(&WatchEvent::BootloaderArrived(std::path::PathBuf::from(
            r"E:\",
        )));
        assert_eq!(kind, WATCH_BOOTLOADER);
        assert_eq!(json, r#"{"path":"E:\\"}"#);
    }
}
