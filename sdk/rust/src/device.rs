//! One controller: connection state, heartbeat, typed operations and recovery.

use crate::error::{Error, ErrorCode, Result};
use crate::event::{ConnectionState, EventHandler, notify};
use crate::log::Level;
use crate::session::{Session, new_session_id};
use crate::transport::Transport;
use input_protocol::v3::{self, Command, SessionInfo, Status, StatusExt};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, Weak};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub(crate) const HEARTBEAT_MS: u64 = 500;

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
}

#[derive(Clone)]
pub struct Device {
    pub(crate) inner: Arc<Inner>,
}

impl Device {
    pub(crate) fn from_transport(
        transport: Box<dyn Transport>,
        target: Target,
        serial: String,
        product: String,
        options: DeviceOptions,
    ) -> Result<Self> {
        let session = Session::open(transport, options.timeout_ms, new_session_id())?;
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
            return Err(Error::disconnected("device is closed"));
        }
        let mut state = self.lock();
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
                    if !recoverable(error.code())
                        || !self.inner.auto_reconnect.load(Ordering::Relaxed)
                    {
                        notify(&self.handler(), |handler| handler.error(&error));
                        return Err(error);
                    }
                }
            }
        }
        if let Err(error) = self.reopen(&mut state) {
            self.inner.closed.store(true, Ordering::Relaxed);
            notify(&self.handler(), |handler| handler.error(&error));
            return Err(error);
        }
        let result = state
            .session
            .as_mut()
            .ok_or_else(|| Error::disconnected("no session after reconnect"))?;
        let value = operation(result);
        if let Err(error) = &value {
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
        let session = Session::open(transport, self.inner.options.timeout_ms, new_session_id())?;
        state.session = Some(session);
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
                Some(session) => session.renew_age() >= Duration::from_millis(HEARTBEAT_MS),
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
                Ok(()) => {}
                Err(error) => {
                    drop(state);
                    let device = Device {
                        inner: inner.clone(),
                    };
                    notify(&device.handler(), |handler| handler.error(&error));
                    // Reconnect from this thread; `with_session` would deadlock on
                    // the lock we just dropped.
                    if device.inner.auto_reconnect.load(Ordering::Relaxed)
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
