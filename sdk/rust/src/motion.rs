//! Motion planning and execution: absolute moves are host-side feedback loops,
//! relative moves are paced chunks. Cancelling or retargeting works mid-flight.

use crate::device::Device;
use crate::error::{Error, Result};
use crate::platform;
use gdb_motion::{HumanProfile, Screen, SplitMix64, trajectory_with};
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
}

impl MotionOptions {
    pub fn linear(duration_ms: u32) -> Self {
        Self {
            duration_ms,
            profile: HumanProfile::linear(),
        }
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

pub(crate) fn run_move_to(
    device: &Device,
    x: i32,
    y: i32,
    options: &MotionOptions,
    shared: &MovementShared,
) -> Result<()> {
    platform::enable_dpi_awareness();
    let (screen, position) = cursor_state(device)?;
    if position.0 < 0 || position.1 < 0 || position.0 >= screen.width || position.1 >= screen.height
    {
        return Err(Error::new(
            crate::error::ErrorCode::Unsupported,
            "put the cursor on the primary monitor before an absolute move",
        ));
    }
    let mut target = clamp_target(x, y, screen);
    let mut rng = SplitMix64::from_entropy();
    let mut nodes = trajectory_with(
        position,
        target,
        screen,
        options.duration_ms,
        &options.profile,
        &mut rng,
    )
    .map_err(|error| Error::invalid(error.to_string()))?;
    let mut started = Instant::now();
    let mut index = 0usize;
    let mut tracker = RelativeTracker::default();
    let mut deadline = started
        + Duration::from_millis(nodes.last().map(|node| node.at_ms).unwrap_or(0))
        + Duration::from_millis(500);
    loop {
        if shared.cancelled() {
            return Err(Error::cancelled());
        }
        if let Some(next) = shared.take_target() {
            target = clamp_target(next.0, next.1, screen);
            let (current_screen, current) = cursor_state(device)?;
            if current_screen.width != screen.width || current_screen.height != screen.height {
                return Err(Error::new(
                    crate::error::ErrorCode::Unsupported,
                    "monitor layout changed during the movement",
                ));
            }
            nodes = trajectory_with(
                current,
                target,
                screen,
                options.duration_ms,
                &options.profile,
                &mut rng,
            )
            .map_err(|error| Error::invalid(error.to_string()))?;
            started = Instant::now();
            index = 0;
            tracker = RelativeTracker::default();
            deadline = started
                + Duration::from_millis(nodes.last().map(|node| node.at_ms).unwrap_or(0))
                + Duration::from_millis(500);
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
        let (_, position) = cursor_state(device)?;
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
            return Err(Error::timeout("pointer did not reach the target"));
        }
    }
}

/// Real cursor on hardware, simulated pointer for the mock device.
fn cursor_state(device: &Device) -> Result<(Screen, (i32, i32))> {
    if device.is_mock() {
        return Ok((
            Screen {
                width: 1920,
                height: 1080,
            },
            device.cursor(),
        ));
    }
    platform::screen_and_cursor()
}

fn clamp_target(x: i32, y: i32, screen: Screen) -> (i32, i32) {
    (
        x.clamp(0, screen.width.saturating_sub(1)),
        y.clamp(0, screen.height.saturating_sub(1)),
    )
}
