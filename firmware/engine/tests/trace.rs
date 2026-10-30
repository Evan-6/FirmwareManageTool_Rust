//! A/B parity: the script below is byte-for-byte the one in
//! `firmware/tests/engine_trace.cpp`. `scripts/test-rp2040.sh` re-runs the C++
//! side and diffs it against the committed golden, so this test fails whenever
//! the Rust port and the shipping engine disagree.

mod common;

use common::Harness;
use firmware_engine::wire;

const GOLDEN: &str = include_str!("../../tests/engine_trace.golden");
const A: u32 = 0x1111_1111;
const B: u32 = 0x2222_2222;
const C: u32 = 0x3333_3333;

#[test]
fn trace_matches_the_cpp_engine() {
    let mut h = Harness::new(0);

    h.send_as(wire::OPEN, 1, &[], A);
    h.tick(6);
    h.send_as(wire::OPEN, 1, &[], B);
    h.tick(6);
    h.send_as(wire::KEY, 2, &[7, 0, 0x1e, 0, 1], A);
    h.tick(3);
    h.send_as(wire::KEY, 2, &[7, 0, 0x1f, 0, 1], B);
    h.tick(3);
    h.send_as(wire::MOVE, 3, &[7, 0, 0xf9, 0xff], A);
    h.tick(3);
    h.send_as(wire::BUTTONS, 3, &[1], B);
    h.tick(3);
    h.send_as(wire::BARRIER, 4, &[], A);
    h.tick(6);
    h.send_as(wire::STATUS, 4, &[], B);
    h.tick(2);
    h.send_as(wire::RELEASE, 5, &[], A);
    h.tick(6);
    h.send_as(wire::KEY, 5, &[7, 0, 0x1f, 0, 0], B);
    h.tick(3);
    h.send_as(wire::BARRIER, 6, &[], B);
    h.tick(6);
    h.send_as(wire::HEARTBEAT, 7, &[], B);
    h.tick(2);
    h.send_as(wire::STATUS, 8, &[], B);
    h.tick(2);

    // Stop acknowledging USB so the head job ages out.
    h.acknowledge = false;
    h.send_as(wire::KEY, 9, &[7, 0, 0x20, 0, 1], B);
    h.step();
    // 50 ms is still allowed, 51 ms is stale.
    // step() already advanced 1 ms, so +49 puts the head job at exactly 50 ms.
    h.now += 49;
    h.step_by(0);
    h.now += 1;
    h.step_by(0);
    h.acknowledge = true;
    // The global fault queued one all-clear state; OPEN is refused until drained.
    for _ in 0..20 {
        if h.engine.idle() {
            break;
        }
        h.step();
    }

    h.send_as(wire::OPEN, 1, &[], C);
    h.tick(6);
    h.send_as(wire::BOOTLOADER, 2, &[], C);
    for _ in 0..20 {
        if h.vendor_pending {
            break;
        }
        h.step();
    }
    h.tick(8);

    // Lease expiry after the boot request.
    h.now += 2000;
    h.engine.service_control(h.now);
    h.tick(2);

    let counters = h.engine.counters();
    let mut actual = h.trace.clone();
    actual.push_str(&format!(
        "S {} {} {} {} {} {} {} {}\n",
        h.now,
        h.engine.session_count(),
        counters.rx,
        counters.tx,
        counters.hid,
        counters.failsafe,
        h.engine.idle() as u8,
        h.engine.boot_confirmed() as u8
    ));
    compare(&actual, GOLDEN);
}

/// Reports the first differing line instead of dumping two long traces.
fn compare(actual: &str, expected: &str) {
    let mut lines = actual.lines().zip(expected.lines());
    for (index, (got, want)) in lines.by_ref().enumerate() {
        assert_eq!(
            got,
            want,
            "trace line {} differs from the C++ engine",
            index + 1
        );
    }
    assert_eq!(
        actual.lines().count(),
        expected.lines().count(),
        "trace length differs from the C++ engine"
    );
}
