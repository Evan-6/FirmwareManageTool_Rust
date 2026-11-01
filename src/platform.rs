use anyhow::{Result, bail};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

pub use gdb_motion::Screen;
#[cfg(windows)]
pub fn screen_and_cursor() -> Result<(Screen, (i32, i32))> {
    gdb::platform::screen_and_cursor().map_err(read_failure)
}
#[cfg(not(windows))]
pub fn screen_and_cursor() -> Result<(Screen, (i32, i32))> {
    bail!("滑鼠實體座標功能僅支援 Windows")
}

#[cfg(windows)]
fn read_failure(error: gdb::Error) -> anyhow::Error {
    anyhow::anyhow!("讀取游標與螢幕資訊失敗：{error}")
}

/// The same DPI setting `gdb::platform` applies on its own paths.
pub fn enable_dpi_awareness() {
    gdb::platform::enable_dpi_awareness();
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

pub fn wait_until(deadline: Instant, cancel: &crate::cancel::Cancellation) -> Result<()> {
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        cancel.sleep(remaining.min(Duration::from_millis(10)))?;
    }
    cancel.check()
}
