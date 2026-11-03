//! In-memory device used by tests and by every binding's self-test. It speaks
//! the same wire protocol as the firmware, including most error paths.
//!
//! A mock device also has a desktop: [`MockLayout`] is the monitor arrangement
//! an absolute move resolves its anchor against. The default is the single
//! 1920x1080 screen the mock has always had; a layout a caller supplies can put
//! a second monitor at negative coordinates, give it a different DPI, and
//! declare windows, so the coordinate arithmetic that is hardest to get right is
//! testable with no display attached.

use crate::error::{Error, ErrorCode, Result};
use crate::platform::{Desktop, Monitor, Rect, Screen, Target};
use crate::transport::Transport;
use input_protocol::v3::{self, InputState, Packet};
use serde::Deserialize;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Debug, Default)]
pub struct MockState {
    pub writes: Vec<Packet>,
    pub state: InputState,
    pub session: u32,
    pub sessions: Vec<u32>,
    pub closed: bool,
    pub silent: bool,
    pub disconnect_after: Option<usize>,
    pub feature_missing: bool,
    pub build: u32,
    pub uptime_ms: u32,
}

#[derive(Clone, Default)]
pub struct MockHandle(Arc<Mutex<MockState>>);

impl MockHandle {
    pub fn writes(&self) -> Vec<Packet> {
        self.0.lock().unwrap().writes.clone()
    }
    pub fn held(&self) -> InputState {
        self.0.lock().unwrap().state.clone()
    }
    pub fn session(&self) -> u32 {
        self.0.lock().unwrap().session
    }
    pub fn sessions(&self) -> Vec<u32> {
        self.0.lock().unwrap().sessions.clone()
    }
    pub fn is_closed(&self) -> bool {
        self.0.lock().unwrap().closed
    }
    pub fn set_silent(&self, silent: bool) {
        self.0.lock().unwrap().silent = silent;
    }
    pub fn set_disconnect_after(&self, writes: Option<usize>) {
        self.0.lock().unwrap().disconnect_after = writes;
    }
    pub fn set_feature_missing(&self, missing: bool) {
        self.0.lock().unwrap().feature_missing = missing;
    }
    pub fn set_build(&self, build: u32) {
        self.0.lock().unwrap().build = build;
    }
}

/// A window a mock layout declares: the frame and the client area inside it.
///
/// A window is here for the same reason the monitors are: the client-area
/// arithmetic (frame origin plus borders, sometimes at negative coordinates) is
/// the part of an absolute move that fails quietly on a real desktop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MockWindow {
    pub hwnd: isize,
    /// The frame, as `GetWindowRect` reports it.
    pub rect: Rect,
    /// The area inside the frame, as `GetClientRect` plus `ClientToScreen` do.
    pub client: Rect,
}

/// The desktop a mock device pretends to have: the monitors an absolute move is
/// clamped into, the pointer it starts from, and the windows an anchor can name.
///
/// The JSON form is
/// `{"cursor":[x,y],"monitors":[{"rect":[left,top,right,bottom],"dpi":144,"primary":true}],
///   "windows":[{"hwnd":4660,"rect":[…],"client":[…]}]}`.
/// `cursor` defaults to the top-left of the primary monitor (or `[0,0]` when no
/// monitor is marked primary), `primary` and `dpi` default to `false` and `96`,
/// and `windows` may be left out. Monitors are half-open on `right`/`bottom`,
/// exactly like [`platform::Rect`](crate::platform::Rect), so two monitors can
/// sit edge to edge without overlapping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MockLayout {
    pub cursor: (i32, i32),
    pub monitors: Vec<Monitor>,
    pub windows: Vec<MockWindow>,
}

/// The wire form of a layout, so a field the caller misspelled is an error
/// instead of a silently ignored key.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LayoutWire {
    #[serde(default)]
    cursor: Option<[i32; 2]>,
    monitors: Vec<MonitorWire>,
    #[serde(default)]
    windows: Vec<WindowWire>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MonitorWire {
    rect: [i32; 4],
    #[serde(default)]
    dpi: u32,
    #[serde(default)]
    primary: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WindowWire {
    hwnd: i64,
    rect: [i32; 4],
    client: [i32; 4],
}

impl Default for MockLayout {
    fn default() -> Self {
        Self::single((960, 540))
    }
}

impl MockLayout {
    /// The one 1920x1080 desktop, primary and at the origin: what a mock device
    /// has always been, and what a caller that does not supply a layout gets.
    pub fn single(cursor: (i32, i32)) -> Self {
        Self {
            cursor,
            monitors: vec![Monitor {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
                primary: true,
                dpi: 96,
            }],
            windows: Vec::new(),
        }
    }

    /// Reads the JSON form, refusing a layout no desktop could have.
    pub fn parse(json: &str) -> Result<Self> {
        let wire: LayoutWire = serde_json::from_str(json).map_err(|error| {
            Error::invalid(format!(
                "mock layout is not the expected JSON ({{cursor, monitors, windows}}): {error}"
            ))
        })?;
        if wire.monitors.is_empty() {
            return Err(Error::invalid(
                "mock layout needs at least one monitor: no anchor could resolve",
            ));
        }
        let mut monitors = Vec::with_capacity(wire.monitors.len());
        for (index, monitor) in wire.monitors.iter().enumerate() {
            let rect = rect_of(monitor.rect, &format!("monitor {index}"))?;
            monitors.push(Monitor {
                left: rect.left,
                top: rect.top,
                right: rect.right,
                bottom: rect.bottom,
                primary: monitor.primary,
                dpi: monitor.dpi,
            });
        }
        let primary: Vec<usize> = monitors
            .iter()
            .enumerate()
            .filter(|(_, monitor)| monitor.primary)
            .map(|(index, _)| index)
            .collect();
        if primary.len() > 1 {
            return Err(Error::invalid(format!(
                "mock layout marks one monitor primary, but {} are: {}",
                primary.len(),
                primary
                    .iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        let mut windows = Vec::with_capacity(wire.windows.len());
        for (index, window) in wire.windows.iter().enumerate() {
            if window.hwnd == 0 {
                return Err(Error::invalid(format!(
                    "window {index} has hwnd 0: a window anchor needs the handle a host holds"
                )));
            }
            if windows
                .iter()
                .any(|known: &MockWindow| known.hwnd == window.hwnd as isize)
            {
                return Err(Error::invalid(format!(
                    "window {:#x} is declared twice: an anchor would resolve to whichever came first",
                    window.hwnd
                )));
            }
            let rect = rect_of(window.rect, &format!("window {:#x}", window.hwnd))?;
            let client = rect_of(
                window.client,
                &format!("the client area of window {:#x}", window.hwnd),
            )?;
            if client.left < rect.left
                || client.top < rect.top
                || client.right > rect.right
                || client.bottom > rect.bottom
            {
                return Err(Error::invalid(format!(
                    "the client area of window {:#x} is ({}, {})-({}, {}), which is not inside \
                     its frame ({}, {})-({}, {})",
                    window.hwnd,
                    client.left,
                    client.top,
                    client.right,
                    client.bottom,
                    rect.left,
                    rect.top,
                    rect.right,
                    rect.bottom
                )));
            }
            windows.push(MockWindow {
                hwnd: window.hwnd as isize,
                rect,
                client,
            });
        }
        let cursor = match wire.cursor {
            Some([x, y]) => (x, y),
            None => monitors
                .iter()
                .find(|monitor| monitor.primary)
                .map(|monitor| monitor.rect().origin())
                .unwrap_or((0, 0)),
        };
        Ok(Self {
            cursor,
            monitors,
            windows,
        })
    }

    /// The union of every monitor: the desktop as one rectangle.
    pub fn bounds(&self) -> Rect {
        self.monitors
            .iter()
            .map(Monitor::rect)
            .reduce(|left, right| left.union(&right))
            .unwrap_or_default()
    }

    /// Index of the flagged monitor, or 0 when none is flagged: the same rule
    /// [`platform::resolve`](crate::platform::resolve) applies to the system.
    pub fn primary(&self) -> usize {
        self.monitors
            .iter()
            .position(|monitor| monitor.primary)
            .unwrap_or(0)
    }

    /// The layout as the JSON `gdb_monitors_json` reports for a real desktop.
    pub fn monitors_json(&self) -> Result<String> {
        serde_json::to_string(&self.monitors).map_err(|error| Error::io(error.to_string()))
    }

    /// Resolves an anchor against this layout, with `cursor` as where the
    /// pointer is now. Everything the real desktop does — the primary flag, the
    /// index bounds, the containing fallback, the window and client rectangles —
    /// goes through the same resolver, so a mock move and a host move clamp
    /// identically.
    pub fn target(&self, screen: Screen, cursor: (i32, i32)) -> Result<Target> {
        crate::platform::resolve(&LayoutDesktop(self, cursor), screen)
    }
}

/// A rectangle from the four numbers the JSON carries, refusing a degenerate
/// one: a rect with no width or no height covers no pixel and would clamp every
/// move to a corner.
fn rect_of(rect: [i32; 4], what: &str) -> Result<Rect> {
    let [left, top, right, bottom] = rect;
    if right <= left || bottom <= top {
        return Err(Error::invalid(format!(
            "{what} has an empty rectangle ({left}, {top})-({right}, {bottom}): \
             right must be greater than left and bottom greater than top"
        )));
    }
    Ok(Rect {
        left,
        top,
        right,
        bottom,
    })
}

/// The layout as the platform resolver sees it, so a mock desktop is judged by
/// exactly the code that judges the real one.
struct LayoutDesktop<'a>(&'a MockLayout, (i32, i32));

impl Desktop for LayoutDesktop<'_> {
    fn monitors(&self) -> Result<Vec<Rect>> {
        Ok(self.0.monitors.iter().map(Monitor::rect).collect())
    }

    fn primary(&self) -> Result<usize> {
        Ok(self.0.primary())
    }

    fn cursor(&self) -> Result<(i32, i32)> {
        Ok(self.1)
    }

    fn window(&self, hwnd: isize) -> Result<Rect> {
        self.0
            .windows
            .iter()
            .find(|window| window.hwnd == hwnd)
            .map(|window| window.rect)
            .ok_or_else(|| self.no_window(hwnd, "rectangle"))
    }

    fn window_client(&self, hwnd: isize) -> Result<Rect> {
        self.0
            .windows
            .iter()
            .find(|window| window.hwnd == hwnd)
            .map(|window| window.client)
            .ok_or_else(|| self.no_window(hwnd, "client area"))
    }
}

impl LayoutDesktop<'_> {
    /// The message names what the anchor looked for, so a caller reading it knows
    /// whether it asked for the frame or the picture inside it.
    fn no_window(&self, hwnd: isize, what: &str) -> Error {
        Error::stale_window(match self.0.windows.len() {
            0 => {
                format!("window {hwnd:#x} has no {what} here: this mock layout declares no windows")
            }
            count => format!(
                "window {hwnd:#x} has no {what} here: this mock layout declares {count} other \
                 window(s)"
            ),
        })
    }
}

/// The transport half. Keep a `MockHandle` clone to inspect what the SDK sent.
pub struct MockTransport {
    state: Arc<Mutex<MockState>>,
    replies: VecDeque<[u8; 64]>,
}

impl Default for MockTransport {
    fn default() -> Self {
        Self::new().0
    }
}

impl MockTransport {
    pub fn new() -> (Self, MockHandle) {
        let state = Arc::new(Mutex::new(MockState {
            build: 0x0DB0_0001,
            uptime_ms: 1_000,
            ..MockState::default()
        }));
        (
            Self {
                state: state.clone(),
                replies: VecDeque::new(),
            },
            MockHandle(state),
        )
    }

    fn feature_report(&self) -> [u8; 64] {
        let mut report = [0u8; 64];
        report[0] = 14;
        report[1..5].copy_from_slice(b"FMT3");
        report[5] = 3;
        report[6] = 2;
        report[7] = 3;
        report[8] = 2;
        report[9] = 0;
        report[10..12].copy_from_slice(&2000u16.to_le_bytes());
        report[14] = 1;
        report[15..19].copy_from_slice(&15u32.to_le_bytes());
        for entry in input_protocol::KEYS {
            if entry.id.usage_page == 7 && entry.id.usage < 224 {
                let usage = entry.id.usage as usize;
                report[19 + usage / 8] |= 1 << (usage % 8);
            }
        }
        report[47] = 0xFF;
        report[48] = 0x7F;
        report[49..57].copy_from_slice(&[0x42, 0x50, 0x33, 0x30, 0x34, 0x42, 0x38, 0x33]);
        let build = self.state.lock().unwrap().build;
        report[57..61].copy_from_slice(&build.to_le_bytes());
        report
    }

    fn push_reply(&mut self, opcode: u8, session: u32, sequence: u32, payload: &[u8]) {
        let mut report = [0u8; 64];
        report[0] = v3::IN_ID;
        report[1] = 3;
        report[2] = opcode | 0x80;
        report[3] = payload.len() as u8;
        report[5..9].copy_from_slice(&session.to_le_bytes());
        report[9..13].copy_from_slice(&sequence.to_le_bytes());
        report[13..13 + payload.len()].copy_from_slice(payload);
        self.replies.push_back(report);
    }
}

impl Transport for MockTransport {
    fn feature(&mut self) -> Result<Option<[u8; 64]>> {
        if self.state.lock().unwrap().feature_missing {
            return Ok(None);
        }
        Ok(Some(self.feature_report()))
    }

    fn write(&mut self, report: &[u8; 64]) -> Result<()> {
        let packet = Packet::decode(report, v3::OUT_ID)
            .map_err(|error| Error::new(ErrorCode::Framing, error.to_string()))?;
        let mut state = self.state.lock().unwrap();
        state.writes.push(packet.clone());
        if let Some(limit) = state.disconnect_after
            && state.writes.len() > limit
        {
            return Err(Error::disconnected("mock transport disconnected"));
        }
        let session = packet.session;
        let sequence = packet.sequence;
        let payload = packet.payload.clone();
        let silent = state.silent;
        match packet.opcode {
            v3::OPEN => {
                if session == 0 || sequence != 1 {
                    if !silent {
                        drop(state);
                        self.push_reply(v3::OPEN, session, sequence, &[1]);
                    }
                    return Ok(());
                }
                if state.session == session {
                    if !silent {
                        drop(state);
                        self.push_reply(v3::OPEN, session, sequence, &[2]);
                    }
                    return Ok(());
                }
                if state.sessions.len() >= 8 {
                    if !silent {
                        drop(state);
                        self.push_reply(v3::OPEN, session, sequence, &[2]);
                    }
                    return Ok(());
                }
                state.session = session;
                state.sessions.push(session);
                state.closed = false;
                if !silent {
                    drop(state);
                    self.push_reply(v3::OPEN, session, sequence, &[0]);
                }
                return Ok(());
            }
            v3::CLOSE => {
                state.sessions.retain(|id| *id != session);
                if state.session == session {
                    state.session = 0;
                    state.closed = true;
                }
                state.state = InputState::default();
                if !silent {
                    drop(state);
                    self.push_reply(v3::CLOSE, session, sequence, &[0]);
                }
                return Ok(());
            }
            v3::HEARTBEAT => return Ok(()),
            _ => {}
        }
        if state.session != session {
            // Unknown session: the firmware drops input silently.
            return Ok(());
        }
        match packet.opcode {
            v3::KEY => {
                let page = u16::from_le_bytes([payload[0], payload[1]]);
                let usage = u16::from_le_bytes([payload[2], payload[3]]);
                let key = input_protocol::KeyId {
                    usage_page: page,
                    usage,
                };
                if state.state.set(key, payload[4] != 0).is_err() {
                    drop(state);
                    if !silent {
                        self.push_reply(v3::EVENT, session, sequence, &[3]);
                    }
                    return Ok(());
                }
            }
            v3::SNAPSHOT => {
                if let Ok(next) = InputState::decode(&payload) {
                    state.state = next;
                }
            }
            v3::MOUSE_MOVE | v3::WHEEL => {}
            v3::MOUSE_BUTTONS => state.state.buttons = payload[0],
            v3::RELEASE_ALL => {
                state.state = InputState::default();
                if !silent {
                    drop(state);
                    self.push_reply(v3::RELEASE_ALL, session, sequence, &[0]);
                }
                return Ok(());
            }
            v3::BOOTLOADER => {
                if !silent {
                    drop(state);
                    self.push_reply(v3::BOOTLOADER, session, sequence, &[0]);
                }
                return Ok(());
            }
            v3::BARRIER => {
                if !silent {
                    drop(state);
                    self.push_reply(v3::BARRIER, session, sequence, &[0]);
                }
                return Ok(());
            }
            v3::STATUS => {
                let mut body = [0u8; 51];
                body[1..5].copy_from_slice(&sequence.to_le_bytes());
                body[5..9].copy_from_slice(&sequence.to_le_bytes());
                body[10..12].copy_from_slice(&2000u16.to_le_bytes());
                body[20..].copy_from_slice(&state.state.encode());
                if !silent {
                    drop(state);
                    self.push_reply(v3::STATUS, session, sequence, &body);
                }
                return Ok(());
            }
            v3::STATUS_EXT => {
                let mut body = [0u8; 23];
                let uptime = state.uptime_ms;
                let build = state.build;
                body[1..5].copy_from_slice(&uptime.to_le_bytes());
                body[10] = state.sessions.len() as u8;
                body[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
                body[16] = 3;
                body[17..19].copy_from_slice(&1u16.to_le_bytes());
                body[19..23].copy_from_slice(&build.to_le_bytes());
                if !silent {
                    drop(state);
                    self.push_reply(v3::STATUS_EXT, session, sequence, &body);
                }
                return Ok(());
            }
            v3::SESSION_LIST => {
                let mut body = [0u8; 48];
                for (index, id) in state.sessions.iter().take(8).enumerate() {
                    let base = index * 6;
                    body[base..base + 2].copy_from_slice(&(*id as u16).to_le_bytes());
                    body[base + 2..base + 4].copy_from_slice(&2000u16.to_le_bytes());
                }
                if !silent {
                    drop(state);
                    self.push_reply(v3::SESSION_LIST, session, sequence, &body);
                }
                return Ok(());
            }
            _ => {
                drop(state);
                if !silent {
                    self.push_reply(v3::EVENT, session, sequence, &[3]);
                }
                return Ok(());
            }
        }
        Ok(())
    }

    fn read(&mut self, _timeout_ms: i32) -> Result<Option<[u8; 64]>> {
        Ok(self.replies.pop_front())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DeviceOptions;
    use crate::error::ErrorCode;
    use crate::motion::{MotionOptions, MovementShared};

    /// A primary 1920x1080 screen at the origin, a 1280x720 one to its *left* (so
    /// every coordinate on it is negative and at 150% scaling), and a window on
    /// that second screen whose client area sits inside its frame.
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

    fn mock(json: &str) -> crate::Device {
        crate::manager::Manager::open_mock_layout(json, DeviceOptions::default())
            .expect("mock device on the caller's layout")
    }

    #[test]
    fn a_layout_reports_its_monitors_primary_flag_and_dpi() {
        let layout = MockLayout::parse(LAYOUT).expect("layout");
        assert_eq!(layout.cursor, (100, 100));
        assert_eq!(layout.monitors.len(), 2);
        assert!(layout.monitors[0].primary && !layout.monitors[1].primary);
        assert_eq!(layout.monitors[1].dpi, 144);
        assert_eq!(layout.primary(), 0);
        assert_eq!(layout.bounds().origin(), (-1280, 0));
        assert_eq!(
            (layout.bounds().width(), layout.bounds().height()),
            (3200, 1080)
        );

        // The device reports the same layout the layout carries: a host that draws
        // the mock's desktop must not be shown the real one.
        let device = mock(LAYOUT);
        let monitors = device.monitors().expect("monitors");
        assert_eq!(monitors, layout.monitors);
        assert_eq!(
            device.monitors_json().expect("JSON"),
            "[{\"left\":0,\"top\":0,\"right\":1920,\"bottom\":1080,\"primary\":true,\"dpi\":96},\
             {\"left\":-1280,\"top\":0,\"right\":0,\"bottom\":720,\"primary\":false,\"dpi\":144}]"
        );
    }

    #[test]
    fn a_missing_cursor_starts_at_the_primary_monitor() {
        let layout = MockLayout::parse(
            r#"{"monitors":[{"rect":[100,50,300,250],"primary":false},
                            {"rect":[-500,-100,0,300],"primary":true}]}"#,
        )
        .expect("layout");
        assert_eq!(
            layout.cursor,
            (-500, -100),
            "no cursor: the primary monitor's origin"
        );
        assert_eq!(layout.primary(), 1, "the flag decides, not index 0");
        let target = layout
            .target(Screen::Primary, layout.cursor)
            .expect("primary");
        assert_eq!(target.rect.origin(), (-500, -100));

        // No primary flag at all falls back to index 0 and [0, 0].
        let layout =
            MockLayout::parse(r#"{"monitors":[{"rect":[100,50,300,250]}]}"#).expect("layout");
        assert_eq!(layout.primary(), 0);
        assert_eq!(layout.cursor, (0, 0));
    }

    #[test]
    fn an_absolute_move_lands_on_the_negative_screen_and_clamps_into_the_anchor() {
        let device = mock(LAYOUT);
        let options = MotionOptions::linear(80).with_screen(Screen::Containing(-500, 300));
        let shared = MovementShared::default();

        // Containing follows the point onto the left-hand screen.
        crate::motion::run_move_to(&device, -500, 300, &options, &shared).expect("containing move");
        let landed = device.cursor();
        assert!(
            (landed.0 + 500).abs() <= 1 && (landed.1 - 300).abs() <= 1,
            "the pointer must reach a negative coordinate, not {landed:?}"
        );

        // The same point anchored on the primary monitor is reachable but outside
        // it, so the move is clamped and the report says so.
        let report = device
            .mouse()
            .move_to_report(
                -500,
                300,
                MotionOptions::linear(80).with_screen(Screen::Monitor(0)),
            )
            .expect("clamped move");
        assert!(report.clamped, "{report:?}");
        assert!(
            report.applied_x.abs() <= 1 && (report.applied_y - 300).abs() <= 1,
            "the clamp aims at (0, 300), not {report:?}"
        );
        assert!((499..=501).contains(&report.error_px()), "{report:?}");
        assert_eq!(
            device.metrics().clamped_total,
            1,
            "only the primary-anchored move had to be pulled back"
        );
    }

    #[test]
    fn a_client_area_anchor_uses_the_declared_window_not_the_frame() {
        let device = mock(LAYOUT);
        let options = MotionOptions::linear(80).with_screen(Screen::WindowClient(0x1234));

        // A point just under the title bar is inside the frame and outside the
        // picture, so it lands on the client area's top-left corner.
        let report = device
            .mouse()
            .move_to_report(-1275, 0, options)
            .expect("client move");
        assert!(report.clamped, "{report:?}");
        assert!(
            (report.applied_x + 1270).abs() <= 1 && (report.applied_y - 9).abs() <= 1,
            "the client area starts at its own origin, not the frame's: {report:?}"
        );

        // Inside the picture the move is not clamped at all.
        let report = device
            .mouse()
            .move_to_report(-600, 300, options)
            .expect("client move");
        assert!(!report.clamped, "{report:?}");
        assert!((report.applied_x + 600).abs() <= 1 && (report.applied_y - 300).abs() <= 1);
    }

    #[test]
    fn a_layout_that_no_desktop_could_have_is_refused_with_the_reason() {
        let cases = [
            (
                r#"{"monitors":[]}"#,
                "mock layout needs at least one monitor",
            ),
            (
                r#"{"monitors":[{"rect":[100,0,100,720]}]}"#,
                "monitor 0 has an empty rectangle (100, 0)-(100, 720)",
            ),
            (
                r#"{"monitors":[{"rect":[0,0,1920,1080]},{"rect":[1920,0,0,1080]}]}"#,
                "monitor 1 has an empty rectangle (1920, 0)-(0, 1080)",
            ),
            (
                r#"{"monitors":[{"rect":[0,0,960,720],"primary":true},
                                {"rect":[960,0,1920,720],"primary":true}]}"#,
                "mock layout marks one monitor primary, but 2 are: 0, 1",
            ),
            (
                r#"{"monitors":[{"rect":[0,0,960,720]}],
                    "windows":[{"hwnd":0,"rect":[0,0,100,100],"client":[0,0,100,100]}]}"#,
                "window 0 has hwnd 0",
            ),
            (
                r#"{"monitors":[{"rect":[0,0,960,720]}],
                    "windows":[{"hwnd":16,"rect":[0,0,100,100],"client":[0,0,100,100]},
                               {"hwnd":16,"rect":[0,0,100,100],"client":[0,0,100,100]}]}"#,
                "window 0x10 is declared twice",
            ),
            (
                r#"{"monitors":[{"rect":[0,0,960,720]}],
                    "windows":[{"hwnd":16,"rect":[0,0,100,100],"client":[0,0,200,100]}]}"#,
                "which is not inside its frame",
            ),
            (
                r#"{"monitors":[{"rect":[0,0,960,720]}],"curser":[1,2]}"#,
                "mock layout is not the expected JSON",
            ),
        ];
        for (json, message) in cases {
            let error = MockLayout::parse(json).expect_err(json);
            assert_eq!(error.code(), ErrorCode::Invalid, "{json}");
            assert!(
                error.message.contains(message),
                "expected {message:?} in {:?}",
                error.message
            );
        }
        // And the same refusal reaches a caller that opens a device with it.
        match crate::manager::Manager::open_mock_layout(
            r#"{"monitors":[]}"#,
            DeviceOptions::default(),
        ) {
            Ok(_) => panic!("an empty layout must not open a device"),
            Err(error) => {
                assert_eq!(error.code(), ErrorCode::Invalid);
                assert!(error.message.contains("at least one monitor"), "{error}");
            }
        }
    }

    #[test]
    fn a_single_screen_layout_is_the_desktop_the_mock_always_had() {
        let layout = MockLayout::default();
        assert_eq!(layout.cursor, (960, 540));
        assert_eq!(layout.monitors.len(), 1);
        assert_eq!(layout.monitors[0].right, 1920);
        let device = crate::manager::Manager::open_mock(DeviceOptions::default()).expect("mock");
        assert_eq!(
            device.mock_layout().map(|layout| layout.monitors.clone()),
            Some(layout.monitors),
            "a plain mock device still has one 1920x1080 screen"
        );
    }
}
