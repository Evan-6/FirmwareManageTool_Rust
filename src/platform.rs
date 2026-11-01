use anyhow::{Result, bail};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Screen {
    pub width: i32,
    pub height: i32,
}
#[cfg(windows)]
pub fn screen_and_cursor() -> Result<(Screen, (i32, i32))> {
    use windows_sys::Win32::{
        Foundation::POINT,
        Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint},
        UI::WindowsAndMessaging::GetPhysicalCursorPos,
    };
    unsafe {
        let mut cursor = POINT { x: 0, y: 0 };
        if GetPhysicalCursorPos(&mut cursor) == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let monitor = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut info) == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let screen = Screen {
            width: info.rcMonitor.right - info.rcMonitor.left,
            height: info.rcMonitor.bottom - info.rcMonitor.top,
        };
        Ok((
            screen,
            (
                cursor.x - info.rcMonitor.left,
                cursor.y - info.rcMonitor.top,
            ),
        ))
    }
}
#[cfg(not(windows))]
pub fn screen_and_cursor() -> Result<(Screen, (i32, i32))> {
    bail!("滑鼠實體座標功能僅支援 Windows")
}

pub fn enable_dpi_awareness() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::UI::HiDpi::{
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
        };
        // The embedded manifest also sets this; ERROR_ACCESS_DENIED means already configured.
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}
pub fn key_a_down() -> bool {
    #[cfg(windows)]
    {
        unsafe {
            windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(0x41) as u16 & 0x8000
                != 0
        }
    }
    #[cfg(not(windows))]
    {
        false
    }
}

pub struct Counter {
    #[cfg(windows)]
    frequency: i64,
    #[cfg(not(windows))]
    start: Instant,
}
impl Counter {
    pub fn new() -> Result<Self> {
        #[cfg(windows)]
        unsafe {
            let mut frequency = 0;
            if windows_sys::Win32::System::Performance::QueryPerformanceFrequency(&mut frequency)
                == 0
                || frequency <= 0
            {
                bail!("QueryPerformanceFrequency 失敗");
            }
            Ok(Self { frequency })
        }
        #[cfg(not(windows))]
        {
            Ok(Self {
                start: Instant::now(),
            })
        }
    }
    pub fn ticks(&self) -> i64 {
        #[cfg(windows)]
        unsafe {
            let mut value = 0;
            windows_sys::Win32::System::Performance::QueryPerformanceCounter(&mut value);
            value
        }
        #[cfg(not(windows))]
        {
            self.start.elapsed().as_nanos() as i64
        }
    }
    pub fn millis(&self, delta: i64) -> f64 {
        #[cfg(windows)]
        {
            delta as f64 * 1000.0 / self.frequency as f64
        }
        #[cfg(not(windows))]
        {
            delta as f64 / 1_000_000.0
        }
    }
}

pub struct PriorityGuard {
    #[cfg(windows)]
    process_class: u32,
    #[cfg(windows)]
    thread_priority: i32,
}
impl PriorityGuard {
    pub fn set(extreme: bool, high: bool) -> Result<Self> {
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::System::Threading::*;
            let process = GetCurrentProcess();
            let thread = GetCurrentThread();
            let old_class = GetPriorityClass(process);
            let old_thread = GetThreadPriority(thread);
            if old_class == 0 || old_thread == i32::MAX {
                return Err(std::io::Error::last_os_error().into());
            }
            let guard = Self {
                process_class: old_class,
                thread_priority: old_thread,
            };
            if extreme && SetPriorityClass(process, REALTIME_PRIORITY_CLASS) == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            let priority = if extreme {
                THREAD_PRIORITY_TIME_CRITICAL
            } else if high {
                THREAD_PRIORITY_ABOVE_NORMAL
            } else {
                THREAD_PRIORITY_NORMAL
            };
            if SetThreadPriority(thread, priority) == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(guard)
        }
        #[cfg(not(windows))]
        {
            let _ = (extreme, high);
            bail!("優先權控制僅支援 Windows")
        }
    }
}
impl Drop for PriorityGuard {
    fn drop(&mut self) {
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::System::Threading::*;
            SetThreadPriority(GetCurrentThread(), self.thread_priority);
            SetPriorityClass(GetCurrentProcess(), self.process_class);
        }
    }
}

pub fn application_dir() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        Ok(PathBuf::from(
            std::env::var_os("LOCALAPPDATA").ok_or_else(|| anyhow::anyhow!("缺少 LOCALAPPDATA"))?,
        )
        .join("FirmwareManageTool_Rust"))
    }
    #[cfg(not(windows))]
    {
        Ok(std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("firmware-manage-tool-development"))
            .join("FirmwareManageTool_Rust"))
    }
}
pub fn firmware_dir() -> PathBuf {
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        let dir = parent.join("firmware");
        if dir.is_dir() {
            return dir;
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("firmware")
}

pub fn locate_program(name: &str) -> Option<PathBuf> {
    gdb::platform::locate_program(name)
}

pub fn boot_drives() -> Result<Vec<PathBuf>> {
    Ok(gdb::flash::drives()?)
}

pub fn wait_until(deadline: Instant, cancel: &crate::cancel::Cancellation) -> Result<()> {
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        cancel.sleep(remaining.min(Duration::from_millis(10)))?;
    }
    cancel.check()
}
