//! One vendor-HID session: Feature 14 handshake, OPEN, sequenced commands and
//! reply matching. Nothing above this layer touches sequence numbers.

use crate::error::{Error, ErrorCode, Result};
use crate::log::Level;
use crate::transport::Transport;
use input_protocol::v3::{self, Capabilities, Command, Packet};
use std::time::{Duration, Instant};

pub(crate) struct Session {
    transport: Box<dyn Transport>,
    id: u32,
    sequence: u32,
    capabilities: Capabilities,
    timeout: Duration,
    /// When this session last sent something the device counts as a lease
    /// renewal. STATUS and BARRIER do not renew (protocol-v3.md:120), so a host
    /// that polls STATUS in a loop must still heartbeat.
    last_renew: Instant,
}

impl Session {
    /// Reads Feature 14, then OPENs a fresh session. The caller owns the id.
    pub(crate) fn open(
        mut transport: Box<dyn Transport>,
        timeout_ms: u32,
        id: u32,
    ) -> Result<Self> {
        let report = transport
            .feature()?
            .ok_or_else(|| Error::new(ErrorCode::Disconnected, "no Feature 14 reply"))?;
        let capabilities = Capabilities::parse(&report).map_err(string_error)?;
        let mut session = Self {
            transport,
            id: id.max(1),
            sequence: 0,
            capabilities,
            timeout: Duration::from_millis(u64::from(timeout_ms.max(100))),
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
    pub(crate) fn command(&mut self, command: &Command) -> Result<Option<Packet>> {
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
        self.transport.write(&report)?;
        if !command.expects_reply() {
            return Ok(None);
        }
        let deadline = Instant::now() + self.timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::timeout(format!(
                    "no reply to opcode {opcode} within {} ms",
                    self.timeout.as_millis()
                )));
            }
            let Some(report) = self.transport.read(remaining.as_millis() as i32)? else {
                continue;
            };
            let packet = Packet::decode(&report, v3::IN_ID).map_err(string_error)?;
            if packet.session != self.id {
                crate::sdk_log!(
                    Level::Trace,
                    "skip opcode={} session={} sequence={} (not ours)",
                    packet.opcode,
                    packet.session,
                    packet.sequence
                );
                continue;
            }
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
                continue;
            }
            if packet.payload.is_empty() {
                return Err(Error::new(ErrorCode::Framing, "empty control reply"));
            }
            if opcode != v3::SESSION_LIST && packet.payload[0] != 0 {
                return Err(Error::from_result(packet.payload[0]));
            }
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
        let _ = self.transport.write(
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
    }
}

pub(crate) fn string_error(error: impl std::fmt::Display) -> Error {
    Error::new(ErrorCode::Invalid, error.to_string())
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

/// The device renews our lease on every command except STATUS and BARRIER
/// (`firmware/protocol-v3.md`, 完成與租約). Polling STATUS therefore keeps the
/// counters fresh without keeping the session alive.
fn renews_lease(opcode: u8) -> bool {
    opcode != v3::STATUS && opcode != v3::BARRIER
}
