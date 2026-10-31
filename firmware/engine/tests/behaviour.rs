//! Behaviour parity with `firmware/tests/rp2040_engine_test.cpp`. Every
//! assertion here exists in the C++ suite; if the two ever disagree, one of them
//! has a real defect.

mod common;

use common::{Harness, command};
use firmware_engine::{
    BOOT_CONFIRMED, BootloaderGate, ERROR, FAILSAFE, InputEngine, LedState, PlatformInfo,
    StatusIndicator, config, wire,
};

#[test]
fn independent_engines_and_counter_wrap() {
    let mut first = Harness::new(0);
    let mut second = Harness::new(0);
    first.open();
    second.open();

    first.send(wire::KEY, 2, &[7, 0, 4, 0, 1]);
    first.tick(8);
    assert_eq!(first.engine.completed_state().keys[0], 16);
    assert!(!second.engine.completed_state().held());

    // u16 RX counters wrap instead of saturating, and never leak across instances.
    first.engine.receive_fault(0, 65535, first.now);
    first.engine.receive_fault(0, 1, first.now);
    assert_eq!(first.engine.counters().rx, 0);
    assert_eq!(second.engine.counters().failsafe, 0);
}

#[test]
fn clock_wrap_keeps_lease_and_status_consistent() {
    let mut wrap = Harness::new(u32::MAX - 1000);
    wrap.open();
    let leased = wrap.now.wrapping_sub(12);

    wrap.now = leased.wrapping_add(1999);
    wrap.engine.service_control(wrap.now);
    assert!(wrap.engine.session(42).is_some());

    wrap.send(wire::STATUS, 2, &[]);
    let status = wrap.engine.vendor_reply().copied().expect("STATUS reply");
    // Reply header is 12 bytes, so the payload's lease field lands at offset 22.
    assert_eq!(wire::read16(&status.bytes[22..]), 1);
    wrap.engine.vendor_submitted();

    wrap.now = leased.wrapping_add(2000);
    wrap.engine.service_control(wrap.now);
    assert!(wrap.engine.session(42).is_none());
    assert_eq!(wrap.engine.counters().failsafe, 1);
    assert_eq!(wrap.engine.vendor_reply().unwrap().bytes[wire::HEADER], 5);
}

#[test]
fn status_and_barrier_do_not_renew_the_lease() {
    let mut h = Harness::new(0);
    h.open();
    let leased = h.engine.session(42).unwrap().lease_at;
    h.now = leased.wrapping_add(1000);
    h.send(wire::STATUS, 2, &[]);
    h.send(wire::BARRIER, 3, &[]);
    h.tick(5);

    h.now = leased.wrapping_add(1999);
    h.engine.service_control(h.now);
    assert!(h.engine.session(42).is_some());
    h.now = leased.wrapping_add(2000);
    h.engine.service_control(h.now);
    assert!(h.engine.session(42).is_none());
    h.send(wire::HEARTBEAT, 4, &[]);
    assert!(h.engine.session(42).is_none());
}

#[test]
fn a_job_older_than_fifty_ms_faults_the_shared_queue() {
    let mut h = Harness::new(0);
    h.open();
    h.send(wire::KEY, 2, &[7, 0, 4, 0, 1]);
    let queued = h.now;
    h.acknowledge = false;
    h.step();

    h.now = queued.wrapping_add(50);
    h.step_by(0);
    assert!(h.engine.session(42).is_some());
    h.now = queued.wrapping_add(51);
    h.step_by(0);
    assert!(h.engine.session(42).is_none());
    assert_eq!(h.engine.counters().failsafe, 1);
    assert!(
        h.replies
            .iter()
            .any(|r| r.bytes[1] == wire::EVENT && r.bytes[wire::HEADER] == 8)
    );
}

#[test]
fn tx_overflow_faults_and_reports_one_event() {
    let mut tx = Harness::new(0);
    tx.open();
    for seq in 2..=config::TX_DEPTH as u32 + 2 {
        tx.send(wire::STATUS, seq, &[]);
    }
    assert_eq!(tx.engine.counters().tx, 1);

    tx.engine.receive_fault(0, 0, tx.now);
    assert!(tx.engine.session(42).is_none());
    let reply = tx.engine.vendor_reply().unwrap();
    assert_eq!(reply.bytes[1], wire::EVENT);
    assert_eq!(reply.bytes[wire::HEADER], 8);
}

#[test]
fn accumulated_motion_beyond_the_limit_faults_only_at_the_boundary() {
    let mut motion = Harness::new(0);
    motion.open();
    // Nine 1024-pixel moves: the first two open new jobs, the rest merge, so the
    // sum lands exactly on the ±8192 limit instead of overflowing it.
    for seq in 2..=10 {
        motion.send(wire::MOVE, seq, &[0, 4, 0, 0]);
    }
    assert!(motion.engine.session(42).is_some());
    motion.send(wire::MOVE, 11, &[1, 0, 0, 0]);
    assert!(motion.engine.session(42).is_none());
}

#[test]
fn boot_confirmation_requires_the_matching_generation() {
    let mut boot = Harness::new(0);
    boot.open();
    boot.send(wire::BOOTLOADER, 2, &[]);
    while !boot.vendor_pending {
        boot.tick(1);
    }
    assert!(boot.engine.boot_requested());
    assert!(!boot.engine.boot_confirmed());

    boot.send(wire::STATUS, 3, &[]);
    boot.engine.vendor_submitted();
    // A later control reply must not erase the boot ACK frontier.
    boot.engine
        .vendor_completed(boot.vendor_generation.wrapping_add(1));
    boot.engine.service_control(boot.now);
    assert!(!boot.engine.boot_confirmed());

    boot.engine.vendor_completed(boot.vendor_generation);
    boot.engine.service_control(boot.now);
    assert!(boot.engine.boot_confirmed());

    // A bus reset between submit and completion cancels the confirmation.
    boot.engine.receive_fault(7, 0, boot.now);
    assert!(!boot.engine.boot_confirmed());
    boot.engine.vendor_completed(boot.vendor_generation);
    boot.engine.service_control(boot.now);
    assert!(!boot.engine.boot_confirmed());
}

#[test]
fn peer_release_preserves_history_and_the_other_session() {
    let mut h = Harness::new(0);
    h.open();
    h.send_as(wire::OPEN, 1, &[], 43);
    h.tick(12);
    h.send(wire::RELEASE, 2, &[]);
    h.tick(12);

    let lease = h.engine.session(42).unwrap().lease_at;
    h.now = lease.wrapping_add(1900);
    h.send_as(wire::HEARTBEAT, 2, &[], 43);
    h.now = lease.wrapping_add(2000);
    h.engine.service_control(h.now);
    assert!(h.engine.session(42).is_none());
    assert!(h.engine.session(43).is_some());

    h.tick(12);
    h.send_as(wire::KEY, 3, &[7, 0, 4, 0, 1], 43);
    h.send_as(wire::KEY, 4, &[7, 0, 4, 0, 0], 43);
    h.send_as(wire::BARRIER, 5, &[], 43);
    h.tick(12);

    assert!(
        h.input
            .iter()
            .any(|r| r.id == wire::KEYBOARD_REPORT_ID && r.data[1] & 16 != 0)
    );
    assert!(
        h.replies
            .iter()
            .any(|r| { r.bytes[1] == (wire::BARRIER | 128) && wire::read32(&r.bytes[4..]) == 43 })
    );
    assert!(!h.engine.completed_state().held());
    assert_eq!(h.engine.counters().failsafe, 1);
}

#[test]
fn close_frees_one_owner_without_a_fault_and_keeps_the_peer() {
    let mut h = Harness::new(0);
    h.open();
    h.send_as(wire::OPEN, 1, &[], 43);
    h.tick(12);
    h.send(wire::KEY, 2, &[7, 0, 4, 0, 1]);
    h.send_as(wire::KEY, 2, &[7, 0, 4, 0, 1], 43);
    h.send_as(wire::KEY, 3, &[7, 0, 5, 0, 1], 43);
    h.tick(12);
    assert!(h.engine.completed_state().held());

    let failsafe = h.engine.counters().failsafe;
    h.send_as(wire::CLOSE, 4, &[], 43);
    h.tick(12);
    assert!(
        h.engine.session(43).is_none(),
        "CLOSE frees the slot at once"
    );
    assert_eq!(
        h.engine.counters().failsafe,
        failsafe,
        "CLOSE must not count a failsafe"
    );
    // 'a' stays because 42 still holds it; 'b' came only from 43.
    let last = h
        .input
        .iter()
        .rev()
        .find(|r| r.id == wire::KEYBOARD_REPORT_ID)
        .expect("a report must be sent");
    assert!(last.data[1] & 16 != 0, "peer key must stay held");
    assert!(
        last.data[1] & 32 == 0,
        "closed owner's key must be released"
    );

    // The slot is free immediately: no lease wait.
    h.send_as(wire::OPEN, 1, &[], 44);
    h.tick(12);
    assert!(h.engine.session(44).is_some());

    // Closing the last owner releases everything and is still not a fault.
    h.send(wire::CLOSE, 3, &[]);
    h.tick(12);
    assert!(!h.engine.completed_state().held());
    assert_eq!(h.engine.counters().failsafe, failsafe);
}

#[test]
fn status_ext_reports_diagnostics() {
    let mut h = Harness::new(0);
    h.open();
    h.engine.set_platform(PlatformInfo {
        rx_depth: 3,
        reset_reason: 2,
        boot_count: 7,
    });
    h.engine.set_build(0xdead_beef);
    h.engine.note_late_tick(420);

    h.send(wire::STATUS_EXT, 2, &[]);
    h.tick(6);
    let reply = h
        .replies
        .iter()
        .rev()
        .find(|r| r.bytes[1] == (wire::STATUS_EXT | 128))
        .expect("STATUS_EXT must be answered");
    let p = &reply.bytes[wire::HEADER..wire::HEADER + wire::EXT_BYTES];
    assert_eq!(p[0], 0, "result ok");
    assert_eq!(u16::from_le_bytes([p[5], p[6]]), 420, "late tick max");
    assert_eq!(p[8], 3, "rx depth comes from the platform");
    assert_eq!(p[10], 1, "one session");
    assert_eq!(p[16], 2, "reset reason");
    assert_eq!(u16::from_le_bytes([p[17], p[18]]), 7, "boot count");
    assert_eq!(
        u32::from_le_bytes([p[19], p[20], p[21], p[22]]),
        0xdead_beef,
        "build hash"
    );

    // Reading clears the late-tick high-water mark.
    h.send(wire::STATUS_EXT, 3, &[]);
    h.tick(6);
    let reply = h
        .replies
        .iter()
        .rev()
        .find(|r| r.bytes[1] == (wire::STATUS_EXT | 128))
        .expect("second STATUS_EXT");
    assert_eq!(
        u16::from_le_bytes([reply.bytes[wire::HEADER + 5], reply.bytes[wire::HEADER + 6]]),
        0,
        "reading clears the late-tick high-water mark"
    );
    assert_eq!(h.engine.counters().failsafe, 0);
}

#[test]
fn session_list_reports_every_owner() {
    // With eight sessions the host needs to know who is holding a key, not just
    // how many sessions exist.
    let mut h = Harness::new(0);
    h.open();
    h.send_as(wire::OPEN, 1, &[], 43);
    h.tick(12);
    h.send(wire::KEY, 2, &[7, 0, 4, 0, 1]);
    h.tick(12);

    h.send(wire::SESSION_LIST, 3, &[]);
    h.tick(6);
    let reply = h
        .replies
        .iter()
        .rev()
        .find(|r| r.bytes[1] == (wire::SESSION_LIST | 128))
        .expect("SESSION_LIST must be answered");
    let p = &reply.bytes[wire::HEADER..wire::HEADER + wire::SESSION_LIST_BYTES];
    assert_eq!(u16::from_le_bytes([p[0], p[1]]), 42, "first owner id tail");
    assert!(u16::from_le_bytes([p[2], p[3]]) > 0, "lease still running");
    assert_eq!(p[4], 1, "one key held");
    assert_eq!(p[5], 0, "no pending work");
    assert_eq!(u16::from_le_bytes([p[6], p[7]]), 43, "second owner");
    assert_eq!(p[10], 0, "second owner holds nothing");
}

#[test]
fn bootloader_gate_needs_120ms_and_a_quiet_bus() {
    let mut gate = BootloaderGate::default();
    let at = u32::MAX - 60;
    assert!(!gate.advance(at, false, true, true, true));
    assert!(!gate.advance(at, true, true, true, true));
    assert!(gate.pending());
    assert!(!gate.advance(at.wrapping_add(119), true, true, true, true));
    assert!(!gate.advance(at.wrapping_add(120), true, false, true, true));
    assert!(!gate.advance(at.wrapping_add(120), true, true, false, true));
    assert!(!gate.advance(at.wrapping_add(120), true, true, true, false));
    assert!(gate.advance(at.wrapping_add(120), true, true, true, true));
    assert!(!gate.advance(at.wrapping_add(120), false, true, true, true));
    assert!(!gate.pending());
    assert!(!gate.advance(at.wrapping_add(121), true, true, true, true));
}

#[test]
fn led_priority_and_timers_wrap() {
    let mut led = StatusIndicator::default();
    led.update(0, false, false);
    assert_eq!(led.state(), LedState::Off);
    led.update(0, true, false);
    assert_eq!(led.state(), LedState::Idle);
    led.update(0, true, true);
    assert_eq!(led.state(), LedState::Held);

    let start = u32::MAX - 50;
    led.signal_failsafe(start);
    led.signal_error(start);
    led.update(start.wrapping_add(139), true, true);
    assert_eq!(led.state(), LedState::Error);
    led.update(start.wrapping_add(140), true, true);
    assert_eq!(led.state(), LedState::Failsafe);
    led.update(start.wrapping_add(2999), true, true);
    assert_eq!(led.state(), LedState::Failsafe);
    led.update(start.wrapping_add(3000), true, true);
    assert_eq!(led.state(), LedState::Held);

    led.signal_bootloader();
    led.update(0, false, false);
    assert_eq!(led.state(), LedState::Bootloader);
}

#[test]
fn notices_are_reported_once() {
    let mut engine = InputEngine::new();
    let mut h = Harness::new(0);
    h.open();
    h.send(wire::KEY, 2, &[7, 0, 4, 0, 1]);
    // Force a lease expiry through the public path.
    h.now = h.now.wrapping_add(config::LEASE_MS);
    h.engine.service_control(h.now);
    let notices = h.engine.take_notices();
    assert!(notices & FAILSAFE != 0, "expiry must request failsafe LED");
    assert!(notices & ERROR != 0, "expiry is reported as an event");
    assert!(notices & BOOT_CONFIRMED == 0);
    assert_eq!(
        h.engine.take_notices(),
        0,
        "notices are consumed by the reader"
    );

    // An unsolicited command from an unknown session is ignored, not counted as a
    // protocol fault for anyone.
    engine.handle(&command(wire::KEY, 2, &[7, 0, 4, 0, 1], 9), 1);
    assert_eq!(engine.session_count(), 0);
}
