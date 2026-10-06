//! Host-side feedback for relative HID motion; never emits absolute coordinates.
use std::time::{Duration, Instant};

pub const TICK: Duration = Duration::from_millis(8);

pub enum Step {
    Arrived,
    Wait,
    Move(i32, i32),
}

#[derive(Clone, Copy)]
struct PendingMotion {
    sent_at: Instant,
    previous: (i32, i32),
    delta: (i32, i32),
}

pub struct RelativeTracker {
    gain: [f64; 2],
    pending: Option<PendingMotion>,
}

impl Default for RelativeTracker {
    fn default() -> Self {
        Self {
            gain: [1.0; 2],
            pending: None,
        }
    }
}

impl RelativeTracker {
    pub fn step(&mut self, position: (i32, i32), target: (i32, i32), now: Instant) -> Step {
        if let Some(PendingMotion {
            sent_at,
            previous,
            delta,
        }) = self.pending
        {
            // Do not integrate the same stale feedback on every tick. Allow USB
            // delivery to become visible before issuing the next correction.
            if now.duration_since(sent_at) < TICK {
                return Step::Wait;
            }
            if position == previous && now.duration_since(sent_at) < Duration::from_millis(32) {
                return Step::Wait;
            }
            for (axis, moved, sent) in [
                (0, position.0 - previous.0, delta.0),
                (1, position.1 - previous.1, delta.1),
            ] {
                if sent != 0 && moved != 0 && moved.signum() == sent.signum() {
                    // Track effective host sensitivity, including acceleration.
                    let measured = (f64::from(moved) / f64::from(sent)).clamp(0.25, 32.0);
                    self.gain[axis] = measured.max(self.gain[axis] * 0.75);
                }
            }
            self.pending = None;
        }
        let error = [target.0 - position.0, target.1 - position.1];
        if error.iter().all(|e| e.abs() <= 1) {
            return Step::Arrived;
        }
        let mut delta = [0; 2];
        for axis in 0..2 {
            if error[axis].abs() > 1 {
                // Near the endpoint use the full measured correction. Large moves
                // retain damping; integer minimum steps handle low host speeds.
                let damping = if error[axis].abs() <= 8 { 1.0 } else { 0.75 };
                let step = (f64::from(error[axis]) * damping / self.gain[axis]).round() as i32;
                delta[axis] = if step == 0 {
                    error[axis].signum()
                } else {
                    step.clamp(-512, 512)
                };
            }
        }
        self.pending = Some(PendingMotion {
            sent_at: now,
            previous: position,
            delta: (delta[0], delta[1]),
        });
        Step::Move(delta[0], delta[1])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converges_from_unknown_position_with_different_host_speeds() {
        for gain in [0.5, 1.0, 2.0, 4.0] {
            let mut tracker = RelativeTracker::default();
            let mut position = (-300, 700); // can start on another monitor
            let target = (1600, 200);
            let start = Instant::now();
            let mut arrived = false;
            for n in 0..38 {
                match tracker.step(position, target, start + TICK * n) {
                    Step::Move(dx, dy) => {
                        assert!(dx.abs() <= 512 && dy.abs() <= 512);
                        position.0 += (f64::from(dx) * gain).round() as i32;
                        position.1 += (f64::from(dy) * gain).round() as i32;
                    }
                    Step::Arrived => {
                        arrived = true;
                        break;
                    }
                    Step::Wait => {}
                }
            }
            assert!(arrived, "gain={gain}, position={position:?}");
        }
    }

    #[test]
    fn waits_for_usb_feedback_instead_of_resending_every_tick() {
        let mut tracker = RelativeTracker::default();
        let now = Instant::now();
        assert!(matches!(
            tracker.step((0, 0), (100, 0), now),
            Step::Move(..)
        ));
        for n in 1..4 {
            assert!(matches!(
                tracker.step((0, 0), (100, 0), now + TICK * n),
                Step::Wait
            ));
        }
        assert!(matches!(
            tracker.step((100, 0), (100, 0), now + TICK * 4),
            Step::Arrived
        ));
    }

    #[test]
    fn latest_target_can_reverse_direction_without_old_trajectory() {
        let mut tracker = RelativeTracker::default();
        let now = Instant::now();
        assert!(matches!(
            tracker.step((100, 100), (500, 100), now),
            Step::Move(300, 0)
        ));
        assert!(
            matches!(tracker.step((400, 100), (0, 100), now + TICK), Step::Move(dx, 0) if dx < 0)
        );
    }
    #[test]
    fn recovers_after_a_dropped_report_and_screen_edge_clamping() {
        let mut tracker = RelativeTracker::default();
        let start = Instant::now();
        let mut position = (1900, 1000);
        let target = (801, 301);
        let mut dropped = false;
        for n in 0..38 {
            match tracker.step(position, target, start + TICK * n) {
                Step::Move(dx, dy) => {
                    if !dropped {
                        dropped = true;
                        continue;
                    }
                    // Fast host sensitivity can hit an edge on the initial step.
                    position.0 = (position.0 + dx * 4).clamp(0, 1919);
                    position.1 = (position.1 + dy * 4).clamp(0, 1079);
                }
                Step::Arrived => return,
                Step::Wait => {}
            }
        }
        panic!("did not converge: {position:?}");
    }
    #[test]
    fn converges_when_acceleration_changes_between_large_and_small_steps() {
        let mut tracker = RelativeTracker::default();
        let start = Instant::now();
        let mut position = (0, 0);
        let target = (1203, 507);
        for n in 0..38 {
            match tracker.step(position, target, start + TICK * n) {
                Step::Move(dx, dy) => {
                    let gain = if dx.abs().max(dy.abs()) > 10 { 4 } else { 1 };
                    position.0 = (position.0 + dx * gain).clamp(0, 1919);
                    position.1 = (position.1 + dy * gain).clamp(0, 1079);
                }
                Step::Arrived => return,
                Step::Wait => {}
            }
        }
        panic!("did not converge under acceleration: {position:?}");
    }

    #[test]
    fn corrects_a_two_pixel_offset_instead_of_reporting_arrival() {
        let mut tracker = RelativeTracker::default();
        let now = Instant::now();
        assert!(matches!(
            tracker.step((100, 100), (102, 98), now),
            Step::Move(2, -2)
        ));
        assert!(matches!(
            tracker.step((102, 98), (102, 98), now + TICK),
            Step::Arrived
        ));
    }

    #[test]
    fn unreachable_pixel_does_not_falsely_complete_or_exceed_motion_bounds() {
        let mut tracker = RelativeTracker::default();
        let now = Instant::now();
        let mut position = (0, 0);
        // A host moving four pixels for even the smallest HID count cannot
        // reach a target two pixels between these positions within 1px.
        for n in 0..38 {
            match tracker.step(position, (2, 0), now + TICK * n) {
                Step::Move(x, y) => {
                    assert!(x.abs() <= 512 && y.abs() <= 512);
                    position.0 += x * 4;
                }
                Step::Arrived => panic!("unreachable target incorrectly completed"),
                Step::Wait => {}
            }
        }
    }
}
