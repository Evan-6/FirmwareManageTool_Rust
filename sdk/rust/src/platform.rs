//! Platform facts the protocol does not provide: the OS cursor position and the
//! monitor rectangles a move can be anchored to. Absolute movement is computed
//! host side from these.
//!
//! Coordinates here are physical pixels in virtual-desktop space, the same space
//! `GetMonitorInfoW` reports: the primary monitor's top-left corner is `(0, 0)`
//! and a second monitor to its right starts at the primary's width. The process
//! is per-monitor DPI aware before any of this is read, so nothing is scaled.

use crate::error::{Error, ErrorCode, Result};
use gdb_motion::Screen as ScreenSize;

/// A physical-pixel rectangle in virtual-desktop coordinates.
///
/// Monitors are half-open on the right and bottom edges: the pixel at `right`
/// belongs to whatever sits next to it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }

    pub fn origin(&self) -> (i32, i32) {
        (self.left, self.top)
    }

    pub fn contains(&self, point: (i32, i32)) -> bool {
        point.0 >= self.left && point.0 < self.right && point.1 >= self.top && point.1 < self.bottom
    }

    pub fn union(&self, other: &Self) -> Self {
        Self {
            left: self.left.min(other.left),
            top: self.top.min(other.top),
            right: self.right.max(other.right),
            bottom: self.bottom.max(other.bottom),
        }
    }

    /// The smallest rectangle that covers this one and one more point.
    pub fn union_point(&self, point: (i32, i32)) -> Self {
        Self {
            left: self.left.min(point.0),
            top: self.top.min(point.1),
            right: self.right.max(point.0 + 1),
            bottom: self.bottom.max(point.1 + 1),
        }
    }

    /// The nearest point inside the rectangle, which is how a caller's coarse
    /// target still lands on an edge instead of failing.
    pub fn clamp(&self, point: (i32, i32)) -> (i32, i32) {
        (
            point.0.clamp(self.left, (self.right - 1).max(self.left)),
            point.1.clamp(self.top, (self.bottom - 1).max(self.top)),
        )
    }
}

/// Which rectangle an absolute move is anchored to.
///
/// `Primary` is the default and keeps the coordinates callers have always used.
/// The other two exist because a game on a second screen or in a window cannot be
/// reached from a rectangle anchored at the primary monitor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Screen {
    /// The primary monitor, whatever the multi-monitor layout looks like.
    #[default]
    Primary,
    /// The `index`-th monitor in `EnumDisplayMonitors` order.
    ///
    /// That order is the order the system enumerates the displays, so it is not
    /// the display number Windows settings shows and index 0 need not be the
    /// primary monitor; a caller that wants the primary one asks for `Primary`.
    Monitor(usize),
    /// A window's bounding rectangle, `hwnd` being the caller's pointer value.
    Window(isize),
}

impl std::fmt::Display for Screen {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Primary => formatter.write_str("the primary monitor"),
            Self::Monitor(index) => write!(formatter, "monitor {index}"),
            Self::Window(hwnd) => write!(formatter, "window {hwnd:#x}"),
        }
    }
}

/// The pointer, the rectangle a move is anchored to, and every monitor the system
/// reports — all in virtual-desktop physical pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// The requested rectangle: the move is clamped into it.
    pub rect: Rect,
    /// Where the pointer actually is, which may be on another monitor.
    pub cursor: (i32, i32),
    /// Every monitor, so "is this point on a screen at all" can be answered.
    pub screens: Vec<Rect>,
}

impl Target {
    pub fn new(rect: Rect, cursor: (i32, i32), screens: Vec<Rect>) -> Self {
        Self {
            rect,
            cursor,
            screens,
        }
    }

    /// The single 1920x1080 desktop a mock device pretends to have.
    pub fn mock(screen: Screen, cursor: (i32, i32)) -> Result<Self> {
        match screen {
            Screen::Primary | Screen::Monitor(0) => {}
            Screen::Monitor(index) => {
                return Err(Error::new(
                    ErrorCode::Unsupported,
                    format!(
                        "monitor index {index} is out of range: a mock device has one 1920x1080 screen"
                    ),
                ));
            }
            Screen::Window(hwnd) => {
                return Err(Error::new(
                    ErrorCode::Unsupported,
                    format!("window {hwnd:#x} has no rectangle: a mock device has no windows"),
                ));
            }
        }
        let rect = Rect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        Ok(Self::new(rect, cursor, vec![rect]))
    }

    /// Union of every monitor: the desktop as one rectangle, for messages.
    pub fn bounds(&self) -> Rect {
        self.screens
            .iter()
            .copied()
            .reduce(|left, right| left.union(&right))
            .unwrap_or(self.rect)
    }

    /// Whether the pointer can physically reach this point, i.e. some monitor
    /// covers it. Points in a gap between monitors are not reachable.
    pub fn reaches(&self, point: (i32, i32)) -> bool {
        self.screens.iter().any(|rect| rect.contains(point))
    }
}

/// The platform queries target resolution needs. Tests implement this instead of
/// moving a real pointer; [`System`] is the Windows one.
pub trait Desktop {
    /// Every monitor rectangle, in enumeration order.
    fn monitors(&self) -> Result<Vec<Rect>>;
    /// Index into [`Desktop::monitors`] of the primary monitor.
    fn primary(&self) -> Result<usize>;
    /// The pointer, in virtual-desktop pixels.
    fn cursor(&self) -> Result<(i32, i32)>;
    /// A window's bounding rectangle, in virtual-desktop pixels.
    fn window(&self, hwnd: isize) -> Result<Rect>;
}

/// Resolves `screen` against a desktop, without touching the pointer.
pub fn resolve(desktop: &dyn Desktop, screen: Screen) -> Result<Target> {
    let screens = desktop.monitors()?;
    if screens.is_empty() {
        return Err(Error::new(
            ErrorCode::Unsupported,
            "the system reports no monitors",
        ));
    }
    let rect = match screen {
        Screen::Primary => {
            let index = desktop.primary()?;
            *screens.get(index).ok_or_else(|| {
                Error::new(
                    ErrorCode::Unsupported,
                    format!(
                        "the primary monitor is index {index}, but the system reports {} monitor(s)",
                        screens.len()
                    ),
                )
            })?
        }
        Screen::Monitor(index) => *screens.get(index).ok_or_else(|| {
            Error::new(
                ErrorCode::Unsupported,
                format!(
                    "monitor index {index} is out of range: the system reports {} monitor(s)",
                    screens.len()
                ),
            )
        })?,
        Screen::Window(hwnd) => {
            let rect = desktop.window(hwnd)?;
            if rect.width() <= 0 || rect.height() <= 0 {
                return Err(Error::new(
                    ErrorCode::Unsupported,
                    format!(
                        "window {hwnd:#x} has an empty rectangle ({}x{})",
                        rect.width(),
                        rect.height()
                    ),
                ));
            }
            rect
        }
    };
    Ok(Target::new(rect, desktop.cursor()?, screens))
}

/// The message an absolute move reports when its point is on no monitor at all:
/// which target was asked for and the desktop the host could see.
pub fn outside_message(screen: Screen, point: (i32, i32), bounds: Rect) -> String {
    format!(
        "absolute move to ({}, {}) targets {screen}, but that point is on no screen: \
         the monitors cover ({}, {})-({}, {})",
        point.0, point.1, bounds.left, bounds.top, bounds.right, bounds.bottom
    )
}

/// Target resolution plus the pointer, for one absolute move.
pub fn screen_and_cursor_for(screen: Screen) -> Result<Target> {
    #[cfg(windows)]
    {
        resolve(&System, screen)
    }
    #[cfg(not(windows))]
    {
        let _ = screen;
        Err(Error::unsupported(
            "absolute mouse coordinates need Windows cursor APIs",
        ))
    }
}

/// Primary monitor rect and the cursor relative to its top-left corner: the
/// shape callers used before targets existed.
pub fn screen_and_cursor() -> Result<(ScreenSize, (i32, i32))> {
    let target = screen_and_cursor_for(Screen::Primary)?;
    Ok((
        ScreenSize {
            width: target.rect.width(),
            height: target.rect.height(),
        },
        (
            target.cursor.0 - target.rect.left,
            target.cursor.1 - target.rect.top,
        ),
    ))
}

/// The live Windows desktop.
#[cfg(windows)]
pub struct System;

#[cfg(windows)]
impl Desktop for System {
    fn monitors(&self) -> Result<Vec<Rect>> {
        Ok(enumerate()?.into_iter().map(|(rect, _)| rect).collect())
    }

    fn primary(&self) -> Result<usize> {
        use windows_sys::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;
        Ok(enumerate()?
            .iter()
            .position(|(_, flags)| flags & MONITORINFOF_PRIMARY != 0)
            .unwrap_or(0))
    }

    fn cursor(&self) -> Result<(i32, i32)> {
        use windows_sys::Win32::{
            Foundation::POINT, UI::WindowsAndMessaging::GetPhysicalCursorPos,
        };
        let mut cursor = POINT { x: 0, y: 0 };
        if unsafe { GetPhysicalCursorPos(&mut cursor) } == 0 {
            return Err(Error::new(
                ErrorCode::Unsupported,
                format!(
                    "GetPhysicalCursorPos failed: {}",
                    std::io::Error::last_os_error()
                ),
            ));
        }
        Ok((cursor.x, cursor.y))
    }

    fn window(&self, hwnd: isize) -> Result<Rect> {
        use windows_sys::Win32::{Foundation::RECT, UI::WindowsAndMessaging::GetWindowRect};
        let mut rect: RECT = unsafe { std::mem::zeroed() };
        if unsafe { GetWindowRect(hwnd as *mut core::ffi::c_void, &mut rect) } == 0 {
            return Err(Error::new(
                ErrorCode::Unsupported,
                format!(
                    "GetWindowRect failed for window {hwnd:#x}: {}",
                    std::io::Error::last_os_error()
                ),
            ));
        }
        Ok(Rect {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        })
    }
}

/// Every monitor rectangle and its flags, in `EnumDisplayMonitors` order.
#[cfg(windows)]
fn enumerate() -> Result<Vec<(Rect, u32)>> {
    use windows_sys::Win32::{
        Foundation::{LPARAM, RECT},
        Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO},
    };
    use windows_sys::core::BOOL;

    unsafe extern "system" fn collect(
        monitor: HMONITOR,
        _hdc: HDC,
        _clip: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        let out = unsafe { &mut *(data as *mut Vec<(Rect, u32)>) };
        let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if unsafe { GetMonitorInfoW(monitor, &mut info) } != 0 {
            out.push((
                Rect {
                    left: info.rcMonitor.left,
                    top: info.rcMonitor.top,
                    right: info.rcMonitor.right,
                    bottom: info.rcMonitor.bottom,
                },
                info.dwFlags,
            ));
        }
        1
    }

    let mut out: Vec<(Rect, u32)> = Vec::new();
    let ok = unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(collect),
            &mut out as *mut Vec<(Rect, u32)> as LPARAM,
        )
    };
    if ok == 0 {
        return Err(Error::new(
            ErrorCode::Unsupported,
            format!(
                "EnumDisplayMonitors failed: {}",
                std::io::Error::last_os_error()
            ),
        ));
    }
    if out.is_empty() {
        return Err(Error::new(
            ErrorCode::Unsupported,
            "the system reports no monitors",
        ));
    }
    Ok(out)
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
    use super::*;

    /// Two monitors side by side with a window on the second one, plus whatever
    /// pointer position the test asks for: no real cursor is involved.
    struct Fake {
        cursor: (i32, i32),
        primary: usize,
    }

    impl Fake {
        fn new(cursor: (i32, i32), primary: usize) -> Self {
            Self { cursor, primary }
        }
    }

    impl Desktop for Fake {
        fn monitors(&self) -> Result<Vec<Rect>> {
            Ok(vec![
                Rect {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1080,
                },
                Rect {
                    left: 1920,
                    top: 0,
                    right: 3200,
                    bottom: 1024,
                },
            ])
        }

        fn primary(&self) -> Result<usize> {
            Ok(self.primary)
        }

        fn cursor(&self) -> Result<(i32, i32)> {
            Ok(self.cursor)
        }

        fn window(&self, hwnd: isize) -> Result<Rect> {
            if hwnd != 0x1234 {
                return Err(Error::new(
                    ErrorCode::Unsupported,
                    format!("GetWindowRect failed for window {hwnd:#x}: no such window"),
                ));
            }
            Ok(Rect {
                left: 2000,
                top: 100,
                right: 3000,
                bottom: 700,
            })
        }
    }

    #[test]
    fn dpi_awareness_is_callable_more_than_once() {
        super::enable_dpi_awareness();
        super::enable_dpi_awareness();
    }

    #[test]
    fn primary_is_the_flagged_monitor_not_index_zero() {
        let desktop = Fake::new((2500, 500), 1);
        let target = resolve(&desktop, Screen::Primary).expect("primary");
        assert_eq!(target.rect.left, 1920, "primary must follow the flag");
        assert_eq!(target.rect.width(), 1280);
        assert_eq!(
            target.cursor,
            (2500, 500),
            "the cursor stays in desktop space"
        );
        assert!(target.reaches((100, 100)), "the first monitor is reachable");
    }

    #[test]
    fn a_monitor_index_selects_that_rect_and_rejects_the_rest() {
        let desktop = Fake::new((100, 100), 0);
        let second = resolve(&desktop, Screen::Monitor(1)).expect("second monitor");
        assert_eq!(second.rect.origin(), (1920, 0));
        assert_eq!((second.rect.width(), second.rect.height()), (1280, 1024));
        assert!(second.reaches((1920, 1023)) && !second.reaches((1920, 1024)));

        let failure = resolve(&desktop, Screen::Monitor(2)).unwrap_err();
        assert_eq!(failure.code(), ErrorCode::Unsupported);
        assert_eq!(
            failure.message,
            "monitor index 2 is out of range: the system reports 2 monitor(s)"
        );
    }

    #[test]
    fn a_window_target_uses_the_window_rectangle() {
        let desktop = Fake::new((2100, 200), 0);
        let window = resolve(&desktop, Screen::Window(0x1234)).expect("window");
        assert_eq!(window.rect.origin(), (2000, 100));
        assert_eq!((window.rect.width(), window.rect.height()), (1000, 600));
        assert_eq!(window.bounds().right, 3200, "bounds stay the whole desktop");

        let failure = resolve(&desktop, Screen::Window(0x44)).unwrap_err();
        assert_eq!(failure.code(), ErrorCode::Unsupported);
        assert!(
            failure.message.contains("GetWindowRect failed"),
            "{failure}"
        );
    }

    #[test]
    fn an_off_desktop_point_names_the_target_and_the_union() {
        let desktop = Fake::new((2500, 500), 0);
        let target = resolve(&desktop, Screen::Monitor(1)).expect("second monitor");
        assert!(!target.reaches((4000, 500)));
        assert_eq!(
            outside_message(Screen::Monitor(1), (4000, 500), target.bounds()),
            "absolute move to (4000, 500) targets monitor 1, but that point is on no screen: \
             the monitors cover (0, 0)-(3200, 1080)"
        );
        assert_eq!(
            outside_message(Screen::Primary, (-1, 0), target.bounds()),
            "absolute move to (-1, 0) targets the primary monitor, but that point is on no \
             screen: the monitors cover (0, 0)-(3200, 1080)"
        );
    }

    #[test]
    fn the_mock_desktop_has_one_screen() {
        let target = Target::mock(Screen::Monitor(0), (10, 20)).expect("mock screen");
        assert_eq!(target.rect.width(), 1920);
        assert_eq!(target.cursor, (10, 20));
        assert_eq!(
            Target::mock(Screen::Monitor(1), (0, 0)).unwrap_err().code(),
            ErrorCode::Unsupported
        );
    }
}
