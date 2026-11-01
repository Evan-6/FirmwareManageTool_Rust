//! Adapter tests: what the tool promises the worker, the UI and the latency
//! tester, checked against the SDK's in-memory device.
//!
//! The session contract itself (short writes, foreign owners' frames, faults,
//! timeouts) is tested where it lives now: `shared/input-protocol/src/client.rs`.

use super::*;
use gdb::mock::MockHandle;

/// The packet log is process-wide, so the tests that read it must not overlap.
static LOG_TEST: Mutex<()> = Mutex::new(());

fn mock(trace: bool) -> (Session, MockHandle) {
    let (device, handle) = gdb::Manager::open_mock_with_handle(gdb::DeviceOptions {
        auto_reconnect: false,
        timeout_ms: 500,
        heartbeat: false,
    })
    .unwrap();
    let mut session = Session::new(device);
    if trace {
        session.set_trace(true);
    }
    (session, handle)
}

#[test]
fn status_fields_keep_the_ui_contract() {
    let (mut session, _handle) = mock(false);
    let cancel = Cancellation::default();
    session
        .key(
            input_protocol::KeyId::from_token("a").unwrap(),
            true,
            &cancel,
        )
        .unwrap();
    let status = session.status(&cancel).unwrap();
    for name in [
        "p",
        "rx",
        "tx",
        "hid",
        "fs",
        "received",
        "completed",
        "pending",
        "lease",
        "mb",
        "keys",
    ] {
        assert!(status.fields.contains_key(name), "缺少欄位 {name}");
    }
    assert_eq!(status.count("p"), 1);
    assert_eq!(status.fields["keys"], "a");
    assert!(!status.released());

    session.set_button(1, true, &cancel).unwrap();
    let status = session.status(&cancel).unwrap();
    assert_eq!(status.buttons(), 1);
    assert_eq!(status.fields["mb"], "01");
    session.set_button(1, false, &cancel).unwrap();
    assert_eq!(session.status(&cancel).unwrap().buttons(), 0);

    // RELEASE_ALL clears keys and buttons together.
    session
        .key(
            input_protocol::KeyId::from_token("a").unwrap(),
            true,
            &cancel,
        )
        .unwrap();
    session.reset(&cancel).unwrap();
    assert!(session.status(&cancel).unwrap().released());
}

#[test]
fn hello_comes_from_the_feature14_identity() {
    let (mut session, _handle) = mock(false);
    let cancel = Cancellation::default();
    let hello = session.hello(&cancel).unwrap();
    assert_eq!(hello.protocol, 3);
    assert!(hello.nkro, "mock reports NKRO");
    assert!(hello.media, "mock reports Consumer");
    assert!(hello.mouse, "mock reports relative mouse");
    assert_eq!(hello.lease_ms, 2000);
    assert_eq!(hello.firmware, "3.2.0");
    let info = session.info().unwrap();
    assert_eq!(info.board, 2);
    assert_eq!(info.lease_ms, 2000);
}

#[test]
fn typed_queries_parse_the_sdk_replies() {
    let (mut session, _handle) = mock(false);
    let cancel = Cancellation::default();
    assert!(!session.sessions(&cancel).unwrap().is_empty());
    assert_eq!(session.status_ext(&cancel).unwrap().build, 0x0DB0_0001);
    let reply = session
        .query(input_protocol::v3::Command::Barrier, &cancel)
        .unwrap();
    assert_eq!(reply.opcode, input_protocol::v3::BARRIER | 128);
}

#[test]
fn close_frees_the_owner_slot_and_stops_the_session() {
    let (mut session, handle) = mock(false);
    let cancel = Cancellation::default();
    session
        .key(
            input_protocol::KeyId::from_token("a").unwrap(),
            true,
            &cancel,
        )
        .unwrap();
    session.close(&cancel).unwrap();
    assert!(handle.is_closed());
    assert!(handle.sessions().is_empty());
    assert!(
        handle.held().pressed().is_empty(),
        "CLOSE releases our keys"
    );
    assert!(!session.is_valid());
    // A closed session refuses further work instead of writing into the void.
    assert!(session.status(&cancel).is_err());
    assert!(session.poll().is_err());
}

#[test]
fn cancelling_before_a_call_sends_nothing() {
    let (mut session, handle) = mock(false);
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(
        session
            .key(
                input_protocol::KeyId::from_token("a").unwrap(),
                true,
                &cancel
            )
            .is_err()
    );
    assert!(session.status(&cancel).is_err());
    assert!(
        !handle
            .writes()
            .iter()
            .any(|packet| packet.opcode == input_protocol::v3::KEY),
        "取消後不得送出輸入"
    );
}

#[test]
fn trace_events_capture_host_and_device_frames() {
    let _guard = LOG_TEST
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (mut session, _handle) = mock(false);
    let cancel = Cancellation::default();
    session.set_trace(true);
    // BARRIER both ways: a clean STATUS poll is deliberately filtered out.
    session.finish(&cancel).unwrap();
    let events = session.take_events();
    session.set_trace(false);
    assert!(
        events
            .iter()
            .any(|event| event.line.starts_with("→ BARRIER")),
        "缺少送出的封包：{events:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| event.line.starts_with("← BARRIER")),
        "缺少收到的封包"
    );
    assert!(events.iter().all(|event| event.unix_ms > 0));
    // Draining is not asserted: the queue is process-wide, and other tests in
    // this binary do device I/O while tracing is on. The worker is the only
    // session in the real process, which is why one queue is the right shape
    // there.
}

fn counters(hid: u64, tx: u64, failsafe: u64) -> crate::protocol::Status {
    let fields = [("hid", hid), ("tx", tx), ("fs", failsafe)]
        .into_iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    crate::protocol::Status {
        fields,
        raw: String::new(),
    }
}

#[test]
fn peer_lease_expiry_does_not_fail_a_measurement() {
    let before = counters(0, 0, 0);
    // An expired lease in another owner's session raises the device-global
    // failsafe counter only; our own transport counters are what condemn a
    // measurement. (That a foreign EVENT does not fault this session is covered
    // in shared/input-protocol/src/client.rs.)
    assert!(counters(0, 0, 1).check_transport_since(&before).is_ok());
    assert!(counters(0, 1, 0).check_transport_since(&before).is_err());
    assert!(counters(1, 0, 0).check_transport_since(&before).is_err());
}

#[test]
fn the_packet_log_reads_like_sentences_and_drops_the_noise() {
    let _guard = LOG_TEST
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _ = EVENTS.lock().map(|mut events| events.clear());
    // Heartbeats and clean polling are noise; a reply with counters is a line.
    record(
        gdb::Level::Trace,
        "out opcode=3 session=1 sequence=4 payload=[]",
    );
    record(
        gdb::Level::Trace,
        "in opcode=134 session=1 sequence=5 payload=[0,143,0,0,0,143,0,0,0,0,208,7,0,0,0,0,0,0,0,0,0,0]",
    );
    record(
        gdb::Level::Trace,
        "in opcode=134 session=1 sequence=6 payload=[5,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]",
    );
    record(
        gdb::Level::Trace,
        "out opcode=16 session=1 sequence=7 payload=[7,0,4,0,1]",
    );
    let lines: Vec<String> = EVENTS
        .lock()
        .map(|mut events| events.drain(..).map(|event| event.line).collect())
        .unwrap_or_default();

    assert!(
        !lines.iter().any(|line| line.contains("心跳")),
        "heartbeats must not reach the log: {lines:?}"
    );
    assert!(
        !lines.iter().any(|line| line.contains("完成 143/143")),
        "a clean poll is noise, not a log line: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("錯誤碼 5（租約到期）")),
        "a failed reply must keep its fault: {lines:?}"
    );
    let key = lines
        .iter()
        .find(|line| line.contains("按鍵"))
        .unwrap_or_else(|| panic!("no readable key line in {lines:?}"));
    assert!(key.contains("page 7 usage 4 按下"), "{key}");
    assert!(!key.contains('['), "raw payload leaked: {key}");
}
