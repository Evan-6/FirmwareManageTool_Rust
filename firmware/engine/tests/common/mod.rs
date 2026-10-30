//! Shared harness for the engine tests. It mirrors `firmware/tests/rp2040_engine_test.cpp`:
//! `now` advances one millisecond per step, one input report is in flight at a
//! time, and every queued vendor reply is drained immediately.
#![allow(dead_code)]

use firmware_engine::{InputEngine, InputReport, Report, wire};
use std::fmt::Write as _;

pub struct Harness {
    pub engine: InputEngine,
    pub now: u32,
    pub mounted: bool,
    /// False stops USB completions, so queued jobs age out.
    pub acknowledge: bool,
    pending: u8,
    pub vendor_pending: bool,
    pub vendor_generation: u32,
    pub input: Vec<InputReport>,
    pub replies: Vec<Report>,
    /// Text form of everything the platform could observe, used by the A/B test
    /// against `firmware/tests/engine_trace.cpp`.
    pub trace: String,
}

impl Harness {
    pub fn new(start: u32) -> Self {
        let mut harness = Self {
            engine: InputEngine::new(),
            now: start,
            mounted: true,
            acknowledge: true,
            pending: 0,
            vendor_pending: false,
            vendor_generation: 0,
            input: Vec::new(),
            replies: Vec::new(),
            trace: String::new(),
        };
        harness.tick(12);
        harness
    }

    pub fn step(&mut self) {
        self.step_by(1);
    }

    pub fn step_by(&mut self, delta: u32) {
        self.now = self.now.wrapping_add(delta);
        let done = if self.acknowledge { self.pending } else { 0 };
        if self.acknowledge {
            self.pending = 0;
        }
        let mut report = InputReport::default();
        self.engine.receive_fault(0, 0, self.now);
        if self.engine.advance(
            self.now,
            self.mounted,
            done,
            false,
            self.pending == 0,
            &mut report,
        ) {
            self.input.push(report);
            self.pending = report.id;
            let _ = write!(
                self.trace,
                "I {} {} {} ",
                self.now, report.id, report.length
            );
            for byte in &report.data[..report.length as usize] {
                let _ = write!(self.trace, "{byte:02x}");
            }
            let _ = writeln!(self.trace);
        }
        if self.vendor_pending && self.acknowledge {
            self.engine.vendor_completed(self.vendor_generation);
            self.vendor_pending = false;
        }
        self.engine.service_control(self.now);
        if let Some(reply) = self.engine.vendor_reply().copied() {
            self.replies.push(reply);
            let _ = write!(
                self.trace,
                "R {} {} {} {} {} ",
                self.now,
                reply.bytes[1],
                wire::read32(&reply.bytes[4..]),
                wire::read32(&reply.bytes[8..]),
                reply.bytes[2]
            );
            for byte in &reply.bytes[wire::HEADER..wire::HEADER + reply.bytes[2] as usize] {
                let _ = write!(self.trace, "{byte:02x}");
            }
            let _ = writeln!(self.trace);
            if reply.bytes[1] == (wire::BOOTLOADER | 128) && reply.bytes[wire::HEADER] == 0 {
                self.vendor_pending = true;
                self.vendor_generation = self.engine.boot_generation();
            }
            self.engine.vendor_submitted();
        }
    }

    pub fn tick(&mut self, n: u32) {
        for _ in 0..n {
            self.step();
        }
    }

    pub fn send(&mut self, op: u8, seq: u32, data: &[u8]) {
        self.send_as(op, seq, data, 42);
    }

    pub fn send_as(&mut self, op: u8, seq: u32, data: &[u8], id: u32) {
        self.engine.handle(&command(op, seq, data, id), self.now);
    }

    pub fn open(&mut self) {
        self.send(wire::OPEN, 1, &[]);
        self.tick(12);
        assert!(
            self.engine.session(42).is_some(),
            "OPEN must allocate session 42"
        );
    }
}

/// Builds the 63-byte payload the host puts behind HID report id 12.
pub fn command(op: u8, seq: u32, data: &[u8], session: u32) -> Report {
    let mut report = Report::default();
    report.bytes[0] = wire::VERSION;
    report.bytes[1] = op;
    report.bytes[2] = data.len() as u8;
    wire::write32(&mut report.bytes[4..], session);
    wire::write32(&mut report.bytes[8..], seq);
    report.bytes[wire::HEADER..wire::HEADER + data.len()].copy_from_slice(data);
    report
}
