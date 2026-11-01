//! Counters a host can read: frames, retries, lease faults, latency.
//!
//! Recording costs one relaxed atomic add and nothing else — no lock, no
//! allocation, no formatting — because the calls sit on the path a mouse report
//! takes. Reading is the expensive half: [`Metrics::snapshot`] walks the
//! histogram once, so a host may ask for statistics as often as its UI likes
//! without ever slowing the HID loop.
//!
//! Everything is per device, not per session: a reconnect adds to the same
//! counters, which is what makes `reconnects` and `lease_faults` useful.

use serde::Serialize;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Latencies below this many microseconds get one bucket each, so every round
/// trip a host can feel — sub-millisecond to a few milliseconds — is reported
/// exactly.
const LINEAR_US: usize = 4096;
/// Buckets per doubling above [`LINEAR_US`]: eight, so a coarse sample is off by
/// at most 6%.
const PER_OCTAVE: usize = 8;
/// Doublings covered above [`LINEAR_US`]: 4 KiB .. 4 MiB, then clamped.
const OCTAVES: usize = 10;
/// Largest sample the histogram can tell apart; later ones join the last bucket.
const MAX_US: u64 = (LINEAR_US as u64) << OCTAVES;
/// Fixed bucket count, so the histogram is allocated once and never moves.
pub const BUCKETS: usize = LINEAR_US + PER_OCTAVE * OCTAVES;

/// Latency statistics in microseconds.
///
/// Below 4 ms `min` and `max` are the exact samples; above it they are the edges
/// of the bucket the sample fell in. Percentiles are bucket representatives.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatencyStats {
    /// How many round trips were timed.
    pub count: u64,
    /// Fastest observed round trip.
    pub min_us: u64,
    /// Median round trip.
    pub p50_us: u64,
    /// 95th percentile round trip.
    pub p95_us: u64,
    /// Slowest observed round trip.
    pub max_us: u64,
}

/// One device's counters as a host displays them. Cheap to clone and to
/// serialize; it owns nothing shared.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricsSnapshot {
    /// Output reports written, including the handshake and the closing CLOSE.
    pub frames_sent: u64,
    /// Input reports that belonged to this device's session and were accepted.
    pub frames_received: u64,
    /// Replies the session had to look past: a read that returned nothing while
    /// the deadline was still running, or a frame for another session or
    /// sequence. The SDK never re-sends a command, so this counts read retries,
    /// not frame retransmissions.
    pub retries: u64,
    /// The subset of `retries` that was a frame for another session or another
    /// sequence: a reply this process did not send the command for. In a shared
    /// board that is the shape a collision takes — the frame was on the one IN
    /// queue and somebody else got it, or this reader got somebody else's.
    pub stolen_frames: u64,
    /// Transports reopened after a failed command or an expired lease.
    pub reconnects: u64,
    /// Commands the device answered with a lease fault.
    pub lease_faults: u64,
    /// Commands whose reply never arrived inside the timeout.
    pub timeouts: u64,
    /// Round-trip histogram; one sample per command that waited for a reply.
    pub latency: LatencyStats,
}

#[derive(Debug)]
struct Counters {
    frames_sent: AtomicU64,
    frames_received: AtomicU64,
    retries: AtomicU64,
    stolen_frames: AtomicU64,
    reconnects: AtomicU64,
    lease_faults: AtomicU64,
    timeouts: AtomicU64,
    buckets: Box<[AtomicU64]>,
}

impl Counters {
    fn new() -> Self {
        Self {
            frames_sent: AtomicU64::new(0),
            frames_received: AtomicU64::new(0),
            retries: AtomicU64::new(0),
            stolen_frames: AtomicU64::new(0),
            reconnects: AtomicU64::new(0),
            lease_faults: AtomicU64::new(0),
            timeouts: AtomicU64::new(0),
            buckets: (0..BUCKETS).map(|_| AtomicU64::new(0)).collect(),
        }
    }

    fn clear(&self) {
        // Relaxed stores: a reset that races a report may lose that report, and
        // that is the only promise worth making — it must not block the sender.
        self.frames_sent.store(0, Ordering::Relaxed);
        self.frames_received.store(0, Ordering::Relaxed);
        self.retries.store(0, Ordering::Relaxed);
        self.stolen_frames.store(0, Ordering::Relaxed);
        self.reconnects.store(0, Ordering::Relaxed);
        self.lease_faults.store(0, Ordering::Relaxed);
        self.timeouts.store(0, Ordering::Relaxed);
        for bucket in self.buckets.iter() {
            bucket.store(0, Ordering::Relaxed);
        }
    }
}

/// The recorder. One per [`Device`](crate::Device) and cloned into the session,
/// so a command can record without reaching back through the device lock.
#[derive(Clone, Debug)]
pub struct Metrics(Arc<Counters>);

impl Metrics {
    pub fn new() -> Self {
        Self(Arc::new(Counters::new()))
    }

    /// One output report left the host.
    pub fn record_frame_sent(&self) {
        self.0.frames_sent.fetch_add(1, Ordering::Relaxed);
    }

    /// One input report was accepted as belonging to this session.
    pub fn record_frame_received(&self) {
        self.0.frames_received.fetch_add(1, Ordering::Relaxed);
    }

    /// The reply wait looped once more: an empty read or a foreign frame.
    pub fn record_retry(&self) {
        self.0.retries.fetch_add(1, Ordering::Relaxed);
    }

    /// One frame the reply loop looked past was not the answer to this command:
    /// another session, or another sequence. Counted inside `retries` too.
    pub fn record_stolen_frame(&self) {
        self.0.stolen_frames.fetch_add(1, Ordering::Relaxed);
    }

    /// The transport was reopened after a fault.
    pub fn record_reconnect(&self) {
        self.0.reconnects.fetch_add(1, Ordering::Relaxed);
    }

    /// The device answered with a lease fault.
    pub fn record_lease_fault(&self) {
        self.0.lease_faults.fetch_add(1, Ordering::Relaxed);
    }

    /// Frames that answered another session or another sequence, without
    /// walking the histogram a full snapshot would.
    pub fn stolen_frames(&self) -> u64 {
        self.0.stolen_frames.load(Ordering::Relaxed)
    }

    /// A reply did not arrive inside the timeout.
    pub fn record_timeout(&self) {
        self.0.timeouts.fetch_add(1, Ordering::Relaxed);
    }

    /// Adds one round trip to the histogram: one relaxed add, nothing else.
    pub fn observe_latency(&self, micros: u64) {
        self.0.buckets[bucket_index(micros)].fetch_add(1, Ordering::Relaxed);
    }

    /// Zeroes every counter and every bucket; the histogram keeps its storage.
    pub fn reset(&self) {
        self.0.clear();
    }

    /// A cheap copy for a UI or a log line. Each atomic is read once, so two
    /// fields may be one report apart; nothing is locked and nothing allocates
    /// beyond the snapshot itself.
    pub fn snapshot(&self) -> MetricsSnapshot {
        let counters = &self.0;
        let mut count = 0u64;
        let mut first: Option<usize> = None;
        let mut last = 0usize;
        for (index, bucket) in counters.buckets.iter().enumerate() {
            let hits = bucket.load(Ordering::Relaxed);
            if hits == 0 {
                continue;
            }
            count += hits;
            first.get_or_insert(index);
            last = index;
        }
        let latency = match first {
            Some(first) => LatencyStats {
                count,
                min_us: lower_bound(first),
                p50_us: percentile(&counters.buckets, count, 50),
                p95_us: percentile(&counters.buckets, count, 95),
                max_us: upper_bound(last),
            },
            None => LatencyStats::default(),
        };
        MetricsSnapshot {
            frames_sent: counters.frames_sent.load(Ordering::Relaxed),
            frames_received: counters.frames_received.load(Ordering::Relaxed),
            retries: counters.retries.load(Ordering::Relaxed),
            stolen_frames: counters.stolen_frames.load(Ordering::Relaxed),
            reconnects: counters.reconnects.load(Ordering::Relaxed),
            lease_faults: counters.lease_faults.load(Ordering::Relaxed),
            timeouts: counters.timeouts.load(Ordering::Relaxed),
            latency,
        }
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Nearest-rank percentile over the occupied buckets, reported as the bucket the
/// rank fell in.
fn percentile(buckets: &[AtomicU64], count: u64, percent: u64) -> u64 {
    if count == 0 || percent == 0 {
        return 0;
    }
    let rank = count.saturating_mul(percent).div_ceil(100);
    let mut seen = 0u64;
    for (index, bucket) in buckets.iter().enumerate() {
        seen += bucket.load(Ordering::Relaxed);
        if seen >= rank {
            return representative(index);
        }
    }
    // Only reachable when a record landed after `count` was summed.
    lower_bound(buckets.len().saturating_sub(1))
}

/// The bucket a microsecond sample belongs to.
fn bucket_index(micros: u64) -> usize {
    let micros = micros.min(MAX_US - 1);
    if (micros as usize) < LINEAR_US {
        return micros as usize;
    }
    let shift = 63 - micros.leading_zeros() as usize;
    let sub = ((micros >> (shift - 3)) & (PER_OCTAVE as u64 - 1)) as usize;
    LINEAR_US + (shift - 12) * PER_OCTAVE + sub
}

/// Lowest microsecond value a bucket stands for.
fn lower_bound(index: usize) -> u64 {
    if index < LINEAR_US {
        return index as u64;
    }
    let offset = index - LINEAR_US;
    let shift = 12 + offset / PER_OCTAVE;
    let sub = (offset % PER_OCTAVE) as u64;
    (1u64 << shift) + (sub << (shift - 3))
}

/// Highest microsecond value a bucket stands for.
fn upper_bound(index: usize) -> u64 {
    if index < LINEAR_US {
        return index as u64;
    }
    let shift = 12 + (index - LINEAR_US) / PER_OCTAVE;
    lower_bound(index) + (1u64 << (shift - 3)) - 1
}

/// The single number a bucket reports: exact below the linear region, the
/// midpoint of the bucket above it.
fn representative(index: usize) -> u64 {
    if index < LINEAR_US {
        return index as u64;
    }
    (lower_bound(index) + upper_bound(index)) / 2
}
