//! Presence probing: Feature 14 only, no session, no packet.
//!
//! Everything here runs against the in-memory transport, because the contract is
//! about what the SDK does *not* send rather than about a board answering. The
//! parts that need hardware are named in the report, not faked here.

use gdb::Manager;
use gdb::error::ErrorCode;
use gdb::mock::{MockHandle, MockTransport};
use gdb::probe::{self, Observer, Probe, ProbeEvent};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// A probe over the mock, plus the handle that counts what the probe sent.
fn mock_probe() -> (Probe, MockHandle) {
    let (transport, handle) = MockTransport::new();
    let probe = probe::read_from(Box::new(transport), "MOCK", "GDBOX mock").expect("feature reply");
    (probe, handle)
}

#[test]
fn a_feature_report_parses_every_field_and_writes_nothing() {
    let (probe, handle) = mock_probe();

    assert_eq!(probe.serial, "MOCK");
    assert_eq!(probe.product, "GDBOX mock");
    assert_eq!(probe.board, 2);
    assert_eq!(probe.firmware, "3.2.0");
    assert_eq!(probe.lease_ms, 2000);
    assert_eq!(probe.poll_ms, 1);
    assert_eq!(probe.capabilities, 15);
    assert_eq!(
        probe.device_id,
        [0x42, 0x50, 0x33, 0x30, 0x34, 0x42, 0x38, 0x33]
    );
    assert_eq!(probe.build, 0x0DB0_0001);
    assert_eq!(probe.label(), "MOCK");

    // The proof: a probe is a control transfer. Not one output report left the
    // host, so another program's IN queue is untouched.
    assert!(
        handle.writes().is_empty(),
        "a probe wrote {} packets",
        handle.writes().len()
    );

    // `info()` is the shape `Device::info()` returns — the same struct.
    let info = probe.info();
    assert_eq!(info.serial, probe.serial);
    assert_eq!(info.product, probe.product);
    assert_eq!(info.board, probe.board);
    assert_eq!(info.firmware, probe.firmware);
    assert_eq!(info.lease_ms, probe.lease_ms);
    assert_eq!(info.poll_ms, probe.poll_ms);
    assert_eq!(info.capabilities, probe.capabilities);
    assert_eq!(info.device_id, probe.device_id);
    assert_eq!(info.build, probe.build);
}

#[test]
fn probing_does_not_open_a_session() {
    let (transport, handle) = MockTransport::new();
    let probe = probe::read_from(Box::new(transport), "MOCK", "GDBOX mock").expect("feature reply");
    assert_eq!(probe.board, 2);
    // No OPEN, therefore no owner slot taken: OPEN is what claims one.
    assert!(handle.writes().is_empty());
    assert_eq!(handle.session(), 0, "a probe must not become an owner");
    assert!(handle.sessions().is_empty());
}

#[test]
fn a_board_that_does_not_answer_is_disconnected() {
    let (transport, handle) = MockTransport::new();
    handle.set_feature_missing(true);
    let error = probe::read_from(Box::new(transport), "MOCK", "GDBOX mock")
        .expect_err("no feature reply must fail");
    assert_eq!(error.code(), ErrorCode::Disconnected);
    assert!(error.message.contains("Feature 14"), "{}", error.message);
    assert!(handle.writes().is_empty());
}

#[test]
fn a_selector_that_matches_nothing_is_disconnected_and_does_not_panic() {
    let error = probe::read_selector(Some("no-board-with-this-serial"))
        .expect_err("an unmatched selector must fail");
    // Windows enumerates and finds nothing; a platform without a HID backend
    // cannot enumerate at all. Neither is a panic.
    assert!(
        matches!(
            error.code(),
            ErrorCode::Disconnected | ErrorCode::Unsupported
        ),
        "{error}"
    );
    assert!(!error.message.is_empty());
}

#[test]
fn the_bus_listing_probes_every_board_without_failing_on_one() {
    // No hardware in the gate: this must be an empty bus, not an error and not a
    // panic. With a board attached it is one probe per vendor collection instead.
    let probes = probe::all().expect("the bus is enumerable");
    for probe in &probes {
        assert!(!probe.firmware.is_empty());
        assert!(probe.lease_ms >= 1000);
        assert!(probe.poll_ms > 0);
    }
    // The convenience wrappers answer the same question.
    if probes.is_empty() {
        assert!(
            matches!(
                Manager::probe().expect_err("no board").code(),
                ErrorCode::Disconnected
            ),
            "an empty bus has no first board"
        );
    } else {
        assert_eq!(
            Manager::probe().expect("first board").serial,
            probes[0].serial
        );
    }
    assert_eq!(Manager::probe_all().expect("bus").len(), probes.len());
}

#[test]
fn the_observer_polls_feature_14_and_refuses_every_command() {
    let polls = Arc::new(AtomicUsize::new(0));
    let counter = polls.clone();
    let source = move || {
        counter.fetch_add(1, Ordering::Relaxed);
        let (transport, _handle) = MockTransport::new();
        probe::read_from(Box::new(transport), "MOCK", "GDBOX mock")
    };
    let events = Arc::new(Mutex::new(Vec::new()));
    let seen = events.clone();
    let mut observer = Observer::start_with(source, Duration::from_millis(30), move |event| {
        let mut seen = seen.lock().unwrap();
        match event {
            ProbeEvent::Present(probe) => seen.push(format!("present:{}", probe.firmware)),
            ProbeEvent::Gone(error) => seen.push(format!("gone:{}", error.code().as_u8())),
        }
    })
    .expect("observer");

    let deadline = Instant::now() + Duration::from_secs(5);
    while polls.load(Ordering::Relaxed) < 3 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        polls.load(Ordering::Relaxed) >= 3,
        "the observer never re-read Feature 14"
    );

    assert_eq!(observer.info().firmware, "3.2.0");
    assert_eq!(observer.probe().board, 2);
    assert_eq!(observer.state(), gdb::ConnectionState::Connected);
    assert!(observer.last_error().is_none());
    assert!(observer.is_running());
    // The same probe three times is one answer, not three events.
    assert!(
        events.lock().unwrap().is_empty(),
        "an unchanged probe must not be an event: {:?}",
        events.lock().unwrap()
    );

    // Every command is refused, and the message says why.
    let refusals = [
        observer.status().err(),
        observer.status_ext().err(),
        observer.sessions().err(),
        observer.barrier().err(),
        observer.release_all().err(),
        observer.bootloader().err(),
        observer.command(&input_protocol::v3::Command::Status).err(),
        observer.reconnect().err(),
    ];
    for refusal in refusals {
        let error = refusal.expect("an observer must refuse every command");
        assert_eq!(error.code(), ErrorCode::Unsupported);
        assert!(
            error.message.contains("Feature 14"),
            "the refusal must name what the handle does: {}",
            error.message
        );
    }

    observer.stop();
    assert!(!observer.is_running(), "stop must join the thread");
    observer.stop(); // safe twice
    assert_eq!(observer.info().firmware, "3.2.0");
}

#[test]
fn the_observer_keeps_the_last_probe_when_the_board_stops_answering() {
    let polls = Arc::new(AtomicUsize::new(0));
    let counter = polls.clone();
    let source = move || {
        if counter.fetch_add(1, Ordering::Relaxed) == 0 {
            let (transport, _handle) = MockTransport::new();
            probe::read_from(Box::new(transport), "MOCK", "GDBOX mock")
        } else {
            Err(gdb::Error::disconnected("board went away"))
        }
    };
    let gone = Arc::new(AtomicUsize::new(0));
    let seen = gone.clone();
    let mut observer = Observer::start_with(source, Duration::from_millis(20), move |event| {
        if let ProbeEvent::Gone(error) = event {
            assert_eq!(error.code(), ErrorCode::Disconnected);
            seen.fetch_add(1, Ordering::Relaxed);
        }
    })
    .expect("observer");

    let deadline = Instant::now() + Duration::from_secs(5);
    while gone.load(Ordering::Relaxed) == 0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        gone.load(Ordering::Relaxed),
        1,
        "a board that stays away is one event, not one per poll"
    );
    assert_eq!(observer.state(), gdb::ConnectionState::Disconnected);
    assert_eq!(
        observer.info().firmware,
        "3.2.0",
        "the last probe must survive the board leaving"
    );
    assert!(observer.last_error().is_some());
    observer.stop();
}

#[test]
fn an_observer_for_a_selector_that_matches_nothing_cannot_start() {
    let error = Observer::start_selector(
        Some("no-board-with-this-serial"),
        Duration::from_millis(50),
        |_| {},
    )
    .expect_err("a handle with nothing to report must not be handed out");
    assert!(
        matches!(
            error.code(),
            ErrorCode::Disconnected | ErrorCode::Unsupported
        ),
        "{error}"
    );
}

#[test]
fn the_probe_json_matches_the_device_info_json() {
    let (probe, _handle) = mock_probe();
    let (device, _mock) = Manager::open_mock_with_handle(gdb::DeviceOptions::default()).unwrap();
    let from_probe = serde_json::to_value(probe.info()).expect("probe JSON");
    let from_device = serde_json::to_value(device.info()).expect("device JSON");
    assert_eq!(from_probe, from_device, "a probe is a DeviceInfo");
    device.close().expect("close");
}
