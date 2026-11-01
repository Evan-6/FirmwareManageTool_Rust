//! End-to-end SDK tests over the in-memory device: no hardware required.

use gdb::manager::Manager;
use gdb::{Button, ConnectionState, DeviceOptions, MotionOptions, TypeTextConfig};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn open() -> (gdb::Device, gdb::mock::MockHandle) {
    Manager::open_mock_with_handle(DeviceOptions::default()).expect("mock device")
}

/// How long the first heartbeat may take. The heartbeat is a thread of its own
/// that the device starts when it opens, and its first tick is not synchronised
/// with the caller: a freshly spawned thread was measured more than 1.2 s late on
/// Windows — 14 of 150 runs of a 1.2 s window saw no tick at all and then three
/// beats in the next 1.5 s. A fixed sleep would be a guess at that latency, so
/// these tests poll the device and wait for the frame instead.
const FIRST_BEAT_WAIT: Duration = Duration::from_secs(5);

/// The lease Feature 14 reports: a session that has not renewed for this long is
/// gone. Between two beats the only renewal in flight is the HEARTBEAT itself, so
/// a host that keeps its session must beat faster than this — it is the bound the
/// second beat of each test is asserted against, and it is not a guess.
const LEASE_WAIT: Duration = Duration::from_millis(2000);

/// Delay between polls: this is the "does not starve the heartbeat" traffic, so
/// it is deliberately faster than the 150 ms a host polls at.
const POLL_GAP: Duration = Duration::from_millis(20);

fn heartbeats(handle: &gdb::mock::MockHandle, before: usize) -> usize {
    handle.writes()[before..]
        .iter()
        .filter(|packet| packet.opcode == input_protocol::v3::HEARTBEAT)
        .count()
}

/// Keeps sending `poll` until `wanted` HEARTBEATs have gone out after `before`,
/// or `deadline` runs out, and reports how many it saw. Driving the traffic in a
/// bounded loop is what makes the two polling tests deterministic: the frame's
/// arrival is asynchronous, so sleeping a fixed number of beat periods only
/// asserts that the machine was fast enough.
fn poll_until_beats(
    handle: &gdb::mock::MockHandle,
    before: usize,
    wanted: usize,
    deadline: Duration,
    mut poll: impl FnMut(),
) -> usize {
    let start = Instant::now();
    loop {
        poll();
        let beats = heartbeats(handle, before);
        if beats >= wanted || start.elapsed() >= deadline {
            return beats;
        }
        std::thread::sleep(POLL_GAP);
    }
}

/// The two halves of the polling tests: wait out the heartbeat thread's own start,
/// then assert the beat rate against the lease it has to keep alive. Both halves
/// poll the whole time, so the traffic that used to starve the heartbeat is
/// present for every frame that is expected.
fn poll_for_two_beats(
    device: &gdb::Device,
    handle: &gdb::mock::MockHandle,
    before: usize,
    mut poll: impl FnMut(),
) {
    let first = poll_until_beats(handle, before, 1, FIRST_BEAT_WAIT, &mut poll);
    assert!(
        first >= 1,
        "polling must not stop the heartbeat (saw none in {FIRST_BEAT_WAIT:?})"
    );
    // The second beat is what proves the rate rather than one lucky frame: later
    // than one lease and the session would be gone in spite of the heartbeat.
    let beats = poll_until_beats(handle, before, 2, LEASE_WAIT, &mut poll);
    assert!(
        beats >= 2,
        "the next heartbeat must arrive inside one {LEASE_WAIT:?} lease (saw {beats})"
    );
    assert_eq!(
        device.metrics().reconnects,
        0,
        "the beats must keep the session that was opened, not a reopened one"
    );
}

#[test]
fn connects_reports_info_and_closes() {
    let (device, handle) = open();
    let info = device.info();
    assert_eq!(info.board, 2);
    assert_eq!(info.firmware, "3.2.0");
    assert_eq!(info.lease_ms, 2000);
    assert_eq!(info.poll_ms, 1);
    assert_eq!(info.capabilities, 15);
    assert_eq!(device.state(), ConnectionState::Connected);

    let status = device.status().expect("status");
    assert_eq!(status.lease_ms, 2000);
    let ext = device.status_ext().expect("status_ext");
    assert_eq!(ext.build, 0x0DB0_0001);
    assert_eq!(ext.boot_count, 1);
    let owners = device.sessions().expect("sessions");
    assert_eq!(owners.len(), 1);
    assert_eq!(owners[0].id, handle.session() as u16);

    device.close().expect("close");
    assert!(handle.is_closed());
    assert_eq!(device.state(), ConnectionState::Disconnected);
}

#[test]
fn raw_commands_and_the_frame_trace_reach_the_caller() {
    let (device, handle) = open();
    let before = handle.writes().len();
    let reply = device
        .command(&input_protocol::v3::Command::Status)
        .expect("status command")
        .expect("status reply");
    assert_eq!(reply.opcode, input_protocol::v3::STATUS | 0x80);
    assert_eq!(handle.writes().len(), before + 1);

    // Trace level forwards both directions through the log sink, which is how a
    // host that wants the raw frames (debug pane, packet log) gets them.
    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = lines.clone();
    gdb::log::set_level(gdb::Level::Trace);
    gdb::log::set_sink(Some(Box::new(move |_, message| {
        sink.lock().unwrap().push(message.to_string());
    })));
    device.barrier().expect("barrier");
    gdb::log::set_sink(None);
    gdb::log::set_level(gdb::Level::Off);

    let lines = lines.lock().unwrap();
    assert!(
        lines.iter().any(|line| line.starts_with("out opcode=")),
        "no outgoing frame in {lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.starts_with("in opcode=")),
        "no incoming frame in {lines:?}"
    );
}

#[test]
fn status_polling_does_not_starve_the_lease_heartbeat() {
    // STATUS does not renew the lease (firmware/engine/src/engine.rs). A host
    // that polls STATUS every 150 ms used to look busy to the SDK, so the
    // heartbeat never fired and the session died every lease period.
    let (device, handle) = open();
    let before = handle.writes().len();
    poll_for_two_beats(&device, &handle, before, || {
        device.status().expect("status");
    });
}

#[test]
fn status_ext_polling_does_not_starve_the_lease_heartbeat() {
    // STATUS_EXT is not a renewal either, so a host whose only traffic is a
    // 100 ms diagnostic poll must still beat inside the 2 s lease instead of
    // losing the session to its own diagnostics.
    let (device, handle) = open();
    let before = handle.writes().len();
    poll_for_two_beats(&device, &handle, before, || {
        device.status_ext().expect("status_ext");
    });
}

#[test]
fn keys_chords_and_text_reach_the_device() {
    let (device, handle) = open();
    let keyboard = device.keyboard();
    keyboard.key_down(7, 0x04).unwrap();
    assert!(handle.held().keys[0] & 0x10 != 0);
    keyboard.key_up(7, 0x04).unwrap();
    assert_eq!(handle.held().keys[0] & 0x10, 0);

    keyboard.chord(&[0xE0, 0x06], 10).unwrap();
    // The chord is synchronous: both keys go down and come back up.
    let downs: Vec<u16> = handle
        .writes()
        .iter()
        .filter(|packet| packet.opcode == input_protocol::v3::KEY && packet.payload[4] == 1)
        .map(|packet| u16::from_le_bytes([packet.payload[2], packet.payload[3]]))
        .collect();
    assert!(downs.contains(&0x06), "{downs:?}");
    assert_eq!(handle.held().keys[0] & 0x40, 0);

    keyboard
        .type_text(&TypeTextConfig {
            text: "hi".to_string(),
            interval_ms: 1,
            hold_ms: 1,
            jitter_ms: 0,
        })
        .unwrap();
    assert_eq!(
        handle.held().keys.iter().filter(|byte| **byte != 0).count(),
        0
    );

    keyboard.long_press(7, 0x2C, 5).unwrap();
    assert_eq!(handle.held().keys[5] & 0x10, 0);
}

#[test]
fn relative_moves_are_split_into_hid_sized_reports() {
    let (device, handle) = open();
    let before = handle.writes().len();
    device
        .mouse()
        .move_by(1500, -1100, MotionOptions::linear(40))
        .unwrap();
    let moves: Vec<_> = handle.writes()[before..]
        .iter()
        .filter(|packet| packet.opcode == input_protocol::v3::MOUSE_MOVE)
        .map(|packet| {
            (
                i16::from_le_bytes([packet.payload[0], packet.payload[1]]),
                i16::from_le_bytes([packet.payload[2], packet.payload[3]]),
            )
        })
        .collect();
    assert_eq!(moves, vec![(1024, -1024), (476, -76)]);
}

#[test]
fn raw_moves_go_out_without_a_barrier() {
    let (device, handle) = open();
    let before = handle.writes().len();
    device.mouse().move_raw(1500, -1100).unwrap();
    let writes: Vec<_> = handle.writes()[before..].to_vec();
    let moves: Vec<_> = writes
        .iter()
        .filter(|packet| packet.opcode == input_protocol::v3::MOUSE_MOVE)
        .map(|packet| {
            (
                i16::from_le_bytes([packet.payload[0], packet.payload[1]]),
                i16::from_le_bytes([packet.payload[2], packet.payload[3]]),
            )
        })
        .collect();
    // Same splitting as move_by, minus the per-chunk completion wait.
    assert_eq!(moves, vec![(1024, -1024), (476, -76)]);
    assert!(
        writes
            .iter()
            .all(|packet| packet.opcode != input_protocol::v3::BARRIER),
        "move_raw must not flush"
    );
}

#[test]
fn clicks_drags_and_release_all() {
    let (device, handle) = open();
    let mouse = device.mouse();
    mouse.click(Button::Left, 2).unwrap();
    let buttons: Vec<u8> = handle
        .writes()
        .iter()
        .filter(|packet| packet.opcode == input_protocol::v3::MOUSE_BUTTONS)
        .map(|packet| packet.payload[0])
        .collect();
    assert_eq!(buttons, vec![1, 0, 1, 0]);

    mouse.button(Button::Right, true).unwrap();
    assert_eq!(handle.held().buttons, 2);
    mouse.release_all().unwrap();
    assert_eq!(handle.held().buttons, 0);
}

#[test]
fn a_failed_operation_reconnects_once_and_keeps_working() {
    let (device, handle) = open();
    let writes = handle.writes().len();
    // Fail the next write, then let the fresh mock accept everything.
    handle.set_disconnect_after(Some(writes));
    let status = device.status().expect("auto reconnect");
    assert_eq!(status.lease_ms, 2000);
    assert_eq!(device.state(), ConnectionState::Connected);
}

#[test]
fn no_hardware_is_required_for_the_mock() {
    let (device, _handle) = open();
    assert_eq!(device.info().serial, "MOCK");
    assert_eq!(device.info().product, "GDBOX mock");
}
