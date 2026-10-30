//! Adapter tests for the transport-agnostic runtime. The USB peripheral itself
//! needs hardware, but the loop order, RX budget, fault propagation, boot gate
//! and LED wiring are all decided here.

use std::collections::VecDeque;

use firmware_engine::{InputReport, LedState, Report, wire};
use xiao_firmware::{Runtime, RxFault, Transport};

#[derive(Default)]
struct Mock {
    mounted: bool,
    rx: VecDeque<Report>,
    discarded: usize,
    sent_input: Vec<InputReport>,
    sent_vendor: Vec<Report>,
    inflight: Option<u8>,
    fail_next_input: bool,
    vendor_complete: Option<u32>,
    reset: bool,
    fault: RxFault,
}

impl Transport for Mock {
    fn mounted(&mut self) -> bool {
        self.mounted
    }

    fn input_ready(&mut self) -> bool {
        self.inflight.is_none()
    }

    fn vendor_ready(&mut self) -> bool {
        true
    }

    fn send_input(&mut self, report: &InputReport) -> bool {
        if self.fail_next_input {
            self.fail_next_input = false;
            return false;
        }
        self.sent_input.push(*report);
        self.inflight = Some(report.id);
        true
    }

    fn send_vendor(&mut self, report: &Report, boot_generation: u32) -> bool {
        self.sent_vendor.push(*report);
        // Model the awaited write completing right away; the runtime only reads
        // it on the next pass, which is the contract the engine expects.
        self.vendor_complete = Some(boot_generation);
        true
    }

    fn read(&mut self, report: &mut Report) -> bool {
        match self.rx.pop_front() {
            Some(packet) => {
                *report = packet;
                true
            }
            None => false,
        }
    }

    fn discard_rx(&mut self) {
        self.discarded += self.rx.len();
        self.rx.clear();
    }

    fn take_input_complete(&mut self) -> u8 {
        self.inflight.take().unwrap_or(0)
    }

    fn take_input_failure(&mut self) -> bool {
        false
    }

    fn take_vendor_complete(&mut self) -> Option<u32> {
        self.vendor_complete.take()
    }

    fn take_reset(&mut self) -> bool {
        std::mem::take(&mut self.reset)
    }

    fn take_rx_fault(&mut self) -> RxFault {
        std::mem::take(&mut self.fault)
    }
}

fn command(op: u8, sequence: u32, data: &[u8], session: u32) -> Report {
    let mut report = Report::default();
    report.bytes[0] = wire::VERSION;
    report.bytes[1] = op;
    report.bytes[2] = data.len() as u8;
    wire::write32(&mut report.bytes[4..], session);
    wire::write32(&mut report.bytes[8..], sequence);
    report.bytes[wire::HEADER..wire::HEADER + data.len()].copy_from_slice(data);
    report
}

fn new_runtime() -> (Runtime<Mock>, u32) {
    let mut runtime = Runtime::new(Mock::default());
    let mut now = 0;

    // Unmounted passes: no session, no output.
    now += 1;
    runtime.step(now);

    // Enumeration queues one all-clear state, exactly like the C++ transport.
    runtime.transport_mut().mounted = true;
    for _ in 0..4 {
        now += 1;
        runtime.step(now);
    }

    runtime
        .transport_mut()
        .rx
        .push_back(command(wire::OPEN, 1, &[], 42));
    for _ in 0..4 {
        now += 1;
        runtime.step(now);
    }
    assert!(runtime.engine().session(42).is_some(), "OPEN must allocate");
    assert!(
        runtime
            .transport()
            .sent_vendor
            .iter()
            .any(|r| r.bytes[1] == (wire::OPEN | 128)),
        "OPEN must be acknowledged"
    );
    (runtime, now)
}

#[test]
fn receives_at_most_four_packets_per_pass() {
    let (mut runtime, mut now) = new_runtime();
    for sequence in 2..=7 {
        let packet = command(wire::STATUS, sequence, &[], 42);
        runtime.transport_mut().rx.push_back(packet);
    }

    now += 1;
    runtime.step(now);
    assert_eq!(
        runtime.engine().session(42).unwrap().received,
        5,
        "one pass must consume exactly four packets"
    );

    now += 1;
    runtime.step(now);
    assert_eq!(runtime.engine().session(42).unwrap().received, 7);
    assert!(runtime.transport_mut().rx.is_empty());
}

#[test]
fn a_rejected_input_report_faults_the_device() {
    let (mut runtime, mut now) = new_runtime();
    runtime
        .transport_mut()
        .rx
        .push_back(command(wire::KEY, 2, &[7, 0, 4, 0, 1], 42));
    runtime.transport_mut().fail_next_input = true;

    now += 1;
    runtime.step(now);
    assert_eq!(runtime.engine().counters().hid, 1);
    assert!(runtime.engine().session(42).is_none());
    assert_eq!(runtime.led_state(), LedState::Error);
}

#[test]
fn a_bus_reset_faults_every_session_and_discards_stale_packets() {
    let (mut runtime, mut now) = new_runtime();
    runtime
        .transport_mut()
        .rx
        .push_back(command(wire::STATUS, 2, &[], 42));
    runtime.transport_mut().reset = true;

    now += 1;
    runtime.step(now);

    assert_eq!(runtime.engine().session_count(), 0);
    assert_eq!(runtime.engine().counters().failsafe, 1);
    assert!(
        runtime.transport().discarded >= 1,
        "stale packets must not survive the fault"
    );
    let event = runtime
        .transport()
        .sent_vendor
        .iter()
        .find(|r| r.bytes[1] == wire::EVENT)
        .expect("session fault must queue an EVENT");
    assert_eq!(event.bytes[wire::HEADER], 7);
}

#[test]
fn rx_framing_fault_reaches_the_engine_with_its_error_count() {
    let (mut runtime, mut now) = new_runtime();
    runtime.transport_mut().fault = RxFault { code: 1, errors: 3 };
    now += 1;
    runtime.step(now);
    assert_eq!(runtime.engine().counters().rx, 3);
    assert_eq!(runtime.engine().session_count(), 0);
}

#[test]
fn boot_request_waits_for_the_gate_and_a_quiet_bus() {
    let (mut runtime, mut now) = new_runtime();
    runtime
        .transport_mut()
        .rx
        .push_back(command(wire::BOOTLOADER, 2, &[], 42));

    // The release job must drain, then the reply, then its completion.
    let mut passes = 0;
    while !runtime.engine().boot_confirmed() && passes < 16 {
        now += 1;
        runtime.step(now);
        passes += 1;
    }
    assert!(runtime.engine().boot_requested());
    assert!(
        runtime.engine().boot_confirmed(),
        "the BOOTLOADER reply must complete on the bus"
    );
    assert!(!runtime.take_boot_request(), "the gate has not opened yet");

    let confirmed_at = now;
    now = confirmed_at + 119;
    runtime.step(now);
    assert!(!runtime.take_boot_request(), "119 ms is too early");

    now = confirmed_at + 120;
    runtime.step(now);
    assert!(
        runtime.take_boot_request(),
        "120 ms with a quiet bus reboots"
    );
    assert!(!runtime.take_boot_request(), "the request is consumed once");
}
