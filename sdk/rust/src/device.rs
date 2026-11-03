//! One controller: connection state, heartbeat, typed operations and recovery.

use crate::diagnostics::{DeviceError, Diagnostics};
use crate::error::{Error, ErrorCode, Result};
use crate::event::{ConnectionState, EventHandler, notify};
use crate::log::Level;
use crate::metrics::{Metrics, MetricsSnapshot};
use crate::session::{Session, Sharing, new_session_id};
use crate::transport::Transport;
use gdb_motion::{Rng, SplitMix64};
use input_protocol::v3::{self, Command, SessionInfo, Status, StatusExt};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, Weak};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// A lease renewal is due between these two ages. The lease itself is 2000 ms,
/// so anywhere in this window is safe; the point is that the beat does not land
/// on one exact interval, which is visible to anything watching the bus.
pub(crate) const HEARTBEAT_MIN_MS: u64 = 600;
pub(crate) const HEARTBEAT_MAX_MS: u64 = 900;

fn beat_due(rng: &mut SplitMix64) -> Duration {
    let span = HEARTBEAT_MAX_MS - HEARTBEAT_MIN_MS;
    Duration::from_millis(HEARTBEAT_MIN_MS + (rng.unit() * span as f64) as u64)
}

#[derive(Clone, Debug)]
pub enum Target {
    Path(String),
    Serial(String),
    Mock,
}

#[derive(Clone)]
pub struct DeviceOptions {
    pub auto_reconnect: bool,
    pub timeout_ms: u32,
    pub heartbeat: bool,
}

impl Default for DeviceOptions {
    fn default() -> Self {
        Self {
            auto_reconnect: true,
            timeout_ms: 1000,
            heartbeat: true,
        }
    }
}

/// A frame that arrived on the shared IN queue but belongs to another session,
/// handed over as the raw 64-byte input report.
pub type StrayHook = Arc<dyn Fn(&[u8]) + Send + Sync>;

/// Whether this process is the only one driving the board.
///
/// The device has one IN queue and no way to address a reply to a handle, so two
/// controllers occasionally take each other's frames. That is a fact about the
/// hardware; what changes here is what the SDK does about it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Ownership {
    /// One owner. A reply that never arrives is `Timeout`, and a fault the SDK
    /// believes it can fix reopens the session on its own.
    #[default]
    Exclusive,
    /// Several programs, all of which may send commands. A reply that never
    /// arrives is one `Busy` naming the command — another process may have taken
    /// it — and the session is left exactly as it is: the SDK never reopens on
    /// its own, because a reopen is one more CLOSE and OPEN on a board it does
    /// not own. `reconnect()` is still there for a host that knows the board is
    /// its own again.
    ///
    /// Nothing else changes: sequence numbers, the lease, the heartbeat and the
    /// counters stay as they are, and a command still costs one exchange. What
    /// sharing cannot give you is a guarantee about which process gets a given
    /// reply; avoiding that collision is the hosts' discipline, not the SDK's.
    Shared,
}

impl Ownership {
    pub fn is_shared(self) -> bool {
        matches!(self, Self::Shared)
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub serial: String,
    pub product: String,
    pub board: u8,
    pub firmware: String,
    pub lease_ms: u16,
    pub poll_ms: u8,
    pub capabilities: u32,
    pub device_id: [u8; 8],
    pub build: u32,
}

struct State {
    session: Option<Session>,
}

pub(crate) struct Inner {
    state: Mutex<State>,
    target: Target,
    options: DeviceOptions,
    auto_reconnect: AtomicBool,
    info: DeviceInfo,
    events: RwLock<Option<Arc<dyn EventHandler>>>,
    closed: AtomicBool,
    buttons: AtomicU8,
    virtual_cursor: Mutex<(i32, i32)>,
    heartbeat: Mutex<Option<JoinHandle<()>>>,
    /// Shared with every session this device opens, so the counters outlive a
    /// reconnect; every read and write goes through a relaxed atomic.
    metrics: Metrics,
    /// Shared with every session this device opens: the ownership the host
    /// picked and the hook for frames that belong to somebody else.
    sharing: Arc<Sharing>,
    /// Shared with every session this device opens: the latest failure, so a
    /// caller on another thread can read what happened.
    diagnostics: Arc<Diagnostics>,
    /// The id of the session in flight, readable without the session lock so a
    /// failure above the session can still name it.
    session_id: AtomicU32,
}

#[derive(Clone)]
pub struct Device {
    pub(crate) inner: Arc<Inner>,
}

/// A [`Device`] that does not keep it alive.
///
/// An event handler is stored on the device, so a handler that held a `Device`
/// would keep its own host alive forever. This is the handle for that case:
/// upgrade it inside the callback, and let the device go when the host drops it.
#[derive(Clone)]
pub struct WeakDevice(Weak<Inner>);

impl WeakDevice {
    /// The device, or `None` when it has already been dropped.
    pub fn upgrade(&self) -> Option<Device> {
        self.0.upgrade().map(|inner| Device { inner })
    }
}

impl Device {
    pub(crate) fn from_transport(
        transport: Box<dyn Transport>,
        target: Target,
        serial: String,
        product: String,
        options: DeviceOptions,
    ) -> Result<Self> {
        Self::connect(
            transport,
            target,
            serial,
            product,
            options,
            Ownership::Exclusive,
        )
    }

    /// A device over a transport the caller supplies: a proxy, a capture, a test
    /// fake. The transport answers Feature 14 and speaks the v3 wire exactly like
    /// the HID backend, and `target` says what a reconnect would re-open.
    ///
    /// `ownership` belongs here rather than in `set_ownership` because it decides
    /// whether the SDK may reopen the session at all; changing it later only
    /// affects commands that have not been sent yet.
    pub fn over_transport(
        transport: Box<dyn Transport>,
        target: Target,
        options: DeviceOptions,
        ownership: Ownership,
    ) -> Result<Self> {
        let (serial, product) = match &target {
            Target::Mock => ("MOCK".to_string(), "GDBOX mock".to_string()),
            Target::Path(path) => (path.clone(), "GDBOX".to_string()),
            Target::Serial(serial) => (serial.clone(), "GDBOX".to_string()),
        };
        Self::connect(transport, target, serial, product, options, ownership)
    }

    /// Opens a board that other programs may be driving too.
    ///
    /// The one difference from [`Manager::open`](crate::Manager::open) is what a
    /// lost reply costs: one `Busy` error naming the command, instead of a
    /// `Timeout` that invites the host to start a session the board did not ask
    /// for. See [`Ownership::Shared`].
    pub fn open_shared(selector: Option<&str>, options: DeviceOptions) -> Result<Self> {
        let device = crate::manager::Manager::open(selector, options)?;
        device.set_ownership(Ownership::Shared);
        Ok(device)
    }

    /// The in-memory device in shared mode, with its handle, for a host that
    /// wants to exercise the stolen-reply path without a board to share.
    pub fn open_mock_shared(options: DeviceOptions) -> Result<(Self, crate::mock::MockHandle)> {
        let (device, handle) = crate::manager::Manager::open_mock_with_handle(options)?;
        device.set_ownership(Ownership::Shared);
        Ok((device, handle))
    }

    /// Switches what a lost reply costs, for commands not yet sent. A shared
    /// device is one that never reopens on its own, so this is worth setting
    /// once, before the first command, which is what the constructors above do.
    pub fn set_ownership(&self, ownership: Ownership) {
        self.inner.sharing.set_shared(ownership.is_shared());
    }

    pub fn ownership(&self) -> Ownership {
        if self.inner.sharing.is_shared() {
            Ownership::Shared
        } else {
            Ownership::Exclusive
        }
    }

    fn connect(
        transport: Box<dyn Transport>,
        target: Target,
        serial: String,
        product: String,
        options: DeviceOptions,
        ownership: Ownership,
    ) -> Result<Self> {
        let metrics = Metrics::new();
        let sharing = Arc::new(Sharing::new(ownership.is_shared()));
        let diagnostics = Arc::new(Diagnostics::new());
        let session_id = new_session_id();
        let session = Session::open(
            transport,
            options.timeout_ms,
            session_id,
            metrics.clone(),
            diagnostics.clone(),
            sharing.clone(),
        )?;
        let capabilities = session.capabilities().clone();
        let info = DeviceInfo {
            serial,
            product,
            board: capabilities.board,
            firmware: capabilities.firmware.clone(),
            lease_ms: capabilities.lease_ms,
            poll_ms: capabilities.poll_ms,
            capabilities: capabilities.flags,
            device_id: capabilities.device_id,
            build: capabilities.build,
        };
        let inner = Arc::new(Inner {
            state: Mutex::new(State {
                session: Some(session),
            }),
            target,
            auto_reconnect: AtomicBool::new(options.auto_reconnect),
            options,
            info,
            events: RwLock::new(None),
            closed: AtomicBool::new(false),
            buttons: AtomicU8::new(0),
            virtual_cursor: Mutex::new((960, 540)),
            heartbeat: Mutex::new(None),
            metrics,
            sharing,
            diagnostics,
            session_id: AtomicU32::new(session_id),
        });
        if inner.options.heartbeat {
            start_heartbeat(&inner);
        }
        crate::sdk_log!(
            Level::Info,
            "connected serial={} firmware={} board={}",
            inner.info.serial,
            inner.info.firmware,
            inner.info.board
        );
        Ok(Self { inner })
    }

    pub fn info(&self) -> DeviceInfo {
        self.inner.info.clone()
    }

    /// Counters as of now: frames, retries, reconnects, lease faults, timeouts
    /// and the round-trip histogram.
    ///
    /// Cheap enough to poll from a UI: it reads atomics and walks the histogram,
    /// it never takes the session lock, so it cannot stall a command in flight.
    pub fn metrics(&self) -> MetricsSnapshot {
        self.inner.metrics.snapshot()
    }

    /// Zeroes the counters, for instance when a host starts a new measurement.
    pub fn reset_metrics(&self) {
        self.inner.metrics.reset();
    }

    /// The latest failure this device recorded, from any thread.
    ///
    /// Unlike the C ABI's `gdb_last_error`, which answers for the calling
    /// thread, this is the device's own record: a command that failed on the
    /// heartbeat, a scheduler worker or a movement thread is still readable
    /// here. A later failure replaces it and a success never clears it, so a
    /// host that polls a status bar sees what went wrong instead of an empty
    /// slot the next good call wiped.
    pub fn last_error(&self) -> Option<DeviceError> {
        self.inner.diagnostics.last()
    }

    /// The device without a strong reference to it, for an event handler that
    /// wants to read the device from inside a callback.
    pub fn downgrade(&self) -> WeakDevice {
        WeakDevice(Arc::downgrade(&self.inner))
    }

    /// The last STATUS the device answered, in the shape a failure reports.
    ///
    /// `None` until a STATUS has been read. Cheap enough for an event callback:
    /// one uncontended lock on a copy of three numbers.
    pub fn status_summary(&self) -> Option<crate::diagnostics::StatusSummary> {
        self.inner.diagnostics.status_summary()
    }

    /// Records `error` as this device's latest failure and hands it back, for
    /// the layers that can fail before a command reaches the session — an
    /// anchor that does not resolve, a click count of zero, a usage the
    /// firmware does not have.
    pub(crate) fn track<T>(&self, result: Result<T>) -> Result<T> {
        if let Err(error) = &result {
            self.inner
                .diagnostics
                .record(error, None, self.session_id());
        }
        result
    }

    fn session_id(&self) -> u32 {
        self.inner.session_id.load(Ordering::Relaxed)
    }

    /// Frames the reply loop had to look past because they answered another
    /// session or another sequence. On a shared board this is the counter that
    /// tells a collision apart from a slow board.
    pub fn stolen_frames(&self) -> u64 {
        self.inner.metrics.stolen_frames()
    }

    /// Records that an absolute move asked for a point outside its anchor
    /// rectangle and was clamped to `applied`.
    pub(crate) fn record_clamp(&self, requested: (i32, i32), applied: (i32, i32)) {
        self.inner.metrics.record_clamp(requested, applied);
    }

    /// Called with the raw 64-byte report of every frame that arrives while a
    /// command waits and belongs to another session. The default is to drop it,
    /// which is what a host alone on the board wants; a cooperating host can use
    /// it to hand the frame back instead of letting it evaporate.
    ///
    /// The hook runs on the thread that is reading the queue, with that device's
    /// session busy, so it must not call back into the same `Device`. It is
    /// called for foreign sessions only: a late reply to this session's own
    /// earlier command moves the counters and nothing else.
    pub fn set_stray_hook(&self, hook: StrayHook) {
        self.inner.sharing.set_stray_hook(Some(hook));
    }

    /// Drops stray frames again.
    pub fn clear_stray_hook(&self) {
        self.inner.sharing.set_stray_hook(None);
    }

    pub fn serial(&self) -> &str {
        &self.inner.info.serial
    }

    pub fn state(&self) -> ConnectionState {
        if self.inner.closed.load(Ordering::Relaxed) {
            return ConnectionState::Disconnected;
        }
        let state = self.lock();
        if state.session.is_some() {
            ConnectionState::Connected
        } else {
            ConnectionState::Disconnected
        }
    }

    pub fn set_event_handler(&self, handler: Option<Arc<dyn EventHandler>>) {
        if let Ok(mut slot) = self.inner.events.write() {
            *slot = handler;
        }
    }

    pub fn set_auto_reconnect(&self, enabled: bool) {
        // The option is read through this flag so a caller can change it live.
        self.inner.auto_reconnect.store(enabled, Ordering::Relaxed);
    }

    pub fn scheduler(&self) -> crate::task::Scheduler {
        crate::task::Scheduler::new(self.clone())
    }

    pub fn status(&self) -> Result<Status> {
        self.with_session(|session| {
            let reply = session
                .command(&Command::Status)?
                .ok_or_else(|| Error::new(ErrorCode::Framing, "STATUS had no reply"))?;
            let status = Status::parse(&reply.payload)
                .map_err(|error| Error::new(ErrorCode::Framing, error.to_string()))?;
            // A failure later on reports what the device last said about itself,
            // so the summary is kept beside the failure rather than re-asked.
            self.inner.diagnostics.record_status(&status);
            Ok(status)
        })
    }

    pub fn status_ext(&self) -> Result<StatusExt> {
        self.with_session(|session| {
            let reply = session
                .command(&Command::StatusExt)?
                .ok_or_else(|| Error::new(ErrorCode::Framing, "STATUS_EXT had no reply"))?;
            StatusExt::parse(&reply.payload)
                .map_err(|error| Error::new(ErrorCode::Framing, error.to_string()))
        })
    }

    pub fn sessions(&self) -> Result<Vec<SessionInfo>> {
        self.with_session(|session| {
            let reply = session
                .command(&Command::SessionList)?
                .ok_or_else(|| Error::new(ErrorCode::Framing, "SESSION_LIST had no reply"))?;
            Ok(v3::parse_session_list(&reply.payload))
        })
    }

    pub fn barrier(&self) -> Result<()> {
        self.with_session(|session| session.command(&Command::Barrier).map(|_| ()))
    }

    /// Sends one command as it is and hands back the raw reply.
    ///
    /// An escape hatch for callers that already speak the wire protocol — the
    /// desktop tool sends chords, keystrokes and raw moves through it. It still
    /// goes through the same session, sequence numbers and reconnect path, but
    /// the SDK's own bookkeeping — held buttons, the mock cursor — is not
    /// updated.
    pub fn command(&self, command: &Command) -> Result<Option<v3::Packet>> {
        self.with_session(|session| session.command(command))
    }

    pub fn release_all(&self) -> Result<()> {
        self.inner.buttons.store(0, Ordering::Relaxed);
        self.with_session(|session| session.command(&Command::ReleaseAll).map(|_| ()))
    }

    /// True when the SDK talks to the in-memory device instead of hardware.
    pub fn is_mock(&self) -> bool {
        matches!(self.inner.target, Target::Mock)
    }

    /// The mock's simulated pointer; absolute moves drive this instead of the
    /// real cursor so tests never depend on where the user left the mouse.
    pub(crate) fn cursor(&self) -> (i32, i32) {
        *self
            .inner
            .virtual_cursor
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn send_mouse_move(&self, dx: i16, dy: i16) -> Result<()> {
        self.send_input(&Command::MouseMove(dx, dy))?;
        if self.is_mock() {
            let mut cursor = self
                .inner
                .virtual_cursor
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            cursor.0 = (cursor.0 + i32::from(dx)).clamp(0, 1919);
            cursor.1 = (cursor.1 + i32::from(dy)).clamp(0, 1079);
        }
        Ok(())
    }

    pub(crate) fn buttons(&self) -> u8 {
        self.inner.buttons.load(Ordering::Relaxed)
    }

    pub(crate) fn set_buttons(&self, mask: u8) -> Result<()> {
        if mask & !0x1F != 0 {
            return Err(Error::invalid("buttons are bits 0-4"));
        }
        self.inner.buttons.store(mask, Ordering::Relaxed);
        Ok(())
    }

    pub fn bootloader(&self) -> Result<()> {
        self.with_session(|session| session.command(&Command::Bootloader).map(|_| ()))
    }

    pub fn close(&self) -> Result<()> {
        self.inner.closed.store(true, Ordering::Relaxed);
        if let Some(handle) = self.inner.heartbeat.lock().unwrap().take() {
            let _ = handle.join();
        }
        self.inner.buttons.store(0, Ordering::Relaxed);
        crate::sdk_log!(Level::Info, "closed serial={}", self.inner.info.serial);
        let session = self.lock().session.take();
        notify(&self.handler(), |handler| {
            handler.state(ConnectionState::Disconnected)
        });
        match session {
            Some(mut session) => session.close(),
            None => Ok(()),
        }
    }

    /// Re-opens the transport and a fresh session; held keys are not restored.
    pub fn reconnect(&self) -> Result<()> {
        let mut state = self.lock();
        self.reopen(&mut state)
    }

    pub(crate) fn with_session<T>(
        &self,
        mut operation: impl FnMut(&mut Session) -> Result<T>,
    ) -> Result<T> {
        if self.inner.closed.load(Ordering::Relaxed) {
            let error = Error::disconnected("device is closed");
            self.inner.diagnostics.record(&error, None, 0);
            return Err(error);
        }
        let mut state = self.lock();
        let session_id = self.session_id();
        // The session records a protocol failure with its opcode; this only
        // records the failures above it (an anchor that does not resolve, an
        // argument refused before anything was sent), and only when the session
        // did not just record the same one.
        let mark = self.inner.diagnostics.mark();
        if let Some(session) = state.session.as_mut() {
            match operation(session) {
                Ok(value) => return Ok(value),
                Err(error) => {
                    crate::sdk_log!(
                        Level::Warn,
                        "command failed ({}): {}",
                        error.code().as_u8(),
                        error.message
                    );
                    // A shared board is not this process's to reopen: another
                    // owner may be mid-command, and CLOSE plus OPEN is exactly
                    // the traffic a collision must not cost. The error goes back
                    // once and the session stays; `reconnect()` is the host's
                    // call to make when it knows it is alone again.
                    if self.inner.sharing.is_shared()
                        || !recoverable(error.code())
                        || !self.inner.auto_reconnect.load(Ordering::Relaxed)
                    {
                        self.inner
                            .diagnostics
                            .record_unless_marked(mark, &error, session_id);
                        notify(&self.handler(), |handler| handler.error(&error));
                        return Err(error);
                    }
                }
            }
        }
        if let Err(error) = self.reopen(&mut state) {
            self.inner.closed.store(true, Ordering::Relaxed);
            self.inner.diagnostics.record(&error, None, session_id);
            notify(&self.handler(), |handler| handler.error(&error));
            return Err(error);
        }
        let result = state
            .session
            .as_mut()
            .ok_or_else(|| Error::disconnected("no session after reconnect"))?;
        let value = operation(result);
        if let Err(error) = &value {
            self.inner
                .diagnostics
                .record_unless_marked(mark, error, session_id);
            notify(&self.handler(), |handler| handler.error(error));
        }
        value
    }

    pub(crate) fn send_input(&self, command: &Command) -> Result<()> {
        self.with_session(|session| session.command(command).map(|_| ()))
    }

    fn reopen(&self, state: &mut MutexGuard<'_, State>) -> Result<()> {
        state.session = None;
        let transport = crate::manager::open_transport(&self.inner.target)?;
        let session_id = new_session_id();
        let session = Session::open(
            transport,
            self.inner.options.timeout_ms,
            session_id,
            self.inner.metrics.clone(),
            self.inner.diagnostics.clone(),
            self.inner.sharing.clone(),
        )?;
        state.session = Some(session);
        self.inner.session_id.store(session_id, Ordering::Relaxed);
        self.inner.metrics.record_reconnect();
        crate::sdk_log!(Level::Info, "reconnected serial={}", self.inner.info.serial);
        notify(&self.handler(), |handler| {
            handler.state(ConnectionState::Connected)
        });
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn handler(&self) -> Option<Arc<dyn EventHandler>> {
        self.inner.events.read().ok().and_then(|slot| slot.clone())
    }
}

fn recoverable(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::Disconnected | ErrorCode::Io | ErrorCode::Lease
    )
}

fn start_heartbeat(inner: &Arc<Inner>) {
    let weak: Weak<Inner> = Arc::downgrade(inner);
    let handle = thread::spawn(move || {
        let mut rng = SplitMix64::from_entropy();
        let mut due_after = beat_due(&mut rng);
        loop {
            thread::sleep(Duration::from_millis(100));
            let Some(inner) = weak.upgrade() else {
                return;
            };
            if inner.closed.load(Ordering::Relaxed) {
                return;
            }
            let mut state = match inner.state.try_lock() {
                Ok(state) => state,
                Err(_) => continue,
            };
            // Ask the session, not the caller: the query and barrier opcodes do
            // not renew the device-side lease, so a host polling STATUS_EXT would
            // otherwise never heartbeat and lose the session every lease period.
            let due = match state.session.as_ref() {
                Some(session) => session.renew_age() >= due_after,
                None => false,
            };
            if !due {
                continue;
            }
            let outcome = {
                let session = state.session.as_mut().expect("checked above");
                session.heartbeat()
            };
            match outcome {
                Ok(()) => due_after = beat_due(&mut rng),
                Err(error) => {
                    drop(state);
                    let device = Device {
                        inner: inner.clone(),
                    };
                    notify(&device.handler(), |handler| handler.error(&error));
                    // Reconnect from this thread; `with_session` would deadlock on
                    // the lock we just dropped. A shared device never reconnects
                    // here: a heartbeat whose reply another owner took is `Busy`,
                    // which is not recoverable, and the next beat is 500 ms away.
                    if !device.inner.sharing.is_shared()
                        && device.inner.auto_reconnect.load(Ordering::Relaxed)
                        && recoverable(error.code())
                    {
                        let _ = device.reconnect();
                    }
                }
            }
        }
    });
    *inner.heartbeat.lock().unwrap() = Some(handle);
}
