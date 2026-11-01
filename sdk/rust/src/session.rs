//! One vendor-HID session: Feature 14 handshake, OPEN, sequenced commands and
//! reply matching. Nothing above this layer touches sequence numbers.
//!
//! Counters live here because this is the only place that knows a frame left, a
//! frame was accepted, a wait looped again, or a reply ran out of time. All of
//! that is one relaxed atomic add — the session never locks to report.

use crate::error::{Error, ErrorCode, Result};
use crate::log::Level;
use crate::metrics::Metrics;
use crate::transport::Transport;
use input_protocol::v3::{self, Capabilities, Command, Packet};
use std::time::{Duration, Instant};

pub(crate) struct Session {
    transport: Box<dyn Transport>,
    id: u32,
    sequence: u32,
    capabilities: Capabilities,
    timeout: Duration,
    /// Shared with the device, so the counters survive a reconnect.
    metrics: Metrics,
    /// When this session last sent something the device counts as a lease
    /// renewal. STATUS, BARRIER, STATUS_EXT and SESSION_LIST do not renew
    /// (protocol-v3.md, 完成與租約), so a host polling any of them must still
    /// heartbeat.
    last_renew: Instant,
}

impl Session {
    /// Reads Feature 14, then OPENs a fresh session. The caller owns the id.
    pub(crate) fn open(
        mut transport: Box<dyn Transport>,
        timeout_ms: u32,
        id: u32,
        metrics: Metrics,
    ) -> Result<Self> {
        let report = transport
            .feature()?
            .ok_or_else(|| Error::new(ErrorCode::Disconnected, "no Feature 14 reply"))?;
        let capabilities = Capabilities::parse(&report).map_err(framing_error)?;
        let mut session = Self {
            transport,
            id: id.max(1),
            sequence: 0,
            capabilities,
            timeout: Duration::from_millis(u64::from(timeout_ms.max(100))),
            metrics,
            last_renew: Instant::now(),
        };
        session.command(&Command::Open)?;
        Ok(session)
    }

    pub(crate) fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    /// How long ago this session last sent a lease-renewing command.
    pub(crate) fn renew_age(&self) -> Duration {
        self.last_renew.elapsed()
    }

    /// Sends one command. Control commands return their reply packet; input
    /// commands return `None` without waiting.
    ///
    /// Accounting is deliberately thin: an input command costs one relaxed add,
    /// a command that waits for a reply costs one more for the round trip.
    pub(crate) fn command(&mut self, command: &Command) -> Result<Option<Packet>> {
        let outcome = self.exchange(command);
        if let Err(error) = &outcome {
            match error.code() {
                ErrorCode::Lease => self.metrics.record_lease_fault(),
                ErrorCode::Timeout => self.metrics.record_timeout(),
                _ => {}
            }
        }
        outcome
    }

    fn exchange(&mut self, command: &Command) -> Result<Option<Packet>> {
        let (opcode, payload) = command.wire().map_err(string_error)?;
        let sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| Error::new(ErrorCode::Invalid, "sequence exhausted; reopen"))?;
        self.sequence = sequence;
        let packet = Packet {
            opcode,
            flags: 0,
            session: self.id,
            sequence,
            payload,
        };
        let report = packet.encode(v3::OUT_ID).map_err(string_error)?;
        if renews_lease(opcode) {
            self.last_renew = Instant::now();
        }
        crate::sdk_log!(
            Level::Trace,
            "out opcode={opcode} session={} sequence={sequence} payload={:?}",
            self.id,
            packet.payload
        );
        // Only a round trip is timed; a fire-and-forget input command must not
        // pay for a clock read it cannot use.
        let started = command.expects_reply().then(Instant::now);
        self.transport.write(&report)?;
        self.metrics.record_frame_sent();
        let Some(started) = started else {
            return Ok(None);
        };
        let deadline = started + self.timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::timeout(format!(
                    "no reply to opcode {opcode} within {} ms",
                    self.timeout.as_millis()
                )));
            }
            let Some(report) = self.transport.read(remaining.as_millis() as i32)? else {
                self.metrics.record_retry();
                continue;
            };
            let packet = Packet::decode(&report, v3::IN_ID).map_err(framing_error)?;
            if packet.session != self.id {
                self.metrics.record_retry();
                crate::sdk_log!(
                    Level::Trace,
                    "skip opcode={} session={} sequence={} (not ours)",
                    packet.opcode,
                    packet.session,
                    packet.sequence
                );
                continue;
            }
            // Ours, whatever it says: this is the frame the caller was waiting
            // for, and the counters are how a host sees the device answering.
            self.metrics.record_frame_received();
            crate::sdk_log!(
                Level::Trace,
                "in opcode={} session={} sequence={} payload={:?}",
                packet.opcode,
                packet.session,
                packet.sequence,
                packet.payload
            );
            if packet.opcode == v3::EVENT {
                let code = packet.payload.first().copied().unwrap_or(0);
                return Err(Error::from_result(code));
            }
            if packet.opcode != (opcode | 0x80) || packet.sequence != sequence {
                self.metrics.record_retry();
                continue;
            }
            if packet.payload.is_empty() {
                return Err(Error::new(ErrorCode::Framing, "empty control reply"));
            }
            if opcode != v3::SESSION_LIST && packet.payload[0] != 0 {
                return Err(Error::from_result(packet.payload[0]));
            }
            self.metrics
                .observe_latency(u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX));
            return Ok(Some(packet));
        }
    }

    pub(crate) fn close(&mut self) -> Result<()> {
        self.command(&Command::Close).map(|_| ())
    }

    pub(crate) fn heartbeat(&mut self) -> Result<()> {
        self.command(&Command::Heartbeat).map(|_| ())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Best effort: the lease is the backstop when this fails.
        let written = self.transport.write(
            &Packet {
                opcode: v3::CLOSE,
                flags: 0,
                session: self.id,
                sequence: self.sequence.saturating_add(1),
                payload: Vec::new(),
            }
            .encode(v3::OUT_ID)
            .unwrap_or([0u8; 64]),
        );
        if written.is_ok() {
            self.metrics.record_frame_sent();
        }
    }
}

/// A well-formed call failed: the caller passed something the SDK cannot encode.
pub(crate) fn string_error(error: impl std::fmt::Display) -> Error {
    Error::new(ErrorCode::Invalid, error.to_string())
}

/// A report did not parse. That is a wire fault, not a calling mistake, so it
/// reports Framing like the protocol table says it should.
pub(crate) fn framing_error(error: impl std::fmt::Display) -> Error {
    Error::new(ErrorCode::Framing, error.to_string())
}

/// Non-zero, per-process-unique session ids.
pub(crate) fn new_session_id() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(1);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    ((now as u32)
        ^ ((now >> 32) as u32)
        ^ std::process::id()
        ^ NEXT.fetch_add(0x9e37_79b9, Ordering::Relaxed))
    .max(1)
}

/// The device writes `lease_at` only for OPEN, HEARTBEAT, RELEASE_ALL,
/// BOOTLOADER and valid input (`firmware/engine/src/engine.rs`). The four
/// query/barrier opcodes leave it alone, so polling STATUS, STATUS_EXT or
/// SESSION_LIST keeps the counters fresh without keeping the session alive.
fn renews_lease(opcode: u8) -> bool {
    matches!(
        opcode,
        v3::OPEN
            | v3::HEARTBEAT
            | v3::RELEASE_ALL
            | v3::BOOTLOADER
            | v3::KEY
            | v3::SNAPSHOT
            | v3::MOUSE_MOVE
            | v3::MOUSE_BUTTONS
            | v3::WHEEL
    )
}

#[cfg(test)]
mod tests {
    use super::renews_lease;
    use input_protocol::v3;

    /// The shipped firmware writes `lease_at` only on OPEN, HEARTBEAT,
    /// RELEASE_ALL, BOOTLOADER and valid input (`firmware/engine/src/engine.rs`).
    /// Everything else — including the two diagnostics queries — is inert.
    #[test]
    fn lease_renewal_set_matches_the_firmware() {
        for opcode in [
            v3::OPEN,
            v3::HEARTBEAT,
            v3::RELEASE_ALL,
            v3::BOOTLOADER,
            v3::KEY,
            v3::SNAPSHOT,
            v3::MOUSE_MOVE,
            v3::MOUSE_BUTTONS,
            v3::WHEEL,
        ] {
            assert!(renews_lease(opcode), "opcode {opcode} must renew the lease");
        }
        for opcode in [
            v3::CLOSE,
            v3::STATUS,
            v3::BARRIER,
            v3::STATUS_EXT,
            v3::SESSION_LIST,
            v3::EVENT,
        ] {
            assert!(
                !renews_lease(opcode),
                "opcode {opcode} must not renew the lease"
            );
        }
    }
}
