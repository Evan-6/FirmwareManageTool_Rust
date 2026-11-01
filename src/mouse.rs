//! Mouse control: the humanised absolute path, drags, clicks, wheel and raw
//! relative moves.
//!
//! The trajectory maths is `gdb_motion` (the SDK's pure, integer-friendly model)
//! and the device work is `gdb::Mouse`; what stays here is the tool's own loop,
//! which exists because every step has to honour the worker's `Cancellation`.

use crate::{
    cancel::Cancellation,
    hid::Session,
    platform::{self, Screen},
    protocol::Status,
};
use anyhow::{Result, ensure};
use input_protocol::pointer::{RelativeTracker, Step, TICK};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

pub use gdb_motion::{Node, current_node, split_delta};

/// Knobs for the humanised path. Defaults reproduce the shape the engine always
/// had; everything is exposed so the UI can tune it instead of guessing.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct HumanProfile {
    /// 0 = straight line, 1 = the original bow, 2 = exaggerated arc.
    pub curve: f64,
    /// Micro-jitter amplitude in pixels (0 = perfectly smooth).
    pub jitter_px: f64,
    /// Percent of the distance the pointer overshoots before correcting.
    pub overshoot_pct: f64,
    /// Pause at the end, the way a hand settles before clicking (ms).
    pub settle_ms: u32,
    /// How progress maps to time.
    pub ease: Ease,
    /// Ignore steps smaller than this, which looks like a coarser sensor.
    pub min_step_px: i32,
}

impl Default for HumanProfile {
    fn default() -> Self {
        Self {
            curve: 1.0,
            jitter_px: 1.0,
            overshoot_pct: 0.0,
            settle_ms: 0,
            ease: Ease::Jerk,
            min_step_px: 1,
        }
    }
}

impl HumanProfile {
    /// The DTO is camelCase because the UI sends it; the maths speaks the SDK's
    /// shape, so this is the one place the two meet.
    fn motion(&self) -> gdb_motion::HumanProfile {
        gdb_motion::HumanProfile {
            curve: self.curve,
            jitter_px: self.jitter_px,
            overshoot_pct: self.overshoot_pct,
            settle_ms: self.settle_ms,
            ease: self.ease.motion(),
            min_step_px: self.min_step_px,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum Ease {
    #[default]
    Jerk,
    EaseInOut,
    Linear,
}

impl Ease {
    pub fn label(self) -> &'static str {
        match self {
            Self::Jerk => "jerk（人體加減速）",
            Self::EaseInOut => "ease-in-out",
            Self::Linear => "等速",
        }
    }
    fn motion(self) -> gdb_motion::Ease {
        match self {
            Self::Jerk => gdb_motion::Ease::Jerk,
            Self::EaseInOut => gdb_motion::Ease::EaseInOut,
            Self::Linear => gdb_motion::Ease::Linear,
        }
    }
}

/// One mouse command plus the movement profile it should use.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MouseRequest {
    pub action: MouseAction,
    #[serde(default)]
    pub profile: HumanProfile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MouseAction {
    MoveTo {
        x: i32,
        y: i32,
        duration_ms: u32,
    },
    MoveBy {
        x: i32,
        y: i32,
        duration_ms: u32,
    },
    Drag {
        button: u8,
        x: i32,
        y: i32,
        duration_ms: u32,
    },
    Click {
        button: u8,
        count: u8,
    },
    Button {
        button: u8,
        down: bool,
    },
    Wheel {
        vertical: i32,
        horizontal: i32,
    },
    Raw {
        x: i32,
        y: i32,
    },
}

/// Adapts a caller's `rand` generator to the SDK's fixed-contract RNG.
struct Unit<'a, R>(&'a mut R);

impl<R: Rng> gdb_motion::Rng for Unit<'_, R> {
    fn unit(&mut self) -> f64 {
        self.0.random_range(0.0..1.0)
    }
}

/// Nodes the path the device will be driven along; `duration_ms = 0` picks a
/// natural duration from the distance.
pub fn trajectory_with<R: Rng>(
    start: (i32, i32),
    target: (i32, i32),
    screen: Screen,
    duration_ms: u32,
    profile: &HumanProfile,
    rng: &mut R,
) -> Result<Vec<Node>> {
    let motion_screen = gdb_motion::Screen {
        width: screen.width,
        height: screen.height,
    };
    gdb_motion::trajectory_with(
        start,
        target,
        motion_screen,
        duration_ms,
        &profile.motion(),
        &mut Unit(rng),
    )
    .map_err(|_| anyhow::anyhow!("顯示器尺寸無效"))
}

fn set_button(session: &mut Session, button: u8, down: bool, cancel: &Cancellation) -> Result<()> {
    ensure!((1..=5).contains(&button), "按鍵編號須為 1–5");
    session.set_button(button, down, cancel)
}

struct ReleaseGuard<'a> {
    session: &'a mut Session,
    armed: bool,
}

impl Drop for ReleaseGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.session.reset(&Cancellation::default());
        }
    }
}

fn move_to(
    session: &mut Session,
    target: (i32, i32),
    duration: u32,
    profile: &HumanProfile,
    cancel: &Cancellation,
) -> Result<()> {
    let (screen, start) = platform::screen_and_cursor()?;
    ensure!(
        start.0 >= 0 && start.0 < screen.width && start.1 >= 0 && start.1 < screen.height,
        "擬人化移動前，請將游標放在主顯示器內"
    );
    let nodes = trajectory_with(start, target, screen, duration, profile, &mut rand::rng())?;
    move_along_trajectory(session, &nodes, screen, cancel, platform::screen_and_cursor)
}

/// How long past the last node the loop keeps retrying. The SDK's absolute move
/// (`gdb::motion`) gives up after the same 500 ms, so the same target must not
/// fail earlier here than it does through `gdb::Device::move_to`. The SDK inlines
/// the budget instead of exporting it; this constant has to move with it.
const ARRIVAL_SLACK: Duration = Duration::from_millis(500);

// Trajectory nodes are host targets. Only relative deltas cross the USB channel.
// Feedback compensates for Windows sensitivity/acceleration and delayed delivery.
//
// One caveat on cancellation: the checks run between device calls, so a move
// stops within one step. A single blocked SDK call is not interruptible and
// returns after its 1000 ms timeout at the latest.
fn move_along_trajectory(
    session: &mut Session,
    nodes: &[Node],
    screen: Screen,
    cancel: &Cancellation,
    mut read_position: impl FnMut() -> Result<(Screen, (i32, i32))>,
) -> Result<()> {
    ensure!(!nodes.is_empty(), "移動路徑不得為空");
    let started = Instant::now();
    let deadline = started + Duration::from_millis(nodes.last().unwrap().at_ms) + ARRIVAL_SLACK;
    let mut tracker = RelativeTracker::default();
    let mut index = 0;
    while Instant::now() < deadline {
        cancel.check()?;
        index = current_node(nodes, index, started.elapsed().as_millis() as u64);
        let node = &nodes[index];
        session.wait_until(started + Duration::from_millis(node.at_ms), cancel)?;
        let (actual_screen, position) = read_position()?;
        ensure!(
            actual_screen.width == screen.width && actual_screen.height == screen.height,
            "移動期間顯示器尺寸改變，請重新定位"
        );
        match tracker.step(position, (node.x, node.y), Instant::now()) {
            Step::Move(dx, dy) => {
                session.mouse_move(dx, dy, cancel)?;
                session.finish(cancel)?;
            }
            Step::Wait => session.wait_until(Instant::now() + TICK, cancel)?,
            Step::Arrived => {
                if index + 1 == nodes.len() {
                    return session.finish(cancel);
                }
                index += 1;
            }
        }
    }
    anyhow::bail!("相對滑鼠未在期限內抵達目標")
}

pub fn execute(
    session: &mut Session,
    action: &MouseAction,
    profile: &HumanProfile,
    cancel: &Cancellation,
) -> Result<Status> {
    let before = session.status(cancel)?;
    match *action {
        MouseAction::MoveTo { x, y, duration_ms } => {
            move_to(session, (x, y), duration_ms, profile, cancel)?
        }
        MouseAction::MoveBy { x, y, duration_ms } => {
            let (_, start) = platform::screen_and_cursor()?;
            move_to(
                session,
                (start.0.saturating_add(x), start.1.saturating_add(y)),
                duration_ms,
                profile,
                cancel,
            )?;
        }
        MouseAction::Drag {
            button,
            x,
            y,
            duration_ms,
        } => {
            let mut guard = ReleaseGuard {
                session,
                armed: true,
            };
            set_button(guard.session, button, true, cancel)?;
            move_to(guard.session, (x, y), duration_ms, profile, cancel)?;
            set_button(guard.session, button, false, cancel)?;
            guard.armed = false;
        }
        MouseAction::Click { button, count } => {
            ensure!(count == 1 || count == 2, "點擊次數須為 1 或 2");
            let mut guard = ReleaseGuard {
                session,
                armed: true,
            };
            for index in 0..count {
                set_button(guard.session, button, true, cancel)?;
                cancel.sleep(Duration::from_millis(55))?;
                set_button(guard.session, button, false, cancel)?;
                if index + 1 < count {
                    cancel.sleep(Duration::from_millis(150))?;
                }
            }
            guard.armed = false;
        }
        MouseAction::Button { button, down } => set_button(session, button, down, cancel)?,
        MouseAction::Raw { x, y } => {
            ensure!(
                x.unsigned_abs() <= 100_000 && y.unsigned_abs() <= 100_000,
                "單次位移限 ±100000"
            );
            for (dx, dy) in split_delta(x, y) {
                session.mouse_move(dx, dy, cancel)?;
                session.finish(cancel)?;
            }
            session.finish(cancel)?;
        }
        MouseAction::Wheel {
            vertical,
            horizontal,
        } => {
            ensure!(
                vertical.unsigned_abs() <= 100_000 && horizontal.unsigned_abs() <= 100_000,
                "單次滾輪限 ±100000"
            );
            for (chunk, _) in split_delta(vertical, 0) {
                session.wheel(chunk, 0, cancel)?;
                session.finish(cancel)?;
            }
            for (chunk, _) in split_delta(horizontal, 0) {
                session.wheel(0, chunk, cancel)?;
                session.finish(cancel)?;
            }
            session.finish(cancel)?;
        }
    }
    let after = session.status(cancel)?;
    after.check_transport_since(&before)?;
    Ok(after)
}

#[cfg(test)]
mod profile_tests {
    use super::*;
    use rand::SeedableRng;

    fn screen() -> Screen {
        Screen {
            width: 1920,
            height: 1080,
        }
    }

    /// Seeded so the two paths in a comparison see the same jitter stream and the
    /// assertion is about the profile, not about tonight's entropy.
    fn rng(seed: u64) -> rand::rngs::StdRng {
        rand::rngs::StdRng::seed_from_u64(seed)
    }

    #[test]
    fn overshoot_adds_a_correction_leg_and_settle_extends_the_end() {
        let plain = trajectory_with(
            (500, 500),
            (900, 500),
            screen(),
            300,
            &HumanProfile::default(),
            &mut rng(11),
        )
        .unwrap();
        let human = trajectory_with(
            (500, 500),
            (900, 500),
            screen(),
            300,
            &HumanProfile {
                overshoot_pct: 8.0,
                settle_ms: 120,
                ..HumanProfile::default()
            },
            &mut rng(11),
        )
        .unwrap();
        assert!(
            human.len() > plain.len(),
            "overshoot + settle must add nodes"
        );
        let last = human.last().unwrap();
        assert_eq!((last.x, last.y), (900, 500), "ends exactly on target");
        assert!(
            human.iter().any(|node| node.x > 900),
            "the path must overshoot before correcting"
        );
        assert!(
            last.at_ms > plain.last().unwrap().at_ms,
            "settle delays the end"
        );
    }

    #[test]
    fn zero_jitter_and_coarse_steps_produce_a_shorter_path() {
        let fine = trajectory_with(
            (100, 100),
            (400, 100),
            screen(),
            300,
            &HumanProfile::default(),
            &mut rng(5),
        )
        .unwrap();
        let coarse = trajectory_with(
            (100, 100),
            (400, 100),
            screen(),
            300,
            &HumanProfile {
                jitter_px: 0.0,
                min_step_px: 25,
                ..HumanProfile::default()
            },
            &mut rng(5),
        )
        .unwrap();
        assert!(coarse.len() < fine.len(), "coarse stepping drops nodes");
        assert!(coarse.len() >= 2);
    }

    #[test]
    fn linear_ease_differs_from_the_default_curve() {
        let jerk = trajectory_with(
            (0, 0),
            (600, 0),
            screen(),
            300,
            &HumanProfile::default(),
            &mut rng(3),
        )
        .unwrap();
        let linear = trajectory_with(
            (0, 0),
            (600, 0),
            screen(),
            300,
            &HumanProfile {
                ease: Ease::Linear,
                jitter_px: 0.0,
                ..HumanProfile::default()
            },
            &mut rng(3),
        )
        .unwrap();
        // The jerk curve is symmetric, so compare a quarter of the way in: there
        // ease-in still holds the pointer back, linear does not.
        let jerk_quarter = jerk[jerk.len() / 4].x;
        let linear_quarter = linear[linear.len() / 4].x;
        assert!(
            linear_quarter > jerk_quarter + 30,
            "jerk lags behind linear early on ({} vs {})",
            jerk_quarter,
            linear_quarter
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gdb::mock::MockHandle;
    use input_protocol::v3;
    use rand::SeedableRng;

    /// The SDK's in-memory device: 1920x1080, and its cursor advances by exactly
    /// the deltas that were written, clamped to the screen.
    fn mock() -> (Session, MockHandle) {
        let (device, handle) = gdb::Manager::open_mock_with_handle(gdb::DeviceOptions {
            auto_reconnect: false,
            timeout_ms: 500,
            heartbeat: false,
        })
        .unwrap();
        let mut session = Session::new(device);
        session.synchronize(&Cancellation::default()).unwrap();
        (session, handle)
    }

    /// Where the device's cursor is, derived from the frames it was sent: the
    /// mock starts at (960, 540) and moves by the MOUSE_MOVE payloads exactly.
    fn cursor_after(writes: &[v3::Packet]) -> (i32, i32) {
        writes
            .iter()
            .filter(|packet| packet.opcode == v3::MOUSE_MOVE)
            .fold((960, 540), |(x, y), packet| {
                let dx = i16::from_le_bytes([packet.payload[0], packet.payload[1]]);
                let dy = i16::from_le_bytes([packet.payload[2], packet.payload[3]]);
                (
                    (x + i32::from(dx)).clamp(0, 1919),
                    (y + i32::from(dy)).clamp(0, 1079),
                )
            })
    }

    #[test]
    fn host_trajectory_uses_relative_feedback_and_preserves_drag_buttons() {
        let cancel = Cancellation::default();
        let (mut session, handle) = mock();
        session.set_button(1, true, &cancel).unwrap();
        let nodes = [
            Node {
                at_ms: 0,
                x: 700,
                y: 300,
            },
            Node {
                at_ms: 16,
                x: 802,
                y: 302,
            },
        ];
        let screen = Screen {
            width: 1920,
            height: 1080,
        };
        move_along_trajectory(&mut session, &nodes, screen, &cancel, || {
            Ok((screen, cursor_after(&handle.writes())))
        })
        .unwrap();
        session.set_button(1, false, &cancel).unwrap();

        let writes = handle.writes();
        let position = cursor_after(&writes);
        assert!(
            (position.0 - 802).abs() <= 1 && (position.1 - 302).abs() <= 1,
            "closed loop must land on the target, got {position:?}"
        );
        assert!(writes.iter().any(|packet| packet.opcode == v3::MOUSE_MOVE));
        let buttons: Vec<_> = writes
            .iter()
            .filter(|packet| packet.opcode == v3::MOUSE_BUTTONS)
            .map(|packet| packet.payload[0])
            .collect();
        assert_eq!(buttons, [1, 0], "the drag button is held across the move");
    }

    #[test]
    fn display_change_aborts_relative_trajectory_before_sending_motion() {
        let cancel = Cancellation::default();
        let (mut session, handle) = mock();
        assert!(
            move_along_trajectory(
                &mut session,
                &[Node {
                    at_ms: 0,
                    x: 100,
                    y: 100
                }],
                Screen {
                    width: 1920,
                    height: 1080
                },
                &cancel,
                || Ok((
                    Screen {
                        width: 1280,
                        height: 720
                    },
                    (0, 0)
                ))
            )
            .is_err()
        );
        assert!(
            !handle
                .writes()
                .iter()
                .any(|packet| packet.opcode == v3::MOUSE_MOVE)
        );
    }

    #[test]
    fn delta_boundary_and_conservation() {
        let parts = split_delta(-2049, 2050);
        assert!(
            parts
                .iter()
                .all(|(x, y)| x.abs() <= 1024 && y.abs() <= 1024)
        );
        assert_eq!(parts.iter().map(|p| p.0).sum::<i32>(), -2049);
        assert_eq!(parts.iter().map(|p| p.1).sum::<i32>(), 2050);
    }

    #[test]
    fn trajectory_exact_final_and_deduplication() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(7);
        let nodes = trajectory_with(
            (10, 10),
            (1500, 700),
            Screen {
                width: 1920,
                height: 1080,
            },
            500,
            &HumanProfile::default(),
            &mut rng,
        )
        .unwrap();
        let end = nodes.last().unwrap();
        assert_eq!((end.x, end.y, end.at_ms), (1500, 700, 500));
        assert!(nodes.windows(2).all(|p| p[0].at_ms < p[1].at_ms));
    }

    #[test]
    fn late_scheduler_skips_old_points() {
        let nodes = vec![
            Node {
                at_ms: 8,
                x: 0,
                y: 0,
            },
            Node {
                at_ms: 16,
                x: 1,
                y: 1,
            },
            Node {
                at_ms: 24,
                x: 2,
                y: 2,
            },
        ];
        assert_eq!(current_node(&nodes, 0, 21), 1);
        assert_eq!(current_node(&nodes, 0, 99), 2);
    }

    #[test]
    fn cancelled_click_releases_after_acknowledged_press() {
        let token = Cancellation::default();
        let (mut session, handle) = mock();
        // Cancel as soon as the device reports the button down, so the click is
        // still holding while the cancellation arrives.
        let watcher = {
            let handle = handle.clone();
            let token = token.clone();
            std::thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(2);
                while handle.held().buttons == 0 && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(1));
                }
                token.cancel();
            })
        };
        assert!(
            execute(
                &mut session,
                &MouseAction::Click {
                    button: 1,
                    count: 2
                },
                &HumanProfile::default(),
                &token
            )
            .is_err()
        );
        watcher.join().unwrap();
        let writes = handle.writes();
        let down = writes
            .iter()
            .position(|packet| packet.opcode == v3::MOUSE_BUTTONS && packet.payload == [1])
            .unwrap();
        assert!(
            writes
                .iter()
                .skip(down + 1)
                .any(|packet| packet.opcode == v3::RELEASE_ALL),
            "the release guard must run after an acknowledged press"
        );
    }
}
