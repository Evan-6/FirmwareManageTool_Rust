//! Platform facts the protocol does not provide: the OS cursor position and the
//! primary monitor size. Absolute movement is computed host side from these.

use crate::error::{Error, ErrorCode, Result};
use gdb_motion::Screen;

pub fn screen_and_cursor() -> Result<(Screen, (i32, i32))> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::{
            Foundation::POINT,
            Graphics::Gdi::{
                GetMonitorInfoW, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint,
            },
            UI::WindowsAndMessaging::GetPhysicalCursorPos,
        };
        unsafe {
            let mut cursor = POINT { x: 0, y: 0 };
            if GetPhysicalCursorPos(&mut cursor) == 0 {
                return Err(Error::new(
                    ErrorCode::Unsupported,
                    format!(
                        "GetPhysicalCursorPos failed: {}",
                        std::io::Error::last_os_error()
                    ),
                ));
            }
            let monitor = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
            let mut info: MONITORINFO = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(monitor, &mut info) == 0 {
                return Err(Error::new(
                    ErrorCode::Unsupported,
                    format!(
                        "GetMonitorInfoW failed: {}",
                        std::io::Error::last_os_error()
                    ),
                ));
            }
            Ok((
                Screen {
                    width: info.rcMonitor.right - info.rcMonitor.left,
                    height: info.rcMonitor.bottom - info.rcMonitor.top,
                },
                (
                    cursor.x - info.rcMonitor.left,
                    cursor.y - info.rcMonitor.top,
                ),
            ))
        }
    }
    #[cfg(not(windows))]
    {
        Err(Error::unsupported(
            "absolute mouse coordinates need Windows cursor APIs",
        ))
    }
}

/// Makes the process DPI aware so physical pixels are what the sensor sees.
pub fn enable_dpi_awareness() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::UI::HiDpi::{
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
        };
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn dpi_awareness_is_callable_more_than_once() {
        super::enable_dpi_awareness();
        super::enable_dpi_awareness();
    }
}
