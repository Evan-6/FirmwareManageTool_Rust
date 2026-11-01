//! Motion planning and execution: absolute moves are host-side feedback loops,
//! relative moves are paced chunks. Cancelling or retargeting works mid-flight.

use crate::device::Device;
use crate::error::{Error, ErrorCode, Result};
use crate::platform;
use gdb_motion::{HumanProfile, Node, Screen as ScreenSize, SplitMix64, trajectory_with};
use input_protocol::pointer::{RelativeTracker, Step};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Default)]
pub struct MotionOptions {
    /// 0 = pick a natural duration from the distance.
    pub duration_ms: u32,
    pub profile: HumanProfile,
    /// Which rectangle an absolute move is anchored to. Relative moves ignore
    /// it; the default is the primary monitor, so nothing changes for callers
    /// that do not ask for another screen.
    pub screen: platform::Screen,
}

impl MotionOptions {
    pub fn linear(duration_ms: u32) -> Self {
        Self {
            duration_ms,
            profile: HumanProfile::linear(),
            screen: platform::Screen::Primary,
        }
    }

    /// Aims the absolute move at another monitor or at a window.
    pub fn with_screen(mut self, screen: platform::Screen) -> Self {
        self.screen = screen;
        self
    }
}

#[derive(Default)]
pub(crate) struct MovementShared {
    pub cancel: AtomicBool,
    pub target: Mutex<Option<(i32, i32)>>,
}

impl MovementShared {
    pub fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    pub fn retarget(&self, x: i32, y: i32) {
        if let Ok(mut target) = self.target.lock() {
            *target = Some((x, y));
        }
    }
    fn take_target(&self) -> Option<(i32, i32)> {
        self.target.lock().ok().and_then(|mut slot| slot.take())
    }
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// A running movement. Dropping it cancels the worker unless it already finished.
pub struct Movement {
    shared: Arc<MovementShared>,
    join: Option<JoinHandle<Result<()>>>,
}

impl Movement {
    pub(crate) fn spawn(
        device: Device,
        x: i32,
        y: i32,
        options: MotionOptions,
        absolute: bool,
    ) -> Self {
        let shared = Arc::new(MovementShared::default());
        let worker_shared = shared.clone();
        let join = thread::spawn(move || {
            if absolute {
                run_move_to(&device, x, y, &options, &worker_shared)
            } else {
                run_move_by(&device, x, y, &options, &worker_shared)
            }
        });
        Self {
            shared,
            join: Some(join),
        }
    }

    pub(crate) fn spawn_path(
        device: Device,
        points: Vec<(i32, i32)>,
        options: MotionOptions,
    ) -> Self {
        let shared = Arc::new(MovementShared::default());
        let worker_shared = shared.clone();
        let join = thread::spawn(move || run_path(&device, &points, &options, &worker_shared));
        Self {
            shared,
            join: Some(join),
        }
    }

    pub fn cancel(&self) {
        self.shared.stop();
    }

    pub fn retarget(&self, x: i32, y: i32) {
        self.shared.retarget(x, y);
    }

    pub fn is_finished(&self) -> bool {
        self.join.as_ref().is_none_or(|handle| handle.is_finished())
    }

    /// Waits at most `timeout`; on timeout the worker keeps running and the
    /// handle stays usable (cancel it or wait again).
    pub fn wait_timeout(&mut self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        while !self.is_finished() {
            if Instant::now() >= deadline {
                return Err(Error::timeout("movement did not finish in time"));
            }
            thread::sleep(Duration::from_millis(2));
        }
        match self.join.take() {
            Some(handle) => handle
                .join()
                .map_err(|_| Error::new(crate::error::ErrorCode::Io, "movement thread panicked"))?,
            None => Ok(()),
        }
    }

    pub fn wait(mut self) -> Result<()> {
        match self.join.take() {
            Some(handle) => handle
                .join()
                .map_err(|_| Error::new(crate::error::ErrorCode::Io, "movement thread panicked"))?,
            None => Ok(()),
        }
    }
}

impl Drop for Movement {
    fn drop(&mut self) {
        self.shared.stop();
    }
}

pub(crate) fn run_path(
    device: &Device,
    points: &[(i32, i32)],
    options: &MotionOptions,
    shared: &MovementShared,
) -> Result<()> {
    for point in points {
        run_move_to(device, point.0, point.1, options, shared)?;
    }
    Ok(())
}

pub(crate) fn run_move_by(
    device: &Device,
    dx: i32,
    dy: i32,
    options: &MotionOptions,
    shared: &MovementShared,
) -> Result<()> {
    let chunks = gdb_motion::split_delta(dx, dy);
    if chunks.is_empty() {
        return Ok(());
    }
    let pause = if options.duration_ms == 0 {
        Duration::from_millis(0)
    } else {
        Duration::from_millis(u64::from(options.duration_ms / chunks.len() as u32))
    };
    for (chunk, (x, y)) in chunks.iter().enumerate() {
        if shared.cancelled() {
            return Err(Error::cancelled());
        }
        device.send_mouse_move(*x as i16, *y as i16)?;
        device.barrier()?;
        if chunk + 1 != chunks.len() && !pause.is_zero() {
            thread::sleep(pause);
        }
    }
    Ok(())
}

/// Where the OS is asked for the pointer and the monitor layout. The mock device
/// answers from its simulated pointer, and tests replace it to start a move on a
/// monitor the mock does not really have.
pub(crate) type TargetProbe = fn(&Device, platform::Screen) -> Result<platform::Target>;

/// How long a movement may go without closing the gap before it is a timeout.
///
/// A long jump paired with a short duration is not a failure: the trajectory
/// ends sooner than the device can report the steps, and every step that shrinks
/// the gap pushes this window forward again, so only a movement that has stopped
/// making progress fails.
const STALL: Duration = Duration::from_millis(400);

fn probe(device: &Device, screen: platform::Screen) -> Result<platform::Target> {
    if device.is_mock() {
        return platform::Target::mock(screen, device.cursor());
    }
    platform::screen_and_cursor_for(screen)
}

/// The box the trajectory is planned in, in its own coordinates.
///
/// It is the target rectangle widened to reach the pointer, so a move that starts
/// on another monitor still gets a straight path instead of starting on an edge.
struct Plan {
    screen: ScreenSize,
    origin: (i32, i32),
}

impl Plan {
    fn around(target: &platform::Target) -> Self {
        let bounds = target.rect.union_point(target.cursor);
        Self {
            screen: ScreenSize {
                width: bounds.width(),
                height: bounds.height(),
            },
            origin: bounds.origin(),
        }
    }

    fn local(&self, point: (i32, i32)) -> (i32, i32) {
        (point.0 - self.origin.0, point.1 - self.origin.1)
    }
}

pub(crate) fn run_move_to(
    device: &Device,
    x: i32,
    y: i32,
    options: &MotionOptions,
    shared: &MovementShared,
) -> Result<()> {
    run_move_to_with(device, probe, x, y, options, shared)
}

/// The absolute move with the pointer source injected.
pub(crate) fn run_move_to_with(
    device: &Device,
    probe: TargetProbe,
    x: i32,
    y: i32,
    options: &MotionOptions,
    shared: &MovementShared,
) -> Result<()> {
    platform::enable_dpi_awareness();
    let screen = options.screen;
    let target = probe(device, screen)?;
    let mut aim = aim_at(&target, (x, y), screen)?;
    if aim != (x, y) {
        device.record_clamp((x, y), aim);
    }
    let mut plan = Plan::around(&target);
    let mut rng = SplitMix64::from_entropy();
    let mut nodes = plan_nodes(&plan, target.cursor, aim, options, &mut rng)?;
    let mut started = Instant::now();
    let mut index = 0usize;
    let mut tracker = RelativeTracker::default();
    // The trajectory duration is a lower bound, not the budget: the pointer can
    // only close the gap as fast as the device reports, so the deadline is the
    // stall window after the last improvement rather than a fixed span.
    let mut best = gap(target.cursor, aim);
    let mut deadline = started + trajectory_span(&nodes) + STALL;
    loop {
        if shared.cancelled() {
            return Err(Error::cancelled());
        }
        if let Some(next) = shared.take_target() {
            let moved = probe(device, screen)?;
            if moved.rect.width() != target.rect.width()
                || moved.rect.height() != target.rect.height()
            {
                return Err(Error::invalid_screen(format!(
                    "monitor layout changed during the movement ({}x{} -> {}x{})",
                    target.rect.width(),
                    target.rect.height(),
                    moved.rect.width(),
                    moved.rect.height()
                )));
            }
            aim = aim_at(&moved, next, screen)?;
            if aim != next {
                device.record_clamp(next, aim);
            }
            plan = Plan::around(&moved);
            nodes = plan_nodes(&plan, moved.cursor, aim, options, &mut rng)?;
            started = Instant::now();
            index = 0;
            tracker = RelativeTracker::default();
            best = gap(moved.cursor, aim);
            deadline = started + trajectory_span(&nodes) + STALL;
        }
        let Some(node) = nodes.get(index).copied() else {
            return Ok(());
        };
        let elapsed = started.elapsed();
        let node_at = Duration::from_millis(node.at_ms);
        if elapsed < node_at {
            let wait = (node_at - elapsed).min(Duration::from_millis(2));
            thread::sleep(wait);
            continue;
        }
        let cursor = probe(device, screen)?.cursor;
        let remaining = gap(cursor, aim);
        if remaining < best {
            best = remaining;
            let extended = Instant::now() + STALL;
            if extended > deadline {
                deadline = extended;
            }
        }
        let position = plan.local(cursor);
        // A device whose reports arrive late or with acceleration can put the
        // pointer past the node it was aiming at. Walking the path backwards from
        // there would fight the pointer, so the path advances to the closest node
        // still ahead of it.
        while index + 1 < nodes.len() {
            let here = (position.0, position.1);
            let next = nodes[index + 1];
            let current = nodes[index];
            if gap(here, (next.x, next.y)) <= gap(here, (current.x, current.y)) {
                index += 1;
            } else {
                break;
            }
        }
        let node = nodes[index];
        match tracker.step(position, (node.x, node.y), Instant::now()) {
            Step::Wait => thread::sleep(Duration::from_millis(1)),
            Step::Move(dx, dy) => {
                device.send_mouse_move(dx as i16, dy as i16)?;
                device.barrier()?;
            }
            Step::Arrived => {
                if index + 1 == nodes.len() {
                    device.barrier()?;
                    return Ok(());
                }
                index += 1;
            }
        }
        if Instant::now() > deadline {
            return Err(Error::timeout(format!(
                "pointer did not reach ({}, {}) targeting {screen}: off by {remaining} px after {} ms",
                aim.0,
                aim.1,
                started.elapsed().as_millis()
            )));
        }
    }
}

/// The largest axis gap between two points, which is the number of pixels the
/// pointer still has to travel.
fn gap(from: (i32, i32), to: (i32, i32)) -> i32 {
    (to.0 - from.0).abs().max((to.1 - from.1).abs())
}

fn trajectory_span(nodes: &[Node]) -> Duration {
    Duration::from_millis(nodes.last().map(|node| node.at_ms).unwrap_or(0))
}

/// Clamps the caller's point into the target rectangle. A point no monitor covers
/// is refused: the pointer cannot go there at all, and the message names the
/// target and the desktop the host could see.
fn aim_at(
    target: &platform::Target,
    point: (i32, i32),
    screen: platform::Screen,
) -> Result<(i32, i32)> {
    if !target.reaches(point) {
        return Err(Error::new(
            ErrorCode::Unsupported,
            platform::outside_message(screen, point, target.bounds()),
        ));
    }
    Ok(target.rect.clamp(point))
}

fn plan_nodes(
    plan: &Plan,
    cursor: (i32, i32),
    aim: (i32, i32),
    options: &MotionOptions,
    rng: &mut SplitMix64,
) -> Result<Vec<Node>> {
    trajectory_with(
        plan.local(cursor),
        plan.local(aim),
        plan.screen,
        options.duration_ms,
        &options.profile,
        rng,
    )
    .map_err(|error| Error::invalid(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DeviceOptions;
    use crate::platform::Rect;

    /// Two 960x540 monitors side by side. The pointer is the mock's own, so the
    /// loop really converges; only the rectangles are pretend.
    fn two_screens(device: &Device, _screen: platform::Screen) -> Result<platform::Target> {
        let left = Rect {
            left: 0,
            top: 0,
            right: 960,
            bottom: 540,
        };
        let right = Rect {
            left: 960,
            top: 0,
            right: 1920,
            bottom: 540,
        };
        Ok(platform::Target::new(
            right,
            device.cursor(),
            vec![left, right],
        ))
    }

    /// One 1920x540 screen: the right half of the pointer's world does not exist.
    fn single_screen(device: &Device, _screen: platform::Screen) -> Result<platform::Target> {
        let only = Rect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 540,
        };
        Ok(platform::Target::new(only, device.cursor(), vec![only]))
    }

    fn mock() -> Device {
        crate::manager::Manager::open_mock_with_handle(DeviceOptions::default())
            .expect("mock device")
            .0
    }

    /// The monitor that holds the point the caller aimed at: what a
    /// `Screen::Containing` anchor must reach.
    fn containing_pointer(device: &Device, screen: platform::Screen) -> Result<platform::Target> {
        let left = Rect {
            left: 0,
            top: 0,
            right: 960,
            bottom: 540,
        };
        let right = Rect {
            left: 960,
            top: 0,
            right: 1920,
            bottom: 540,
        };
        let rect = match screen {
            platform::Screen::Containing(x, _) if x >= right.left => right,
            _ => left,
        };
        Ok(platform::Target::new(
            rect,
            device.cursor(),
            vec![left, right],
        ))
    }

    /// A pointer that moves 200 px per report and takes 60 ms to answer: a long
    /// jump paced far below the trajectory's own duration.
    static LAGGING_X: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

    fn lagging_pointer(_device: &Device, _screen: platform::Screen) -> Result<platform::Target> {
        thread::sleep(Duration::from_millis(60));
        let moved = (LAGGING_X.load(Ordering::Relaxed) + 200).min(4000);
        LAGGING_X.store(moved, Ordering::Relaxed);
        let rect = Rect {
            left: 0,
            top: 0,
            right: 4096,
            bottom: 1080,
        };
        Ok(platform::Target::new(rect, (moved, 500), vec![rect]))
    }

    /// A pointer that never moves: whatever the SDK asks for, the report says the
    /// same position again.
    fn stuck_pointer(_device: &Device, _screen: platform::Screen) -> Result<platform::Target> {
        let rect = Rect {
            left: 0,
            top: 0,
            right: 2400,
            bottom: 1080,
        };
        Ok(platform::Target::new(rect, (100, 100), vec![rect]))
    }

    #[test]
    fn a_containing_anchor_reaches_the_monitor_that_holds_the_point() {
        let device = mock();
        let options =
            MotionOptions::linear(80).with_screen(platform::Screen::Containing(1400, 300));
        let shared = MovementShared::default();
        run_move_to_with(&device, containing_pointer, 1400, 300, &options, &shared)
            .expect("containing move");
        let landed = device.cursor();
        assert!(
            (landed.0 - 1400).abs() <= 1 && (landed.1 - 300).abs() <= 1,
            "the anchor must follow the point to the second screen, not {landed:?}"
        );
    }

    #[test]
    fn a_move_that_keeps_closing_the_gap_outlives_its_short_duration() {
        LAGGING_X.store(0, Ordering::Relaxed);
        let device = mock();
        let options = MotionOptions::linear(40);
        let shared = MovementShared::default();
        // 4000 px at 200 px per 60 ms report is 1.2 s, three times the deadline a
        // fixed `trajectory + 500 ms` would have given.
        run_move_to_with(&device, lagging_pointer, 4000, 500, &options, &shared)
            .expect("a slow but progressing pointer must not time out");
        assert_eq!(LAGGING_X.load(Ordering::Relaxed), 4000);
    }

    #[test]
    fn a_stalled_move_reports_the_remaining_distance_and_the_time_spent() {
        let device = mock();
        let options = MotionOptions::linear(20);
        let shared = MovementShared::default();
        let failure = run_move_to_with(&device, stuck_pointer, 1000, 500, &options, &shared)
            .expect_err("a pointer that never moves cannot arrive");
        assert_eq!(failure.code(), ErrorCode::Timeout);
        assert!(
            failure.message.contains("off by 900 px"),
            "the timeout must say how far off it stopped: {failure}"
        );
        assert!(
            failure.message.contains(" ms"),
            "the timeout must say how long it waited: {failure}"
        );
    }

    #[test]
    fn a_clamped_move_is_counted_in_the_metrics() {
        let device = mock();
        let shared = MovementShared::default();
        // The probe anchors on the right monitor, but (500, 300) is on the left
        // one: the move converges inside the anchor and only the counter says the
        // requested pixel was never reached.
        run_move_to_with(
            &device,
            two_screens,
            500,
            300,
            &MotionOptions::linear(60),
            &shared,
        )
        .expect("clamped move");
        let metrics = device.metrics();
        assert_eq!(metrics.clamped_total, 1);
        let clamp = metrics.last_clamp.expect("one clamp");
        assert_eq!(
            (
                clamp.requested_x,
                clamp.requested_y,
                clamp.applied_x,
                clamp.applied_y
            ),
            (500, 300, 960, 300)
        );
    }

    #[test]
    fn absolute_move_from_another_monitor_converges() {
        let device = mock();
        // The mock pointer starts in the middle of its 1920x1080 space, which is
        // on the left screen here.
        device
            .mouse()
            .move_by(-800, -400, MotionOptions::linear(0))
            .expect("move by");
        let start = device.cursor();
        assert!(
            start.0 < 960 && start.1 < 540,
            "the pointer should start on the left screen, at {start:?}"
        );

        let options = MotionOptions::linear(120).with_screen(platform::Screen::Monitor(1));
        let shared = MovementShared::default();
        run_move_to_with(&device, two_screens, 1400, 300, &options, &shared)
            .expect("cross-monitor");

        let landed = device.cursor();
        assert!(
            (landed.0 - 1400).abs() <= 1 && (landed.1 - 300).abs() <= 1,
            "the pointer should land on (1400, 300), not {landed:?}"
        );
    }

    #[test]
    fn a_point_on_no_screen_is_refused_with_the_target_and_the_union() {
        let device = mock();
        let options = MotionOptions::linear(80).with_screen(platform::Screen::Monitor(1));
        let shared = MovementShared::default();
        let failure = run_move_to_with(&device, single_screen, 2400, 300, &options, &shared)
            .expect_err("a point beside every screen");
        assert_eq!(failure.code(), ErrorCode::Unsupported);
        assert_eq!(
            failure.message,
            "absolute move to (2400, 300) targets monitor 1, but that point is on no screen: \
             the monitors cover (0, 0)-(1920, 540)"
        );
    }

    #[test]
    fn the_primary_target_still_lands_on_the_mock_screen() {
        let device = mock();
        let shared = MovementShared::default();
        let options = MotionOptions::linear(80);
        run_move_to(&device, 200, 150, &options, &shared).expect("primary move");
        let landed = device.cursor();
        assert!(
            (landed.0 - 200).abs() <= 1 && (landed.1 - 150).abs() <= 1,
            "the default target is still the primary monitor, not {landed:?}"
        );
    }
}
