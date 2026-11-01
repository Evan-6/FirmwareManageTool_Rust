//! Counters and the latency histogram, driven through the in-memory device.

use gdb::metrics::{Metrics, MetricsSnapshot};
use gdb::{DeviceOptions, ErrorCode, Manager};

/// Heartbeat off: it would write frames behind the assertions.
fn options() -> DeviceOptions {
    DeviceOptions {
        auto_reconnect: true,
        timeout_ms: 100,
        heartbeat: false,
    }
}

#[test]
fn every_answered_command_is_one_frame_and_one_round_trip() {
    let (device, _handle) = Manager::open_mock_with_handle(options()).expect("mock device");
    let before = device.metrics();
    for _ in 0..8 {
        device.barrier().expect("barrier");
    }
    let after = device.metrics();

    assert_eq!(after.frames_sent - before.frames_sent, 8);
    assert_eq!(after.frames_received - before.frames_received, 8);
    assert_eq!(after.latency.count - before.latency.count, 8);
    assert_eq!(after.timeouts, before.timeouts);
    assert_eq!(after.reconnects, before.reconnects);

    // A mock answers instantly, so the histogram must be internally ordered
    // even when every sample lands in the first few buckets.
    assert!(after.latency.min_us <= after.latency.p50_us);
    assert!(after.latency.p50_us <= after.latency.p95_us);
    assert!(after.latency.p95_us <= after.latency.max_us);
    assert!(
        after.latency.max_us > 0 && after.latency.max_us < 50_000,
        "{:?}",
        after.latency
    );
}

#[test]
fn input_commands_are_counted_but_not_timed() {
    // A fire-and-forget input command never waits for a reply, so it must not
    // appear in the histogram — that is the frame the mouse path sends.
    let (device, _handle) = Manager::open_mock_with_handle(options()).expect("mock device");
    let before = device.metrics();
    device.mouse().move_raw(10, 0).expect("move");
    device.mouse().move_raw(-10, 0).expect("move");
    let after = device.metrics();

    assert_eq!(after.frames_sent - before.frames_sent, 2);
    assert_eq!(after.latency.count, before.latency.count);
    assert_eq!(after.frames_received, before.frames_received);
}

#[test]
fn a_reply_that_never_arrives_is_a_timeout() {
    let (device, handle) = Manager::open_mock_with_handle(options()).expect("mock device");
    handle.set_silent(true);
    let before = device.metrics();

    let error = device.status().expect_err("a silent device cannot answer");
    assert_eq!(error.code(), ErrorCode::Timeout);

    let after = device.metrics();
    assert_eq!(after.timeouts - before.timeouts, 1);
    assert_eq!(
        after.latency.count, before.latency.count,
        "a timeout has no round trip to time"
    );
}

#[test]
fn reconnecting_is_counted() {
    let (device, _handle) = Manager::open_mock_with_handle(options()).expect("mock device");
    let before = device.metrics();
    device.reconnect().expect("reconnect");
    assert_eq!(device.metrics().reconnects - before.reconnects, 1);
}

#[test]
fn an_automatic_recovery_is_counted_as_a_reconnect() {
    // The SDK reopens a lost transport on its own; a host reading `reconnects`
    // must see that too, not just the manual ones.
    let (device, handle) = Manager::open_mock_with_handle(options()).expect("mock device");
    let written = handle.writes().len();
    let before = device.metrics();

    handle.set_disconnect_after(Some(written));
    device.barrier().expect("the SDK reopens and retries once");

    let after = device.metrics();
    assert_eq!(after.reconnects - before.reconnects, 1);
    assert!(after.frames_sent > before.frames_sent, "the retry went out");
}

#[test]
fn reset_metrics_zeroes_the_snapshot() {
    let (device, _handle) = Manager::open_mock_with_handle(options()).expect("mock device");
    device.barrier().expect("barrier");
    device.status().expect("status");
    assert!(device.metrics().frames_sent > 0);

    device.reset_metrics();
    assert_eq!(device.metrics(), MetricsSnapshot::default());
}

#[test]
fn percentiles_are_sane_for_a_synthetic_histogram() {
    // 1..=100 µs: below the linear cut-off, so every value is exact and the
    // percentile is the rank itself.
    let metrics = Metrics::new();
    for micros in 1..=100 {
        metrics.observe_latency(micros);
    }
    let stats = metrics.snapshot().latency;
    assert_eq!(stats.count, 100);
    assert_eq!(stats.min_us, 1);
    assert_eq!(stats.max_us, 100);
    assert!(
        (45..=55).contains(&stats.p50_us),
        "p50 was {}",
        stats.p50_us
    );
    assert!(
        (90..=100).contains(&stats.p95_us),
        "p95 was {}",
        stats.p95_us
    );

    // Milli-second round trips are still told apart, within one bucket.
    let coarse = Metrics::new();
    for _ in 0..20 {
        coarse.observe_latency(1_500);
    }
    let stats = coarse.snapshot().latency;
    assert_eq!(stats.count, 20);
    assert!(
        (1_400..=1_600).contains(&stats.p50_us),
        "1.5 ms landed on {}",
        stats.p50_us
    );
    assert_eq!(stats.p50_us, stats.p95_us);
    assert!(stats.min_us <= 1_500 && 1_500 <= stats.max_us);
}

#[test]
fn an_empty_histogram_reports_zeros() {
    let metrics = Metrics::new();
    assert_eq!(metrics.snapshot(), MetricsSnapshot::default());
    metrics.reset();
    assert_eq!(metrics.snapshot(), MetricsSnapshot::default());
}
