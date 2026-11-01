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

/// Finds an executable the way a shell would: `PATH` first, then the places an
/// installer writes without refreshing an inherited environment (the registry
/// `Path`, the winget links folder).
pub fn locate_program(name: &str) -> Option<std::path::PathBuf> {
    let paths: Vec<std::path::PathBuf> =
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
    #[cfg(windows)]
    let paths = {
        let mut paths = paths;
        paths.extend(registry_paths());
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            paths.push(std::path::PathBuf::from(local).join("Microsoft/WinGet/Links"));
        }
        paths
    };
    paths
        .into_iter()
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
}

#[cfg(windows)]
fn registry_paths() -> Vec<std::path::PathBuf> {
    use windows_sys::Win32::System::Registry::*;
    let mut result = Vec::new();
    for (root, sub) in [
        (HKEY_CURRENT_USER, "Environment"),
        (
            HKEY_LOCAL_MACHINE,
            "SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment",
        ),
    ] {
        let key: Vec<u16> = sub.encode_utf16().chain(Some(0)).collect();
        let value: Vec<u16> = "Path".encode_utf16().chain(Some(0)).collect();
        let mut buffer = vec![0u16; 32768];
        let mut size = (buffer.len() * 2) as u32;
        // An install that landed after this process started is only visible here.
        unsafe {
            if RegGetValueW(
                root,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
                std::ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut size,
            ) == 0
            {
                let end = buffer.iter().position(|n| *n == 0).unwrap_or(buffer.len());
                result.extend(std::env::split_paths(&std::ffi::OsString::from(
                    String::from_utf16_lossy(&buffer[..end]),
                )));
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    #[test]
    fn dpi_awareness_is_callable_more_than_once() {
        super::enable_dpi_awareness();
        super::enable_dpi_awareness();
    }
}
