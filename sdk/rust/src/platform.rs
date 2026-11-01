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
use serde::Serialize;

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

    /// Whether the two rectangles share at least one pixel.
    pub fn intersects(&self, other: &Self) -> bool {
        self.left < other.right
            && other.left < self.right
            && self.top < other.bottom
            && other.top < self.bottom
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
    /// [`monitors`] reports the layout so a caller can pick an index it has seen.
    Monitor(usize),
    /// The monitor whose rectangle contains this point, found at resolve time.
    ///
    /// This is the anchor to use when the destination is a coordinate rather
    /// than a known display: it follows the point onto whichever monitor holds
    /// it, instead of silently clamping to the primary monitor. A point on no
    /// monitor falls back to the union of every screen, so the failure message
    /// can name the desktop.
    Containing(i32, i32),
    /// A window's bounding rectangle, `hwnd` being the caller's pointer value.
    Window(isize),
}

impl std::fmt::Display for Screen {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Primary => formatter.write_str("the primary monitor"),
            Self::Monitor(index) => write!(formatter, "monitor {index}"),
            Self::Containing(x, y) => write!(formatter, "the monitor containing ({x}, {y})"),
            Self::Window(hwnd) => write!(formatter, "window {hwnd:#x}"),
        }
    }
}

/// One monitor as the system reports it, for callers that need to pick an index
/// for [`Screen::Monitor`] instead of guessing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Monitor {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    /// Whether this is the monitor Windows treats as the primary one, which is
    /// what [`Screen::Primary`] resolves to.
    pub primary: bool,
    /// Effective DPI (96 = 100%). 0 when the system will not report it.
    pub dpi: u32,
}

impl Monitor {
    pub fn rect(&self) -> Rect {
        Rect {
            left: self.left,
            top: self.top,
            right: self.right,
            bottom: self.bottom,
        }
    }
}

/// Every monitor in `EnumDisplayMonitors` order: the index is exactly the one
/// [`Screen::Monitor`] takes, and `primary` says which entry `Screen::Primary`
/// uses.
pub fn monitors() -> Result<Vec<Monitor>> {
    #[cfg(windows)]
    {
        // Without per-monitor awareness the rectangles and DPI come back
        // virtualized, and a caller that then feeds an index into a move would
        // anchor on a scaled rectangle.
        enable_dpi_awareness();
        enumerate()
    }
    #[cfg(not(windows))]
    {
        Err(Error::unsupported(
            "monitor enumeration needs Windows display APIs",
        ))
    }
}

/// [`monitors`] as a JSON array, the shape hosts log and dashboards read.
pub fn monitors_json() -> Result<String> {
    serde_json::to_string(&monitors()?).map_err(|error| Error::io(error.to_string()))
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
            Screen::Primary | Screen::Monitor(0) | Screen::Containing(..) => {}
            Screen::Monitor(index) => {
                return Err(Error::invalid_screen(format!(
                    "monitor index {index} is out of range: a mock device has one 1920x1080 screen"
                )));
            }
            Screen::Window(hwnd) => {
                return Err(Error::stale_window(format!(
                    "window {hwnd:#x} has no rectangle: a mock device has no windows"
                )));
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
            Error::invalid_screen(format!(
                "monitor index {index} is out of range: the system reports {} monitor(s)",
                screens.len()
            ))
        })?,
        Screen::Containing(x, y) => screens
            .iter()
            .copied()
            .find(|rect| rect.contains((x, y)))
            .unwrap_or_else(|| {
                screens
                    .iter()
                    .copied()
                    .reduce(|left, right| left.union(&right))
                    .unwrap_or_default()
            }),
        Screen::Window(hwnd) => {
            let rect = desktop.window(hwnd)?;
            if rect.width() <= 0 || rect.height() <= 0 {
                return Err(Error::stale_window(format!(
                    "window {hwnd:#x} has an empty rectangle ({}x{}): it may be minimised or hidden",
                    rect.width(),
                    rect.height()
                )));
            }
            if !screens.iter().any(|screen| screen.intersects(&rect)) {
                return Err(Error::stale_window(format!(
                    "window {hwnd:#x} is on no monitor, its rectangle is ({}, {})-({}, {}): \
                     it may be minimised or hidden",
                    rect.left, rect.top, rect.right, rect.bottom
                )));
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
        Ok(enumerate()?.iter().map(Monitor::rect).collect())
    }

    fn primary(&self) -> Result<usize> {
        Ok(enumerate()?
            .iter()
            .position(|monitor| monitor.primary)
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
            return Err(Error::stale_window(format!(
                "GetWindowRect failed for window {hwnd:#x}: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(Rect {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        })
    }
}

/// Every monitor in `EnumDisplayMonitors` order, with its primary flag and its
/// effective DPI.
#[cfg(windows)]
fn enumerate() -> Result<Vec<Monitor>> {
    use windows_sys::Win32::{
        Foundation::{LPARAM, RECT},
        Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO},
        UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
        UI::WindowsAndMessaging::MONITORINFOF_PRIMARY,
    };
    use windows_sys::core::BOOL;

    unsafe extern "system" fn collect(
        monitor: HMONITOR,
        _hdc: HDC,
        _clip: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        let out = unsafe { &mut *(data as *mut Vec<Monitor>) };
        let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
            return 1;
        }
        let (mut dpi_x, mut dpi_y) = (0u32, 0u32);
        let dpi =
            match unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) } {
                0 => dpi_x,
                _ => 0,
            };
        out.push(Monitor {
            left: info.rcMonitor.left,
            top: info.rcMonitor.top,
            right: info.rcMonitor.right,
            bottom: info.rcMonitor.bottom,
            primary: info.dwFlags & MONITORINFOF_PRIMARY != 0,
            dpi,
        });
        1
    }

    let mut out: Vec<Monitor> = Vec::new();
    let ok = unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(collect),
            &mut out as *mut Vec<Monitor> as LPARAM,
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
            if hwnd == 0x5678 {
                // What a minimised window reports: a small rectangle parked far
                // outside every monitor.
                return Ok(Rect {
                    left: -32000,
                    top: -32000,
                    right: -31840,
                    bottom: -31972,
                });
            }
            if hwnd != 0x1234 {
                return Err(Error::stale_window(format!(
                    "GetWindowRect failed for window {hwnd:#x}: no such window"
                )));
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
        assert_eq!(failure.code(), ErrorCode::InvalidScreen);
        assert_eq!(
            failure.message,
            "monitor index 2 is out of range: the system reports 2 monitor(s)"
        );
    }

    #[test]
    fn a_containing_anchor_follows_the_point_onto_that_monitor() {
        let desktop = Fake::new((2500, 500), 0);
        let second = resolve(&desktop, Screen::Containing(2500, 900)).expect("second monitor");
        assert_eq!(second.rect.origin(), (1920, 0));
        assert_eq!((second.rect.width(), second.rect.height()), (1280, 1024));
        assert_eq!(
            second.rect.clamp((2000, 300)),
            (2000, 300),
            "a point inside the found monitor is not clamped away"
        );

        let first = resolve(&desktop, Screen::Containing(100, 100)).expect("first monitor");
        assert_eq!(first.rect.origin(), (0, 0));
        assert_eq!(first.rect.width(), 1920);
    }

    #[test]
    fn a_containing_point_on_no_monitor_falls_back_to_the_union() {
        let desktop = Fake::new((2500, 500), 0);
        let target = resolve(&desktop, Screen::Containing(4000, 2000)).expect("union fallback");
        assert_eq!(target.rect, target.bounds(), "the fallback is the union");
        assert_eq!(target.rect.origin(), (0, 0));
        assert_eq!((target.rect.width(), target.rect.height()), (3200, 1080));
        assert!(
            !target.reaches((4000, 2000)),
            "the move is still refused, but by aim_at with a message naming the desktop"
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
        assert_eq!(failure.code(), ErrorCode::StaleWindow);
        assert!(
            failure.message.contains("GetWindowRect failed"),
            "{failure}"
        );
    }

    #[test]
    fn a_window_off_every_monitor_is_a_stale_window() {
        let desktop = Fake::new((100, 100), 0);
        let failure = resolve(&desktop, Screen::Window(0x5678)).unwrap_err();
        assert_eq!(failure.code(), ErrorCode::StaleWindow);
        assert!(
            failure.message.contains("is on no monitor"),
            "a minimised window must not read as `unsupported`: {failure}"
        );
    }

    #[test]
    fn monitor_json_carries_the_rect_the_primary_flag_and_the_dpi() {
        let monitor = Monitor {
            left: 1920,
            top: 0,
            right: 3200,
            bottom: 1024,
            primary: false,
            dpi: 144,
        };
        assert_eq!(
            serde_json::to_string(&monitor).unwrap(),
            "{\"left\":1920,\"top\":0,\"right\":3200,\"bottom\":1024,\
             \"primary\":false,\"dpi\":144}"
        );
        assert_eq!(monitor.rect().origin(), (1920, 0));
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
            ErrorCode::InvalidScreen
        );
        assert_eq!(
            Target::mock(Screen::Window(1), (0, 0)).unwrap_err().code(),
            ErrorCode::StaleWindow
        );
        assert!(
            Target::mock(Screen::Containing(5000, 5000), (0, 0)).is_ok(),
            "a mock has one screen, so a containing anchor resolves to it"
        );
    }

    /// The real desktop, not a fake: this is the check that a coordinate on a
    /// second monitor anchors on that monitor instead of the primary one.
    #[cfg(windows)]
    #[test]
    fn the_system_anchors_a_containing_point_on_the_monitor_that_holds_it() {
        let monitors = super::monitors().expect("the system lists its displays");
        assert!(!monitors.is_empty());
        assert_eq!(
            monitors.iter().filter(|monitor| monitor.primary).count(),
            1,
            "exactly one monitor is the primary one"
        );

        let first = monitors[0];
        let point = (
            (first.left + first.right) / 2,
            (first.top + first.bottom) / 2,
        );
        let target = resolve(&System, Screen::Containing(point.0, point.1)).expect("containing");
        assert_eq!(target.rect.origin(), (first.left, first.top));
        assert!(target.rect.contains(point));

        let bounds = monitors
            .iter()
            .map(Monitor::rect)
            .reduce(|left, right| left.union(&right))
            .expect("a desktop");
        let outside = (bounds.right + 10, bounds.top + 10);
        let fallback =
            resolve(&System, Screen::Containing(outside.0, outside.1)).expect("union fallback");
        assert_eq!(fallback.rect, fallback.bounds());
        assert!(!fallback.reaches(outside));
    }
}
