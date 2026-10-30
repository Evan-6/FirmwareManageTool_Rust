//! Runs the committed host-codec goldens (`shared/input-protocol/vectors.json`)
//! through the Rust device engine. This is the cross-language check: the host
//! expects these bytes, the device must accept them and answer in kind.

mod common;

include!("data/vectors_generated.rs");

use common::Harness;
use firmware_engine::{Report, wire};

const SESSION: u32 = 0x1234_5678;

/// Host vectors are full 64-byte HID reports; the engine takes the 63-byte
/// payload behind report id 12.
fn payload(vector: &[u8; 64]) -> Report {
    assert_eq!(
        vector[0], 12,
        "host vectors must carry vendor HID report id 12"
    );
    let mut report = Report::default();
    report.bytes.copy_from_slice(&vector[1..64]);
    report
}

#[test]
fn every_host_vector_is_a_valid_device_packet() {
    for vector in &WIRE_VECTORS {
        let report = payload(vector);
        assert!(
            wire::valid(&report),
            "host packet rejected by the device framing rules: opcode {}",
            report.bytes[1]
        );
        assert_eq!(wire::read32(&report.bytes[4..]), SESSION);
    }
    let opcodes: Vec<u8> = WIRE_VECTORS.iter().map(|v| v[2]).collect();
    assert_eq!(
        opcodes,
        vec![wire::OPEN, wire::KEY, wire::KEY, wire::KEY, wire::BARRIER]
    );
}

#[test]
fn goldens_drive_the_engine_end_to_end() {
    let mut h = Harness::new(0);

    // OPEN: acknowledged once the queue is free.
    h.engine.handle(&payload(&WIRE_VECTORS[0]), h.now);
    h.tick(4);
    let open = h
        .replies
        .iter()
        .find(|r| r.bytes[1] == (wire::OPEN | 128))
        .expect("OPEN reply");
    assert_eq!(open.bytes[wire::HEADER], 0, "OPEN accepted");
    assert_eq!(wire::read32(&open.bytes[4..]), SESSION);
    assert_eq!(wire::read32(&open.bytes[8..]), 1);

    // Digit1 down (usage 0x1e) must reach the NKRO bitmap.
    h.engine.handle(&payload(&WIRE_VECTORS[1]), h.now);
    h.tick(4);
    assert!(
        h.input
            .iter()
            .any(|r| r.id == wire::KEYBOARD_REPORT_ID && r.data[1 + 3] & 0x40 != 0),
        "digit1 must set keys[3] bit 6"
    );

    // Keypad1 down (usage 0x59) must coexist with digit1.
    h.engine.handle(&payload(&WIRE_VECTORS[2]), h.now);
    h.tick(4);
    assert!(
        h.input
            .iter()
            .any(|r| r.id == wire::KEYBOARD_REPORT_ID && r.data[1 + 11] & 0x02 != 0),
        "keypad1 must set keys[11] bit 1"
    );
    let held = h.engine.desired_state();
    assert_eq!(held.keys[3] & 0x40, 0x40);
    assert_eq!(held.keys[11] & 0x02, 0x02);

    // Volume up (consumer 0x00e9) is bit 1 of the consumer byte.
    h.engine.handle(&payload(&WIRE_VECTORS[3]), h.now);
    h.tick(4);
    assert!(
        h.input
            .iter()
            .any(|r| r.id == wire::CONSUMER_REPORT_ID && r.data[0] == 0x02),
        "volume up must report consumer bit 1"
    );

    // BARRIER completes only after the queued outputs did.
    h.engine.handle(&payload(&WIRE_VECTORS[4]), h.now);
    h.tick(8);
    let barrier = h
        .replies
        .iter()
        .find(|r| r.bytes[1] == (wire::BARRIER | 128))
        .expect("BARRIER reply");
    assert_eq!(barrier.bytes[wire::HEADER], 0);
    assert_eq!(wire::read32(&barrier.bytes[8..]), 5);
    assert!(h.engine.completed_state().held());
    assert_eq!(h.engine.counters().failsafe, 0);
}
