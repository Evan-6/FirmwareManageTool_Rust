//! Command dispatch, TX replies, fault coordination and boot confirmation.
//!
//! Every method takes `now_ms` from the caller. The engine never reads a clock,
//! touches USB or drives an LED; the platform layer reports what happened.

use crate::config::{LEASE_MS, SESSIONS, TX_DEPTH};
use crate::scheduler::{InputReport, OutputScheduler};
use crate::session::{Pending, Session, SessionTable};
use crate::state::{InputState, merge_sources};
use crate::wire::{self, Report};

/// Set in [`InputEngine::take_notices`] when a fault or expiry needs the LED.
pub const ERROR: u8 = 1;
/// Set when a lease expired (or an idle released session expired).
pub const FAILSAFE: u8 = 2;
/// Set when the bootloader reply completed on the bus.
pub const BOOT_CONFIRMED: u8 = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    pub rx: u16,
    pub tx: u16,
    pub hid: u16,
    pub failsafe: u16,
}

#[derive(Clone, Debug, Default)]
pub struct InputEngine {
    sessions: SessionTable,
    output: OutputScheduler,
    counters: Counters,
    tx: [Report; TX_DEPTH],
    tx_head: u8,
    tx_count: u8,
    notices: u8,
    now: u32,
    boot_generation: u32,
    rx_generation: u32,
    mounted: bool,
    overflow: bool,
    boot_requested: bool,
    boot_inflight: bool,
    boot_completed: bool,
    boot_confirmed: bool,
}

impl InputEngine {
    pub fn new() -> Self {
        Self::default()
    }

    // ---- inbound path -----------------------------------------------------

    pub fn handle(&mut self, report: &Report, now: u32) {
        self.now = now;
        let p = &report.bytes;
        let op = p[1];
        let session = wire::read32(&p[4..]);
        let seq = wire::read32(&p[8..]);
        let mut slot = self.sessions.find(session);
        if let Some(index) = slot
            && now.wrapping_sub(self.sessions.get(index).lease_at) >= LEASE_MS
        {
            self.session_fault(index, 5);
            slot = None;
        }
        if !wire::valid(report) {
            self.counters.rx = self.counters.rx.wrapping_add(1);
            if let Some(index) = slot {
                self.session_fault(index, 1);
            }
            return;
        }
        if op == wire::OPEN {
            self.open(session, seq);
            return;
        }
        let Some(slot) = slot else {
            return;
        };
        let received = self.sessions.get(slot).received;
        if seq <= received || seq != received.wrapping_add(1) {
            self.counters.rx = self.counters.rx.wrapping_add(1);
            self.session_fault(slot, if seq <= received { 9 } else { 10 });
            return;
        }
        self.sessions.get_mut(slot).received = seq;
        let data = &p[wire::HEADER..];
        if op == wire::STATUS {
            self.status(slot, seq);
            return;
        }
        if op == wire::HEARTBEAT {
            self.sessions.get_mut(slot).lease_at = self.now;
            return;
        }
        if op == wire::CLOSE {
            self.close(slot);
            return;
        }
        if self.boot_requested {
            // A successful boot request has committed to an empty input state.
            return;
        }
        if op == wire::RELEASE || op == wire::BOOTLOADER {
            self.release_or_boot(slot, op, seq);
            return;
        }
        if op == wire::BARRIER {
            if !self.sessions.get(slot).pending.is_none() {
                self.session_fault(slot, 2);
                return;
            }
            if self.queue_barrier(seq, session) {
                self.sessions.get_mut(slot).pending = Pending { op, seq };
            }
            return;
        }
        self.input(slot, op, seq, session, data);
    }

    fn open(&mut self, session: u32, seq: u32) {
        if session == 0 || seq != 1 {
            self.result(wire::OPEN, session, seq, 1);
            return;
        }
        // Never let OPEN with a colliding id reset a live client's state.
        if self.sessions.find(session).is_some()
            || self.boot_requested
            || self.boot_confirmed
            || (self.sessions.count() == 0 && !self.output.idle())
        {
            self.result(wire::OPEN, session, seq, 2);
            return;
        }
        let Some(slot) = self.sessions.allocate() else {
            self.result(wire::OPEN, session, seq, 2);
            return;
        };
        *self.sessions.get_mut(slot) = Session {
            id: session,
            received: 1,
            lease_at: self.now,
            ..Session::default()
        };
        let force = self.sessions.count() == 1;
        if self.queue_state(seq, force, session) {
            self.sessions.get_mut(slot).pending = Pending {
                op: wire::OPEN,
                seq,
            };
        }
    }

    /// Explicit teardown. The slot is free before the reply is written, and this
    /// is deliberately *not* a fault: only a lease timeout counts a failsafe.
    fn close(&mut self, slot: usize) {
        let snapshot = *self.sessions.get(slot);
        let id = snapshot.id;
        let seq = snapshot.received;
        self.discard_replies(id);
        if self.boot_requested {
            self.clear_boot();
        }
        *self.sessions.get_mut(slot) = Session::default();
        if snapshot.state.held() || self.output.pending_for(id) != 0 {
            self.cancel_owner(id, 0, slot);
        }
        self.result(wire::CLOSE, id, seq, 0);
    }

    fn release_or_boot(&mut self, slot: usize, op: u8, seq: u32) {
        let snapshot = *self.sessions.get(slot);
        let id = snapshot.id;
        if !snapshot.pending.is_none() {
            self.session_fault(slot, 2);
            return;
        }
        if op == wire::BOOTLOADER && self.sessions.count() > 1 {
            self.result(op, id, seq, 2);
            return;
        }
        // An empty owner with no queued input is already physically released, so
        // do not consume a queue slot (or disturb peer jobs) just to confirm it.
        if op == wire::RELEASE && !snapshot.state.held() && self.output.pending_for(id) == 0 {
            let session = self.sessions.get_mut(slot);
            session.released = true;
            session.completed = seq;
            session.lease_at = self.now;
            self.result(op, id, seq, 0);
            return;
        }
        if op == wire::BOOTLOADER {
            self.boot_requested = true;
        }
        let session = self.sessions.get_mut(slot);
        session.state = InputState::default();
        session.released = false;
        if self.cancel_owner(id, seq, slot) {
            self.sessions.get_mut(slot).pending = Pending { op, seq };
        }
        self.sessions.get_mut(slot).lease_at = self.now;
    }

    fn input(&mut self, slot: usize, op: u8, seq: u32, session: u32, data: &[u8]) {
        if !self.sessions.get(slot).pending.is_none() {
            // Input behind a pending control reply would make it ambiguous.
            self.session_fault(slot, 2);
            return;
        }
        self.sessions.get_mut(slot).released = false;
        let ok = if op == wire::KEY {
            let mut state = self.sessions.get(slot).state;
            let page = wire::read16(&data[0..]);
            let usage = wire::read16(&data[2..]);
            if data[4] > 1 || !state.set(page, usage, data[4] != 0) {
                self.session_fault(slot, 3);
                return;
            }
            self.sessions.get_mut(slot).state = state;
            self.queue_state(seq, false, session)
        } else if op == wire::SNAPSHOT {
            let state = InputState {
                modifiers: data[0],
                keys: data[1..29].try_into().unwrap(),
                consumer: data[29],
                buttons: data[30],
            };
            if !state.valid() {
                self.session_fault(slot, 3);
                return;
            }
            self.sessions.get_mut(slot).state = state;
            self.queue_state(seq, false, session)
        } else if op == wire::BUTTONS {
            if data[0] & !crate::state::BUTTON_BITS != 0 {
                self.session_fault(slot, 3);
                return;
            }
            self.sessions.get_mut(slot).state.buttons = data[0];
            self.queue_state(seq, false, session)
        } else if op == wire::ABSOLUTE {
            self.session_fault(slot, 3);
            return;
        } else {
            let a = wire::read16(&data[0..]) as i16 as i32;
            let b = wire::read16(&data[2..]) as i16 as i32;
            if !(-1024..=1024).contains(&a) || !(-1024..=1024).contains(&b) {
                self.session_fault(slot, 3);
                return;
            }
            if op == wire::MOVE {
                self.queue_motion(a, b, 0, 0, seq, session)
            } else {
                self.queue_motion(0, 0, a, b, seq, session)
            }
        };
        if ok {
            self.sessions.get_mut(slot).lease_at = self.now;
        }
    }

    // ---- platform path ----------------------------------------------------

    /// Folds callback faults (framing/overflow codes) and RX error counts into
    /// the engine. `code == 7` means the bus reset or unplugged.
    pub fn receive_fault(&mut self, code: u8, errors: u16, now: u32) {
        self.now = now;
        self.counters.rx = self.counters.rx.wrapping_add(errors);
        if code != 0 {
            self.fault(code);
            if code == 7 {
                self.output.disconnect();
            }
        } else if self.overflow {
            self.fault(8);
        }
    }

    /// Advances the USB output state machine and returns the report to send.
    #[allow(clippy::too_many_arguments)]
    pub fn advance(
        &mut self,
        now: u32,
        mounted: bool,
        completed: u8,
        failed: bool,
        ready: bool,
        report: &mut InputReport,
    ) -> bool {
        self.now = now;
        if !mounted {
            if self.mounted || self.sessions.count() != 0 {
                self.fault(7);
            }
            self.mounted = false;
            self.output.disconnect();
            return false;
        }
        if !self.mounted {
            self.mounted = true;
            self.output.release(now);
        }
        if failed {
            self.output.rejected();
            self.counters.hid = self.counters.hid.wrapping_add(1);
            self.fault(6);
            return false;
        }
        self.output.complete(completed);
        if self.sessions.count() != 0 && self.output.stale(now) {
            self.fault(8);
            return false;
        }
        while let Some(completion) = self.output.pop_completed() {
            if completion.sequence != 0 {
                self.sessions
                    .completed(completion.session, completion.sequence);
            }
        }
        self.output.prepare(now, ready, report)
    }

    pub fn input_rejected(&mut self, now: u32) {
        self.now = now;
        self.output.rejected();
        self.counters.hid = self.counters.hid.wrapping_add(1);
        self.fault(6);
    }

    pub fn service_control(&mut self, now: u32) {
        self.now = now;
        if self.boot_completed {
            self.boot_completed = false;
            self.boot_confirmed = true;
            self.notices |= BOOT_CONFIRMED;
        }
        if self.sessions.count() == 0 {
            return;
        }
        for slot in 0..SESSIONS {
            let snapshot = *self.sessions.get(slot);
            if snapshot.id == 0 {
                continue;
            }
            if now.wrapping_sub(snapshot.lease_at) >= LEASE_MS {
                self.session_fault(slot, 5);
                self.notices |= FAILSAFE;
                continue;
            }
            if !snapshot.pending.is_none() && snapshot.completed >= snapshot.pending.seq {
                if snapshot.pending.op == wire::RELEASE {
                    self.sessions.get_mut(slot).released = true;
                }
                self.result(snapshot.pending.op, snapshot.id, snapshot.pending.seq, 0);
                self.sessions.get_mut(slot).pending = Pending::default();
            }
            if self.output.idle() && self.sessions.get(slot).pending.is_none() {
                self.sessions.get_mut(slot).completed = self.sessions.get(slot).received;
            }
        }
    }

    // ---- TX ---------------------------------------------------------------

    pub fn vendor_reply(&self) -> Option<&Report> {
        if self.tx_count == 0 {
            None
        } else {
            Some(&self.tx[self.tx_head as usize])
        }
    }

    /// The platform accepted a reply on the wire. The boot confirmation only
    /// counts once that reply is removed from the queue.
    pub fn vendor_submitted(&mut self) {
        if self.tx_count == 0 {
            return;
        }
        let report = self.tx[self.tx_head as usize];
        if report.bytes[1] == (wire::BOOTLOADER | 128) && report.bytes[wire::HEADER] == 0 {
            self.boot_inflight = true;
        }
        self.tx_head = ((self.tx_head as usize + 1) % TX_DEPTH) as u8;
        self.tx_count -= 1;
    }

    /// Called with the generation captured when the reply was submitted, so a
    /// reset in between cannot confirm the request.
    pub fn vendor_completed(&mut self, generation: u32) {
        if generation == self.boot_generation && self.boot_requested && self.boot_inflight {
            self.boot_inflight = false;
            self.boot_completed = true;
        }
    }

    // ---- accessors --------------------------------------------------------

    pub fn take_notices(&mut self) -> u8 {
        let notices = self.notices;
        self.notices = 0;
        notices
    }

    pub fn boot_generation(&self) -> u32 {
        self.boot_generation
    }

    pub fn rx_generation(&self) -> u32 {
        self.rx_generation
    }

    pub fn session_count(&self) -> u8 {
        self.sessions.count()
    }

    pub fn session(&self, id: u32) -> Option<&Session> {
        self.sessions.find(id).map(|slot| self.sessions.get(slot))
    }

    pub fn desired_state(&self) -> &InputState {
        self.output.desired()
    }

    pub fn completed_state(&self) -> &InputState {
        self.output.completed()
    }

    pub fn counters(&self) -> &Counters {
        &self.counters
    }

    pub fn idle(&self) -> bool {
        self.output.idle()
    }

    pub fn tx_empty(&self) -> bool {
        self.tx_count == 0
    }

    pub fn boot_requested(&self) -> bool {
        self.boot_requested
    }

    pub fn boot_confirmed(&self) -> bool {
        self.boot_confirmed
    }

    // ---- internals --------------------------------------------------------

    fn reply(&mut self, op: u8, session: u32, seq: u32, data: &[u8]) {
        if self.tx_count as usize == TX_DEPTH {
            self.counters.tx = self.counters.tx.wrapping_add(1);
            self.overflow = true;
            return;
        }
        let index = (self.tx_head as usize + self.tx_count as usize) % TX_DEPTH;
        self.tx[index] = wire::reply(op, session, seq, data);
        self.tx_count += 1;
    }

    fn result(&mut self, op: u8, session: u32, seq: u32, code: u8) {
        self.reply(op | 128, session, seq, &[code]);
    }

    fn event(&mut self, code: u8, session: u32, seq: u32) {
        self.reply(wire::EVENT, session, seq, &[code]);
    }

    /// Drops only this owner's queued replies, keeping the peer's order.
    fn discard_replies(&mut self, session: u32) {
        let depth = self.tx_count;
        for _ in 0..depth {
            let report = self.tx[self.tx_head as usize];
            self.tx_head = ((self.tx_head as usize + 1) % TX_DEPTH) as u8;
            self.tx_count -= 1;
            if wire::read32(&report.bytes[4..]) != session {
                let index = (self.tx_head as usize + self.tx_count as usize) % TX_DEPTH;
                self.tx[index] = report;
                self.tx_count += 1;
            }
        }
    }

    fn clear_boot(&mut self) {
        self.boot_requested = false;
        self.boot_inflight = false;
        self.boot_completed = false;
        self.boot_confirmed = false;
        self.boot_generation = self.boot_generation.wrapping_add(1);
    }

    fn session_fault(&mut self, slot: usize, code: u8) {
        let snapshot = *self.sessions.get(slot);
        let id = snapshot.id;
        let seq = snapshot.received;
        let remove = snapshot.state.held() || self.output.pending_for(id) != 0;
        self.discard_replies(id);
        if self.boot_requested {
            self.clear_boot();
        }
        *self.sessions.get_mut(slot) = Session::default();
        self.counters.failsafe = self.counters.failsafe.wrapping_add(1);
        if remove {
            self.cancel_owner(id, 0, slot);
        }
        self.event(code, id, seq);
        self.notices |= ERROR;
    }

    /// Shared-queue or USB fault: every session is released and told why.
    fn fault(&mut self, code: u8) {
        self.clear_boot();
        self.rx_generation = self.rx_generation.wrapping_add(1);
        self.overflow = false;
        self.output.release(self.now);
        self.notices |= ERROR;
        self.tx_head = 0;
        self.tx_count = 0;
        for slot in 0..SESSIONS {
            let snapshot = *self.sessions.get(slot);
            if snapshot.id != 0 {
                self.counters.failsafe = self.counters.failsafe.wrapping_add(1);
                self.event(code, snapshot.id, snapshot.received);
            }
        }
        self.sessions.reset();
    }

    fn lease_remaining(&self, lease_at: u32) -> u16 {
        if self.sessions.count() == 0 {
            return 0;
        }
        let elapsed = self.now.wrapping_sub(lease_at);
        if elapsed >= LEASE_MS {
            0
        } else {
            (LEASE_MS - elapsed) as u16
        }
    }

    fn status(&mut self, slot: usize, seq: u32) {
        let snapshot = *self.sessions.get(slot);
        let mut p = [0u8; 51];
        wire::write32(&mut p[1..], snapshot.received);
        wire::write32(&mut p[5..], snapshot.completed);
        p[9] = self.output.pending_for(snapshot.id);
        let lease = self.lease_remaining(snapshot.lease_at);
        wire::write16(&mut p[10..], lease);
        wire::write16(&mut p[12..], self.counters.rx);
        wire::write16(&mut p[14..], self.counters.tx);
        wire::write16(&mut p[16..], self.counters.hid);
        wire::write16(&mut p[18..], self.counters.failsafe);
        // Client-local state: RELEASE must not test another client's holds.
        p[20] = snapshot.state.modifiers;
        p[21..49].copy_from_slice(&snapshot.state.keys);
        p[49] = snapshot.state.consumer;
        p[50] = snapshot.state.buttons;
        self.reply(wire::STATUS | 0x80, snapshot.id, seq, &p);
    }

    fn check_scheduled(&mut self, ok: bool) -> bool {
        let code = self.output.take_fault();
        if code != 0 {
            self.fault(code);
        }
        ok
    }

    fn queue_state(&mut self, seq: u32, force: bool, session: u32) -> bool {
        let sources = self.sessions.states();
        let merged = merge_sources(&sources);
        let ok = self
            .output
            .state(merged, &sources, self.now, seq, force, session);
        self.check_scheduled(ok)
    }

    fn queue_motion(
        &mut self,
        x: i32,
        y: i32,
        wheel: i32,
        hwheel: i32,
        seq: u32,
        session: u32,
    ) -> bool {
        let sources = self.sessions.states();
        let ok = self
            .output
            .motion(x, y, wheel, hwheel, &sources, self.now, seq, session);
        self.check_scheduled(ok)
    }

    fn queue_barrier(&mut self, seq: u32, session: u32) -> bool {
        let sources = self.sessions.states();
        let ok = self.output.barrier(&sources, self.now, seq, session);
        self.check_scheduled(ok)
    }

    fn cancel_owner(&mut self, session: u32, seq: u32, slot: usize) -> bool {
        let sources = self.sessions.states();
        let ok = self
            .output
            .release_session(session, seq, slot, &sources, self.now);
        self.check_scheduled(ok)
    }
}
