//! Two controllers on one board.
//!
//! The device has a single IN queue and no addressing, so two programs
//! occasionally take each other's replies. These tests put two devices on one
//! board and pin down what that collision costs: in shared mode one `Busy` error
//! naming the command, a counter that says it happened, a hook that sees the
//! frame that was not ours — and no CLOSE, no OPEN, no dead session. The same
//! collision in exclusive mode is pinned down next to it, so the difference is
//! asserted rather than described.

use gdb::device::Target;
use gdb::{Device, DeviceOptions, ErrorCode, Ownership, Transport};
use input_protocol::v3::{self, InputState, Packet};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The session another process would be holding. Any id but ours will do; what
/// matters is that the frame on the queue is addressed to somebody else.
const THIEF: u32 = 0xF00D_BEEF;

#[derive(Default)]
struct Board {
    writes: Vec<Packet>,
    queue: VecDeque<[u8; 64]>,
    /// While set, every reply is addressed to this session and the asker never
    /// sees its own: a reply taken by another owner.
    thief: Option<u32>,
    /// The protocol result byte to answer with, once.
    fault: Option<u8>,
}

impl Board {
    fn opcodes(&self, wanted: u8) -> usize {
        self.writes
            .iter()
            .filter(|packet| packet.opcode == wanted)
            .count()
    }
}

type Shared = Arc<Mutex<Board>>;

fn board() -> Shared {
    Arc::new(Mutex::new(Board::default()))
}

/// Feature 14: the firmware contract every session reads before OPEN.
fn feature_report() -> [u8; 64] {
    let mut report = [0u8; 64];
    report[0] = 14;
    report[1..5].copy_from_slice(b"FMT3");
    report[5] = 3;
    report[6] = 2;
    report[7] = 3;
    report[8] = 2;
    report[9] = 0;
    report[10..12].copy_from_slice(&2000u16.to_le_bytes());
    report[14] = 1;
    report[15..19].copy_from_slice(&15u32.to_le_bytes());
    for entry in input_protocol::KEYS {
        if entry.id.usage_page == 7 && entry.id.usage < 224 {
            report[19 + entry.id.usage as usize / 8] |= 1 << (entry.id.usage % 8);
        }
    }
    report[47] = 0xFF;
    report[48] = 0x7F;
    report[49..57].copy_from_slice(&[0x42, 0x50, 0x33, 0x30, 0x34, 0x42, 0x38, 0x33]);
    report[57..61].copy_from_slice(&0x0DB0_0001u32.to_le_bytes());
    report
}

fn reply(opcode: u8, session: u32, sequence: u32, payload: &[u8]) -> [u8; 64] {
    let mut report = [0u8; 64];
    report[0] = v3::IN_ID;
    report[1] = 3;
    report[2] = opcode | 0x80;
    report[3] = payload.len() as u8;
    report[5..9].copy_from_slice(&session.to_le_bytes());
    report[9..13].copy_from_slice(&sequence.to_le_bytes());
    report[13..13 + payload.len()].copy_from_slice(payload);
    report
}

fn status_body(sequence: u32) -> Vec<u8> {
    let mut body = [0u8; 51];
    body[1..5].copy_from_slice(&sequence.to_le_bytes());
    body[5..9].copy_from_slice(&sequence.to_le_bytes());
    body[10..12].copy_from_slice(&2000u16.to_le_bytes());
    body[20..].copy_from_slice(&InputState::default().encode());
    body.to_vec()
}

/// One transport over the shared board: each device talks to the same device, so
/// a reply one of them never sees is gone.
struct BoardTransport {
    board: Shared,
}

impl Transport for BoardTransport {
    fn feature(&mut self) -> gdb::Result<Option<[u8; 64]>> {
        Ok(Some(feature_report()))
    }

    fn write(&mut self, report: &[u8; 64]) -> gdb::Result<()> {
        let packet = Packet::decode(report, v3::OUT_ID)
            .map_err(|error| gdb::Error::new(ErrorCode::Framing, error.to_string()))?;
        let mut board = self.board.lock().unwrap();
        board.writes.push(packet.clone());
        let expects_reply = matches!(
            packet.opcode,
            v3::OPEN
                | v3::CLOSE
                | v3::HEARTBEAT
                | v3::STATUS
                | v3::BARRIER
                | v3::RELEASE_ALL
                | v3::STATUS_EXT
                | v3::SESSION_LIST
        );
        if !expects_reply {
            return Ok(());
        }
        if let Some(code) = board.fault.take() {
            let queued = reply(packet.opcode, packet.session, packet.sequence, &[code]);
            board.queue.push_back(queued);
            return Ok(());
        }
        if let Some(thief) = board.thief {
            let stolen = reply(packet.opcode, thief, packet.sequence, &[0]);
            board.queue.push_back(stolen);
            return Ok(());
        }
        let payload = match packet.opcode {
            v3::STATUS => status_body(packet.sequence),
            _ => vec![0u8],
        };
        let queued = reply(packet.opcode, packet.session, packet.sequence, &payload);
        board.queue.push_back(queued);
        Ok(())
    }

    fn read(&mut self, _timeout_ms: i32) -> gdb::Result<Option<[u8; 64]>> {
        Ok(self.board.lock().unwrap().queue.pop_front())
    }
}

fn options(timeout_ms: u32, heartbeat: bool) -> DeviceOptions {
    DeviceOptions {
        auto_reconnect: true,
        timeout_ms,
        heartbeat,
    }
}

fn open(board: &Shared, ownership: Ownership) -> Device {
    Device::over_transport(
        Box::new(BoardTransport {
            board: board.clone(),
        }),
        Target::Mock,
        options(120, false),
        ownership,
    )
    .expect("the board answers Feature 14 and OPEN")
}

#[test]
fn a_stolen_reply_costs_one_busy_and_the_session_survives() {
    let board = board();
    let first = open(&board, Ownership::Shared);
    // The second controller on the same board, exactly as the tool would see it:
    // two sessions, one queue.
    let second = open(&board, Ownership::Shared);
    assert_eq!(board.lock().unwrap().opcodes(v3::OPEN), 2);

    let seen: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    first.set_stray_hook(Arc::new(move |report: &[u8]| {
        sink.lock().unwrap().push(report.to_vec());
    }));

    // From here the board answers somebody else.
    board.lock().unwrap().thief = Some(THIEF);
    let error = first.status().expect_err("the reply went elsewhere");

    assert_eq!(error.code(), ErrorCode::Busy);
    assert!(
        error.message.contains("STATUS"),
        "the message names the command: {}",
        error.message
    );
    assert!(
        error.message.contains("another process may have taken it"),
        "the message says what happened: {}",
        error.message
    );

    // One error, and nothing on the wire: no CLOSE, no second OPEN.
    assert_eq!(board.lock().unwrap().opcodes(v3::OPEN), 2);
    assert_eq!(board.lock().unwrap().opcodes(v3::CLOSE), 0);
    assert_eq!(first.metrics().reconnects, 0);
    assert_eq!(first.state(), gdb::ConnectionState::Connected);

    // The frame that was not ours was counted and handed over.
    assert_eq!(first.stolen_frames(), 1);
    assert!(
        first.metrics().retries >= 1,
        "the loop looked past one frame"
    );
    let frames = seen.lock().unwrap().clone();
    assert_eq!(frames.len(), 1, "the hook saw the frame that was not ours");
    let raw: [u8; 64] = frames[0].as_slice().try_into().expect("64 bytes");
    let stolen = Packet::decode(&raw, v3::IN_ID).expect("the hook gets the raw report");
    assert_eq!(stolen.session, THIEF);

    // And the session is still good: the board answers the next command.
    board.lock().unwrap().thief = None;
    first
        .status()
        .expect("the same session answers once the board does");
    assert_eq!(board.lock().unwrap().opcodes(v3::CLOSE), 0);
    drop(second);
}

#[test]
fn the_same_collision_is_a_timeout_when_the_device_is_exclusive() {
    let board = board();
    let device = open(&board, Ownership::Exclusive);
    board.lock().unwrap().thief = Some(THIEF);

    let error = device.status().expect_err("the reply went elsewhere");
    assert_eq!(
        error.code(),
        ErrorCode::Timeout,
        "exclusive mode still reports the plain timeout"
    );
    assert!(
        !error.message.contains("another process"),
        "only a shared session talks about the other owner: {}",
        error.message
    );
    // The frame was still not ours and is still counted; only the answer a host
    // gets differs.
    assert_eq!(device.stolen_frames(), 1);
}

#[test]
fn shared_never_reopens_where_exclusive_heals_itself() {
    // The same fault on both: the device answers STATUS with a lease fault.
    let exclusive_board = board();
    let exclusive = open(&exclusive_board, Ownership::Exclusive);
    exclusive_board.lock().unwrap().fault = Some(5);
    exclusive
        .status()
        .expect("exclusive reopens and asks again");
    assert_eq!(exclusive.metrics().reconnects, 1);
    assert_eq!(
        exclusive_board.lock().unwrap().opcodes(v3::CLOSE),
        1,
        "the reopen closes the session it gave up on"
    );

    let shared_board = board();
    let shared = open(&shared_board, Ownership::Shared);
    shared_board.lock().unwrap().fault = Some(5);
    let error = shared
        .status()
        .expect_err("a shared session reports it once");
    assert_eq!(error.code(), ErrorCode::Lease);
    assert_eq!(shared.metrics().reconnects, 0);
    assert_eq!(
        shared_board.lock().unwrap().opcodes(v3::CLOSE),
        0,
        "no CLOSE, so no storm for the other owners"
    );
    assert_eq!(shared.state(), gdb::ConnectionState::Connected);

    // The host decides: the board answers again, and the session was never lost.
    shared.status().expect("the same session is still usable");
}

#[test]
fn the_stray_hook_can_be_cleared() {
    let board = board();
    let device = open(&board, Ownership::Shared);
    let count = Arc::new(AtomicU64::new(0));
    let seen = count.clone();
    device.set_stray_hook(Arc::new(move |_report: &[u8]| {
        seen.fetch_add(1, Ordering::Relaxed);
    }));

    board.lock().unwrap().thief = Some(THIEF);
    let _ = device.status();
    assert_eq!(count.load(Ordering::Relaxed), 1);

    device.clear_stray_hook();
    let _ = device.status();
    assert_eq!(
        count.load(Ordering::Relaxed),
        1,
        "cleared hooks stay silent"
    );
    assert_eq!(device.stolen_frames(), 2, "the counter keeps counting");
}

#[test]
fn a_stolen_heartbeat_retries_and_never_closes() {
    let board = board();
    let device = Device::over_transport(
        Box::new(BoardTransport {
            board: board.clone(),
        }),
        Target::Mock,
        options(120, true),
        Ownership::Shared,
    )
    .expect("open");

    let opened = board.lock().unwrap().opcodes(v3::OPEN);
    board.lock().unwrap().thief = Some(THIEF);

    // One lease period is 2 s and a beat is due every 500 ms, so two beats fit
    // in this window: the first is taken, the second proves the retry.
    let deadline = Instant::now() + Duration::from_millis(2000);
    while Instant::now() < deadline && board.lock().unwrap().opcodes(v3::HEARTBEAT) < 2 {
        std::thread::sleep(Duration::from_millis(20));
    }

    let board = board.lock().unwrap();
    assert!(
        board.opcodes(v3::HEARTBEAT) >= 2,
        "a lost beat is retried on the next tick"
    );
    assert_eq!(board.opcodes(v3::OPEN), opened, "no OPEN");
    assert_eq!(board.opcodes(v3::CLOSE), 0, "no CLOSE");
    drop(board);
    assert_eq!(device.metrics().reconnects, 0);
    assert_eq!(device.state(), gdb::ConnectionState::Connected);
}

#[test]
fn a_shared_device_reconnects_only_when_the_host_asks() {
    let board = board();
    let device = open(&board, Ownership::Shared);
    board.lock().unwrap().thief = Some(THIEF);
    let _ = device.status();
    assert_eq!(device.metrics().reconnects, 0);

    // The host knows the board is its own again and says so.
    device.reconnect().expect("reconnect");
    assert_eq!(device.metrics().reconnects, 1);
    assert_eq!(board.lock().unwrap().opcodes(v3::CLOSE), 1);
}
