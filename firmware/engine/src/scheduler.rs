//! Fixed 32-slot output queue. Jobs copy the session snapshot that existed when
//! they were queued, so a peer's short tap or motion survives that peer's
//! release.

use crate::config::{MOTION_LIMIT, OUTPUT_DEPTH, STALE_MS};
use crate::state::{InputState, Sources, merge_sources};
use crate::wire;

pub const KEYBOARD: u8 = 1;
pub const CONSUMER: u8 = 2;
pub const RELATIVE: u8 = 4;
/// `kind` of a queued relative-motion job; only these merge with a later motion.
const KIND_MOTION: u8 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputReport {
    pub id: u8,
    /// Bytes used in `data`; 29 for NKRO, 1 for Consumer, 5 for the mouse.
    pub length: u8,
    pub data: [u8; 29],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Completion {
    pub session: u32,
    pub sequence: u32,
}

#[derive(Clone, Copy, Debug, Default)]
struct Job {
    state: InputState,
    sources: Sources,
    x: i32,
    y: i32,
    wheel: i32,
    hwheel: i32,
    sequence: u32,
    session: u32,
    queued_at: u32,
    flags: u8,
    kind: u8,
}

#[derive(Clone, Debug, Default)]
pub struct OutputScheduler {
    jobs: [Job; OUTPUT_DEPTH],
    desired: InputState,
    completed: InputState,
    head: u8,
    count: u8,
    fault: u8,
    last_send_at: u32,
    generation: u32,
    inflight_generation: u32,
    in_flight: bool,
    sent_x: i8,
    sent_y: i8,
    sent_wheel: i8,
    sent_hwheel: i8,
}

impl OutputScheduler {
    pub fn pending(&self) -> u8 {
        self.count
    }

    pub fn pending_for(&self, session: u32) -> u8 {
        (0..self.count)
            .filter(|i| self.job(*i).session == session)
            .count() as u8
    }

    pub fn idle(&self) -> bool {
        self.count == 0 && !self.in_flight
    }

    pub fn desired(&self) -> &InputState {
        &self.desired
    }

    pub fn completed(&self) -> &InputState {
        &self.completed
    }

    fn job(&self, offset: u8) -> &Job {
        &self.jobs[(self.head as usize + offset as usize) % OUTPUT_DEPTH]
    }

    fn job_mut(&mut self, offset: u8) -> &mut Job {
        let index = (self.head as usize + offset as usize) % OUTPUT_DEPTH;
        &mut self.jobs[index]
    }

    fn room(&mut self) -> bool {
        if self.count < OUTPUT_DEPTH as u8 {
            return true;
        }
        self.fault = 8;
        false
    }

    fn add(&mut self, now: u32) {
        let index = (self.head as usize + self.count as usize) % OUTPUT_DEPTH;
        self.jobs[index].queued_at = now;
        self.count += 1;
    }

    /// Queues a state change. `flags` records only the report types that
    /// actually changed since the last desired state.
    // The argument list mirrors the C++ signature one-for-one so the two
    // implementations stay comparable; a parameter struct would hide that.
    #[allow(clippy::too_many_arguments)]
    pub fn state(
        &mut self,
        next: InputState,
        sources: &Sources,
        now: u32,
        seq: u32,
        force: bool,
        session: u32,
    ) -> bool {
        if !self.room() {
            return false;
        }
        let desired = self.desired;
        let mut job = Job {
            state: next,
            sources: *sources,
            sequence: seq,
            session,
            ..Job::default()
        };
        if force || next.modifiers != desired.modifiers || next.keys != desired.keys {
            job.flags |= KEYBOARD;
        }
        if force || next.consumer != desired.consumer {
            job.flags |= CONSUMER;
        }
        // One relative mouse owns movement, all five buttons and both wheels.
        if force || next.buttons != desired.buttons {
            job.flags |= RELATIVE;
        }
        let index = (self.head as usize + self.count as usize) % OUTPUT_DEPTH;
        self.jobs[index] = job;
        self.add(now);
        self.desired = next;
        true
    }

    /// Queues relative motion. Adjacent pending motion of the same owner merges
    /// into the last job; the in-flight head never does.
    #[allow(clippy::too_many_arguments)]
    pub fn motion(
        &mut self,
        x: i32,
        y: i32,
        wheel: i32,
        hwheel: i32,
        sources: &Sources,
        now: u32,
        seq: u32,
        session: u32,
    ) -> bool {
        if self.count > 1 {
            let offset = self.count - 1;
            let last = self.job(offset);
            if last.kind == KIND_MOTION
                && last.session == session
                && last.state.buttons == self.desired.buttons
            {
                let nx = last.x as i64 + x as i64;
                let ny = last.y as i64 + y as i64;
                let nw = last.wheel as i64 + wheel as i64;
                let nh = last.hwheel as i64 + hwheel as i64;
                let limit = MOTION_LIMIT as i64;
                if [nx, ny, nw, nh].iter().any(|v| *v < -limit || *v > limit) {
                    self.fault = 8;
                    return false;
                }
                let last = self.job_mut(offset);
                last.x = nx as i32;
                last.y = ny as i32;
                last.wheel = nw as i32;
                last.hwheel = nh as i32;
                last.sequence = seq;
                return true;
            }
        }
        if !self.room() {
            return false;
        }
        let index = (self.head as usize + self.count as usize) % OUTPUT_DEPTH;
        self.jobs[index] = Job {
            state: self.desired,
            sources: *sources,
            x,
            y,
            wheel,
            hwheel,
            sequence: seq,
            session,
            kind: KIND_MOTION,
            flags: RELATIVE,
            queued_at: 0,
        };
        self.add(now);
        true
    }

    /// Requests completion up to and including this sequence. When the owner
    /// already has the last queued job, its sequence is reused instead of
    /// consuming another slot.
    pub fn barrier(&mut self, sources: &Sources, now: u32, seq: u32, session: u32) -> bool {
        if self.count != 0 {
            let offset = self.count - 1;
            if self.job(offset).session == session {
                self.job_mut(offset).sequence = seq;
                return true;
            }
        }
        if !self.room() {
            return false;
        }
        let index = (self.head as usize + self.count as usize) % OUTPUT_DEPTH;
        self.jobs[index] = Job {
            state: self.desired,
            sources: *sources,
            sequence: seq,
            session,
            ..Job::default()
        };
        self.add(now);
        true
    }

    /// Global fault: discard everything and emit one all-clear state.
    pub fn release(&mut self, now: u32) {
        self.generation = self.generation.wrapping_add(1);
        self.head = 0;
        self.count = 0;
        self.desired = InputState::default();
        self.jobs[0] = Job {
            state: self.desired,
            flags: KEYBOARD | CONSUMER | RELATIVE,
            queued_at: now,
            ..Job::default()
        };
        self.count = 1;
    }

    /// Removes one session's queued work while rebuilding every surviving job's
    /// state from its captured sources, preserving short taps, motion and
    /// barriers of the other owners.
    pub fn release_session(
        &mut self,
        session: u32,
        seq: u32,
        slot: usize,
        sources: &Sources,
        now: u32,
    ) -> bool {
        let preserve_inflight = self.in_flight && self.count != 0 && self.job(0).session != session;
        if !preserve_inflight {
            // Ignore the pending completion only when its owner is cancelled.
            self.generation = self.generation.wrapping_add(1);
        }
        let old_count = self.count;
        let mut kept = 0u8;
        let mut previous = self.completed;
        for i in 0..old_count {
            let mut job = *self.job(i);
            if job.session == session {
                continue;
            }
            let original_flags = job.flags;
            job.sources[slot] = InputState::default();
            job.state = merge_sources(&job.sources);
            job.flags = 0;
            if kept == 0
                || job.state.modifiers != previous.modifiers
                || job.state.keys != previous.keys
            {
                job.flags |= KEYBOARD;
            }
            if kept == 0 || job.state.consumer != previous.consumer {
                job.flags |= CONSUMER;
            }
            if kept == 0 || job.kind == KIND_MOTION || job.state.buttons != previous.buttons {
                job.flags |= RELATIVE;
            }
            if i == 0 && preserve_inflight {
                // Its pending completion must subtract motion exactly once.
                job.flags = original_flags;
            }
            let index = (self.head as usize + kept as usize) % OUTPUT_DEPTH;
            self.jobs[index] = job;
            previous = job.state;
            kept += 1;
        }
        self.count = kept;
        self.desired = previous;
        self.state(merge_sources(sources), sources, now, seq, true, session)
    }

    /// Applies one USB completion (report id) to the head job.
    pub fn complete(&mut self, done: u8) {
        if done == 0 || !self.in_flight {
            return;
        }
        self.in_flight = false;
        if self.inflight_generation != self.generation || self.count == 0 {
            return;
        }
        let job = &mut self.jobs[self.head as usize];
        if done == wire::KEYBOARD_REPORT_ID {
            job.flags &= !KEYBOARD;
        } else if done == wire::CONSUMER_REPORT_ID {
            job.flags &= !CONSUMER;
        } else if done == wire::MOUSE_REPORT_ID {
            job.x -= self.sent_x as i32;
            job.y -= self.sent_y as i32;
            job.wheel -= self.sent_wheel as i32;
            job.hwheel -= self.sent_hwheel as i32;
            if job.x == 0 && job.y == 0 && job.wheel == 0 && job.hwheel == 0 {
                job.flags &= !RELATIVE;
            }
        }
    }

    pub fn stale(&self, now: u32) -> bool {
        self.count != 0 && now.wrapping_sub(self.job(0).queued_at) > STALE_MS
    }

    pub fn disconnect(&mut self) {
        self.in_flight = false;
    }

    pub fn rejected(&mut self) {
        self.in_flight = false;
    }

    pub fn take_fault(&mut self) -> u8 {
        let code = self.fault;
        self.fault = 0;
        code
    }

    /// Pops the head job once every report type it carries has been completed.
    pub fn pop_completed(&mut self) -> Option<Completion> {
        if self.in_flight || self.count == 0 || self.job(0).flags != 0 {
            return None;
        }
        let job = *self.job(0);
        self.completed = job.state;
        self.head = ((self.head as usize + 1) % OUTPUT_DEPTH) as u8;
        self.count -= 1;
        Some(Completion {
            session: job.session,
            sequence: job.sequence,
        })
    }

    /// Builds the next HID report. One report type per call, at most one report
    /// per millisecond, and only while nothing is in flight.
    pub fn prepare(&mut self, now: u32, ready: bool, report: &mut InputReport) -> bool {
        if self.in_flight || self.count == 0 || !ready || now.wrapping_sub(self.last_send_at) < 1 {
            return false;
        }
        let job = *self.job(0);
        *report = InputReport::default();
        if job.flags & KEYBOARD != 0 {
            report.id = wire::KEYBOARD_REPORT_ID;
            report.length = 29;
            report.data[0] = job.state.modifiers;
            report.data[1..29].copy_from_slice(&job.state.keys);
        } else if job.flags & CONSUMER != 0 {
            report.id = wire::CONSUMER_REPORT_ID;
            report.length = 1;
            report.data[0] = job.state.consumer;
        } else if job.flags & RELATIVE != 0 {
            report.id = wire::MOUSE_REPORT_ID;
            report.length = 5;
            report.data[0] = job.state.buttons;
            self.sent_x = clamp(job.x);
            self.sent_y = clamp(job.y);
            self.sent_wheel = clamp(job.wheel);
            self.sent_hwheel = clamp(job.hwheel);
            report.data[1] = self.sent_x as u8;
            report.data[2] = self.sent_y as u8;
            report.data[3] = self.sent_wheel as u8;
            report.data[4] = self.sent_hwheel as u8;
        }
        self.last_send_at = now;
        self.in_flight = true;
        self.inflight_generation = self.generation;
        true
    }
}

/// USB relative axes are 8-bit even though the wire accepts ±1024.
fn clamp(n: i32) -> i8 {
    if n < -127 {
        -127
    } else if n > 127 {
        127
    } else {
        n as i8
    }
}
