//! The last failure a device recorded, kept on the device instead of on the
//! calling thread.
//!
//! `gdb_last_error` answers "what did *my* last call do", which is what a
//! single-threaded host needs. A host whose commands also run on a heartbeat, a
//! scheduler worker or a movement thread needs the other question answered: what
//! did this *device* fail at, whoever called it. That answer lives here, is
//! replaced by the next failure and is never cleared by a success — a background
//! thread that failed a moment ago is exactly the case it exists for.

use crate::error::{Error, ErrorCode};
use serde::Serialize;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// The part of a STATUS reply worth keeping in a failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusSummary {
    /// Frames the device has taken in since boot.
    pub received: u32,
    /// Frames it has finished.
    pub completed: u32,
    /// Frames still queued.
    pub pending: u8,
}

impl StatusSummary {
    pub fn of(status: &input_protocol::v3::Status) -> Self {
        Self {
            received: status.received,
            completed: status.completed,
            pending: status.pending,
        }
    }
}

/// One recorded failure: the error, plus the structure a message alone cannot
/// carry — which opcode failed, under which session, and how the device said it
/// was doing the last time it was asked.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceError {
    /// The numeric `ErrorCode`, the same value the call returned.
    pub code: u8,
    pub message: String,
    /// The command that failed, by its wire name, when the failure came from a
    /// command. `None` for a failure above the session — a screen that does not
    /// resolve, an argument the SDK refused before sending anything.
    pub opcode: Option<&'static str>,
    /// The session the failure belongs to, 0 when none was open.
    pub session: u32,
    /// The last STATUS the device answered, when one has been read.
    pub status: Option<StatusSummary>,
    /// How long ago the failure happened.
    pub age_ms: u64,
}

impl DeviceError {
    /// The message a log line or `gdb_device_last_error` carries: the error as
    /// the SDK raised it, with the structure appended, so the prefix stays the
    /// one callers already match on.
    pub fn describe(&self) -> String {
        let mut tail: Vec<String> = Vec::new();
        if let Some(opcode) = self.opcode {
            tail.push(format!("opcode={opcode}"));
        }
        tail.push(format!("session={}", self.session));
        if let Some(status) = &self.status {
            tail.push(format!(
                "status=received:{},completed:{},pending:{}",
                status.received, status.completed, status.pending
            ));
        }
        format!("{} ({})", self.message, tail.join(" "))
    }

    pub fn code(&self) -> ErrorCode {
        ErrorCode::from_u8(self.code)
    }
}

#[derive(Clone)]
struct Failure {
    error: Error,
    opcode: Option<&'static str>,
    session: u32,
    at: Instant,
}

/// Shared by a device and every session it opens, so the record survives a
/// reconnect the same way the counters do.
#[derive(Default)]
pub(crate) struct Diagnostics {
    failure: Mutex<Option<Failure>>,
    status: Mutex<Option<StatusSummary>>,
    /// Bumped by every record, so a caller that is about to record an error of
    /// its own can tell whether the session underneath already recorded a more
    /// specific one.
    mark: AtomicU64,
}

impl Diagnostics {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Stores `error` as the device's latest failure.
    ///
    /// A failure the session already recorded for this exact code and message
    /// keeps its opcode: the layer above it only sees the same error without
    /// that structure, and losing it would make the record poorer, not newer.
    pub(crate) fn record(&self, error: &Error, opcode: Option<&'static str>, session: u32) {
        self.mark.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut slot) = self.failure.lock() {
            if opcode.is_none()
                && let Some(existing) = slot.as_ref()
                && existing.opcode.is_some()
                && existing.error.code() == error.code()
                && existing.error.message == error.message
            {
                return;
            }
            *slot = Some(Failure {
                error: error.clone(),
                opcode,
                session,
                at: Instant::now(),
            });
        }
    }

    /// [`Diagnostics::record`] unless something was recorded since `mark`: the
    /// session sees the failure first and knows the opcode, so its record wins.
    pub(crate) fn record_unless_marked(&self, mark: u64, error: &Error, session: u32) {
        if self.mark.load(Ordering::Relaxed) == mark {
            self.record(error, None, session);
        }
    }

    pub(crate) fn mark(&self) -> u64 {
        self.mark.load(Ordering::Relaxed)
    }

    /// The last STATUS the device answered, which a later failure reports.
    pub(crate) fn record_status(&self, status: &input_protocol::v3::Status) {
        if let Ok(mut slot) = self.status.lock() {
            *slot = Some(StatusSummary::of(status));
        }
    }

    /// The latest failure, or `None` while the device has not failed yet.
    pub(crate) fn last(&self) -> Option<DeviceError> {
        let failure = self.failure.lock().ok()?.clone()?;
        Some(DeviceError {
            code: failure.error.code().as_u8(),
            message: failure.error.message,
            opcode: failure.opcode,
            session: failure.session,
            status: self.status.lock().ok().and_then(|slot| *slot),
            age_ms: u64::try_from(failure.at.elapsed().as_millis()).unwrap_or(u64::MAX),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_keeps_the_opcode_the_session_and_the_last_status() {
        let diagnostics = Diagnostics::new();
        assert!(diagnostics.last().is_none());
        diagnostics.record(&Error::timeout("no reply"), Some("STATUS"), 0x1234);
        let last = diagnostics.last().expect("one failure");
        assert_eq!(last.code(), ErrorCode::Timeout);
        assert_eq!(last.opcode, Some("STATUS"));
        assert_eq!(last.session, 0x1234);
        assert_eq!(last.describe(), "no reply (opcode=STATUS session=4660)");
    }

    #[test]
    fn a_later_failure_replaces_the_earlier_one_and_a_success_does_not_clear_it() {
        let diagnostics = Diagnostics::new();
        // The mark is taken before the call, so a record made during it is what
        // the caller above the session must not overwrite.
        let mark = diagnostics.mark();
        diagnostics.record(&Error::invalid("bad argument"), None, 7);
        let first = diagnostics.last().expect("first");
        assert_eq!(first.opcode, None);
        assert_eq!(first.describe(), "bad argument (session=7)");

        diagnostics.record_unless_marked(mark, &Error::invalid("stale"), 7);
        assert_eq!(
            diagnostics.last().expect("still the session's").message,
            "bad argument",
            "a marked record must not overwrite a fresher one"
        );

        let mark = diagnostics.mark();
        diagnostics.record_unless_marked(mark, &Error::invalid("from above the session"), 8);
        assert_eq!(
            diagnostics.last().expect("the new one").message,
            "from above the session"
        );
    }

    /// The layer above the session sees the same error without the opcode; that
    /// must not erase the structure the session recorded.
    #[test]
    fn recording_the_same_failure_without_an_opcode_keeps_the_opcode() {
        let diagnostics = Diagnostics::new();
        diagnostics.record(&Error::timeout("no reply"), Some("STATUS"), 42);
        diagnostics.record(&Error::timeout("no reply"), None, 42);
        let last = diagnostics.last().expect("one failure");
        assert_eq!(last.opcode, Some("STATUS"));
        assert_eq!(last.session, 42);

        // A different failure is newer and replaces it.
        diagnostics.record(&Error::invalid("count"), None, 42);
        assert_eq!(diagnostics.last().expect("newer").opcode, None);
    }

    #[test]
    fn a_status_summary_rides_along_once_the_device_has_answered() {
        let diagnostics = Diagnostics::new();
        let status = input_protocol::v3::Status {
            received: 143,
            completed: 142,
            pending: 1,
            lease_ms: 2000,
            rx: 0,
            tx: 0,
            hid: 0,
            failsafe: 0,
            state: Default::default(),
        };
        diagnostics.record_status(&status);
        diagnostics.record(&Error::timeout("no reply"), Some("BARRIER"), 9);
        let last = diagnostics.last().expect("one failure");
        assert_eq!(
            last.status,
            Some(StatusSummary {
                received: 143,
                completed: 142,
                pending: 1
            })
        );
        assert_eq!(
            last.describe(),
            "no reply (opcode=BARRIER session=9 status=received:143,completed:142,pending:1)"
        );
    }
}
