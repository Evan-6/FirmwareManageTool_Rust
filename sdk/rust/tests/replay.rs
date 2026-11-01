//! Replay tests over the in-memory device: record, persist, play back.
//!
//! Everything here runs without hardware. Timing assertions are written as
//! bounds that a broken implementation cannot satisfy and a loaded machine still
//! can, so the suite is not a stopwatch.

use gdb::ErrorCode;
use gdb::flash::Cancel;
use gdb::replay::{NoCancel, Op, Recorder, Recording, VERSION};
use gdb::{Button, DeviceOptions, Manager, MotionOptions};
use input_protocol::v3;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

fn open() -> (gdb::Device, gdb::mock::MockHandle) {
    Manager::open_mock_with_handle(DeviceOptions::default()).expect("mock device")
}

/// A temp file that removes itself; tests run in parallel in one process, so
/// every name carries the test's own suffix.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("gdb-replay-{}-{name}.json", std::process::id()));
        let _ = fs::remove_file(&path);
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// The frames a host actually put on the wire, minus session bookkeeping.
///
/// Sequence numbers and session ids are per-connection, so they are left out;
/// the contract a replay must keep is opcode plus payload, in order.
fn frames(handle: &gdb::mock::MockHandle) -> Vec<(u8, Vec<u8>)> {
    handle
        .writes()
        .iter()
        .filter(|packet| {
            matches!(
                packet.opcode,
                v3::KEY
                    | v3::MOUSE_MOVE
                    | v3::MOUSE_BUTTONS
                    | v3::WHEEL
                    | v3::BARRIER
                    | v3::RELEASE_ALL
            )
        })
        .map(|packet| (packet.opcode, packet.payload.clone()))
        .collect()
}

/// A cancel token the test can flip from another thread.
struct Flag(Arc<AtomicBool>);

impl Cancel for Flag {
    fn cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

fn full_recording() -> Recorder {
    let mut recorder = Recorder::new(0x0102_0304, (1920, 1080));
    recorder.record_at(0, Op::MoveRaw { dx: 40, dy: -12 });
    recorder.record_at(
        12,
        Op::Key {
            page: 7,
            usage: 0x04,
            down: true,
        },
    );
    recorder.record_at(
        96,
        Op::Key {
            page: 7,
            usage: 0x04,
            down: false,
        },
    );
    recorder.record_at(97, Op::Wheel { dx: -40, dy: 120 });
    recorder.record_at(98, Op::Buttons { bits: 1 });
    recorder.record_at(99, Op::MoveBy { dx: -3, dy: 9 });
    recorder.record_at(100, Op::Barrier);
    recorder.record_at(101, Op::Wait);
    recorder.record_at(102, Op::ReleaseAll);
    recorder
}

#[test]
fn saving_twice_is_byte_stable_and_load_round_trips() {
    let recorder = full_recording();
    let first = Scratch::new("stable-first");
    let second = Scratch::new("stable-second");
    let resaved = Scratch::new("stable-resaved");

    recorder.save(first.path()).expect("save");
    recorder.save(second.path()).expect("save");
    let bytes = fs::read(first.path()).expect("read first");
    assert_eq!(
        bytes,
        fs::read(second.path()).expect("read second"),
        "two saves of one recording must be identical"
    );

    let loaded = Recording::load(first.path()).expect("load");
    assert_eq!(loaded.version(), VERSION);
    assert_eq!(loaded.seed(), 0x0102_0304);
    assert_eq!(loaded.screen(), (1920, 1080));
    assert_eq!(loaded.started_at_ms(), recorder.recording().started_at_ms());
    assert_eq!(loaded.entries(), recorder.recording().entries());
    assert_eq!(loaded.len(), 9);

    // A save of the loaded copy must not add, drop or reorder a single byte —
    // that is what keeps a checked-in regression fixture reviewable.
    loaded.save(resaved.path()).expect("resave");
    assert_eq!(
        bytes,
        fs::read(resaved.path()).expect("read resaved"),
        "load then save must reproduce the file exactly"
    );
}

#[test]
fn the_json_shape_is_what_other_languages_read() {
    let recorder = full_recording();
    let path = Scratch::new("shape");
    recorder.save(path.path()).expect("save");
    let text = fs::read_to_string(path.path()).expect("read");
    let value: serde_json::Value = serde_json::from_str(&text).expect("json");

    assert_eq!(value["version"], VERSION);
    assert_eq!(value["seed"], 0x0102_0304u64);
    assert_eq!(value["screen"][0], 1920);
    assert_eq!(value["screen"][1], 1080);
    assert_eq!(value["entries"][0]["at_ms"], 0);
    assert_eq!(value["entries"][0]["op"], "move_raw");
    assert_eq!(value["entries"][0]["dx"], 40);
    assert_eq!(value["entries"][1]["op"], "key");
    assert_eq!(value["entries"][1]["page"], 7);
    assert_eq!(value["entries"][1]["down"], true);
    assert_eq!(value["entries"][3]["op"], "wheel");
    assert_eq!(value["entries"][3]["dx"], -40);
    assert_eq!(value["entries"][6]["op"], "barrier");
    assert_eq!(value["entries"][7]["op"], "wait");
    assert_eq!(value["entries"][8]["op"], "release_all");
}

#[test]
fn playback_keeps_entry_order_at_double_speed() {
    // Three entries 20 ms apart: 40 ms of recording, 20 ms of playback.
    let mut recorder = Recorder::new(11, (1920, 1080));
    recorder.record_at(
        0,
        Op::Key {
            page: 7,
            usage: 0x04,
            down: true,
        },
    );
    recorder.record_at(
        20,
        Op::Key {
            page: 7,
            usage: 0x04,
            down: false,
        },
    );
    recorder.record_at(40, Op::Wheel { dx: 1, dy: 120 });
    let recording = recorder.into_recording();

    let (mut device, handle) = open();
    let before = frames(&handle).len();
    let outcome = recording.play(&mut device, 2.0, &NoCancel).expect("play");
    assert_eq!(outcome.played, 3);
    assert_eq!(outcome.skipped, 0);
    assert!(outcome.is_complete());

    let played = frames(&handle)[before..].to_vec();
    let order: Vec<u8> = played.iter().map(|(opcode, _)| *opcode).collect();
    assert_eq!(order, vec![v3::KEY, v3::KEY, v3::WHEEL], "entry order");
    assert_eq!(played[0].1[4], 1, "first key goes down");
    assert_eq!(played[1].1[4], 0, "second key comes up");

    // The last entry is due at 20 ms; a replay that ignored `at_ms` would be
    // done in about a millisecond, and 2.0 must not stretch past the recording.
    assert!(
        outcome.elapsed_ms >= 15,
        "gaps were not honoured: {outcome:?}"
    );
    assert!(outcome.elapsed_ms < 200, "way past schedule: {outcome:?}");
}

#[test]
fn speed_scales_the_whole_timeline() {
    // 240 ms of recording with two 120 ms gaps: the quantum of a Windows sleep
    // is far below that, so the comparison is stable on a busy machine.
    let mut recorder = Recorder::new(12, (1920, 1080));
    recorder.record_at(0, Op::Barrier);
    recorder.record_at(120, Op::Barrier);
    recorder.record_at(240, Op::Barrier);
    let recording = recorder.into_recording();

    let (mut plain_device, _plain) = open();
    let plain_started = Instant::now();
    let plain = recording
        .play(&mut plain_device, 1.0, &NoCancel)
        .expect("1x");
    let plain_wall = plain_started.elapsed();

    let (mut fast_device, _fast) = open();
    let fast_started = Instant::now();
    let fast = recording
        .play(&mut fast_device, 4.0, &NoCancel)
        .expect("4x");
    let fast_wall = fast_started.elapsed();

    assert_eq!(plain.played, 3);
    assert_eq!(fast.played, 3);
    assert!(
        plain_wall >= Duration::from_millis(180),
        "1x playback must wait out 240 ms, took {plain_wall:?}"
    );
    assert!(
        fast_wall <= Duration::from_millis(180),
        "4x playback of 240 ms must fit in well under the recording, took {fast_wall:?}"
    );
}

#[test]
fn cancel_stops_playback_and_reports_the_remainder() {
    let mut recorder = Recorder::new(13, (1920, 1080));
    for index in 0..8u64 {
        recorder.record_at(index * 80, Op::Barrier);
    }
    let recording = recorder.into_recording();

    let (device, handle) = open();
    let flag = Arc::new(AtomicBool::new(false));
    let token = Flag(flag.clone());
    let worker = std::thread::spawn(move || {
        let mut device = device;
        recording.play(&mut device, 1.0, &token)
    });
    // 8 entries * 80 ms = 560 ms; flipping at 200 ms is safely mid-playback.
    std::thread::sleep(Duration::from_millis(200));
    flag.store(true, Ordering::Relaxed);

    let started = Instant::now();
    let outcome = worker.join().expect("playback thread").expect("playback");
    assert!(outcome.skipped > 0, "cancel was ignored: {outcome:?}");
    assert!(outcome.played > 0, "cancel fired too early: {outcome:?}");
    assert!(outcome.played < 8, "playback ran to the end: {outcome:?}");
    assert_eq!(outcome.played + outcome.skipped, 8);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "playback did not stop on the token"
    );

    // One barrier per played entry, and nothing after the stop.
    let barriers = handle
        .writes()
        .iter()
        .filter(|packet| packet.opcode == v3::BARRIER)
        .count();
    assert_eq!(barriers, outcome.played, "no frame after the cancel");
    // The stopped macro lets go of whatever it was holding.
    assert!(
        handle
            .writes()
            .iter()
            .any(|packet| packet.opcode == v3::RELEASE_ALL),
        "a cancelled playback must release held input"
    );
}

#[test]
fn a_recording_from_another_version_is_refused() {
    let path = Scratch::new("version");
    for version in [0, 2, 99] {
        fs::write(
            path.path(),
            format!(
                "{{\"version\":{version},\"seed\":1,\"screen\":[1,1],\
                 \"started_at_ms\":0,\"entries\":[]}}"
            ),
        )
        .expect("write");
        let error = Recording::load(path.path()).expect_err("must refuse");
        assert_eq!(error.code(), ErrorCode::Invalid, "{error}");
        assert!(error.message.contains("version"), "{}", error.message);
    }

    // No version at all is not a recording either.
    fs::write(path.path(), "{\"seed\":1,\"entries\":[]}").expect("write");
    let error = Recording::load(path.path()).expect_err("must refuse");
    assert_eq!(error.code(), ErrorCode::Invalid, "{error}");

    // An unknown op inside a v1 file is refused at load, not at playback.
    fs::write(
        path.path(),
        "{\"version\":1,\"seed\":1,\"screen\":[1,1],\"started_at_ms\":0,\
         \"entries\":[{\"at_ms\":0,\"op\":\"teleport\"}]}",
    )
    .expect("write");
    let error = Recording::load(path.path()).expect_err("must refuse");
    assert_eq!(error.code(), ErrorCode::Invalid, "{error}");
}

#[test]
fn a_replay_writes_the_same_frames_as_the_live_session() {
    let (device, live) = open();
    let mut recorder = Recorder::new(0xDEAD_BEEF, (1920, 1080));
    let keyboard = device.keyboard();
    let mouse = device.mouse();

    keyboard.key_down(7, 0x04).expect("key down");
    recorder.record(Op::Key {
        page: 7,
        usage: 0x04,
        down: true,
    });
    mouse.move_raw(300, -120).expect("raw move");
    recorder.record(Op::MoveRaw { dx: 300, dy: -120 });
    mouse.button(Button::Left, true).expect("button");
    recorder.record(Op::Buttons { bits: 1 });
    mouse.wheel(120, -40).expect("wheel");
    // Op::Wheel is (horizontal, vertical); the wire order is vertical first.
    recorder.record(Op::Wheel { dx: -40, dy: 120 });
    device.barrier().expect("barrier");
    recorder.record(Op::Barrier);
    mouse
        .move_by(200, 150, MotionOptions::linear(0))
        .expect("move by");
    recorder.record(Op::MoveBy { dx: 200, dy: 150 });
    keyboard.key_up(7, 0x04).expect("key up");
    recorder.record(Op::Key {
        page: 7,
        usage: 0x04,
        down: false,
    });
    mouse.release_all().expect("release");
    recorder.record(Op::ReleaseAll);

    let path = Scratch::new("frames");
    recorder.save(path.path()).expect("save");
    let loaded = Recorder::load(path.path()).expect("load");
    assert_eq!(loaded.len(), 8);

    let (mut replay_device, replay) = open();
    let outcome = loaded
        .play(&mut replay_device, 10.0, &NoCancel)
        .expect("replay");
    assert_eq!(outcome.played, 8);
    assert_eq!(outcome.skipped, 0);

    let live_frames = frames(&live);
    assert!(!live_frames.is_empty());
    assert_eq!(
        live_frames,
        frames(&replay),
        "replay must put the same opcodes and payloads on the wire"
    );
}
