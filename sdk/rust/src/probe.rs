//! Presence without a session: what a program may read while another program
//! owns the board.
//!
//! The identity a client needs — board id, firmware version, lease length, poll
//! interval, capability flags, device id, build — arrives in `GET_FEATURE(14)`,
//! the handshake report [`Session::open`](crate::session) reads before OPEN. A
//! feature report is a control transfer: it never touches the interrupt IN
//! queue, so a second process can read it at any time without stealing a reply
//! from the program that holds the session.
//!
//! Everything else in this SDK is the opposite. A command consumes the shared IN
//! queue, and a second reader that drains a reply the owner was waiting for kills
//! that owner's session. So this module opens no session and sends no packet:
//! [`Probe`] is read-only by construction, and an [`Observer`] refuses every
//! command with `Unsupported` instead of trying.
//!
//! ```no_run
//! let boards = gdb::probe::all().unwrap_or_default();
//! for board in &boards {
//!     println!("{} {} {}", board.serial, board.board, board.firmware);
//! }
//! ```

use crate::device::{DeviceInfo, Target};
use crate::error::{Error, ErrorCode, Result};
use crate::event::ConnectionState;
use crate::log::Level;
use crate::manager::{DeviceEntry, Manager};
use crate::transport::Transport;
use input_protocol::v3::{Capabilities, Command, Packet, SessionInfo, Status, StatusExt};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// How often an [`Observer`] re-reads Feature 14 unless told otherwise.
pub const DEFAULT_INTERVAL: Duration = Duration::from_millis(500);

/// Below this the observer thread would spend its life in control transfers.
const MIN_INTERVAL: Duration = Duration::from_millis(50);

/// What every command method on an [`Observer`] reports.
///
/// The text is part of the contract: a host that logs the refusal, or shows it
/// in a UI, should not have to write its own explanation of why a presence
/// handle cannot drive the board.
pub const OBSERVER_REFUSAL: &str =
    "observer handles read Feature 14 only; it has no session, so it never sends a packet";

/// One board's identity, read from Feature 14 alone.
///
/// The fields are `DeviceInfo`'s, because it is the same handshake report: this
/// is what a `Device` would know after OPEN, obtained without opening anything.
/// Reads Feature 14 only; safe to call while another process holds a session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Probe {
    /// USB serial number, empty when the device reports none.
    pub serial: String,
    /// USB product string, e.g. `GDBOX`.
    pub product: String,
    /// Board id; 2 is the XIAO RP2040.
    pub board: u8,
    /// `major.minor.patch`, e.g. `3.2.0`.
    pub firmware: String,
    /// Device-side lease length in milliseconds.
    pub lease_ms: u16,
    /// Device-side poll interval in milliseconds.
    pub poll_ms: u8,
    /// Capability bits: bit0 NKRO, bit1 consumer, bit2 relative mouse, bit3 concurrency.
    pub capabilities: u32,
    /// Eight identity bytes from the RP2040's unique id.
    pub device_id: [u8; 8],
    /// First four bytes of the firmware's git commit, 0 when unknown.
    pub build: u32,
}

impl Probe {
    /// The same shape `Device::info()` returns.
    pub fn info(&self) -> DeviceInfo {
        DeviceInfo {
            serial: self.serial.clone(),
            product: self.product.clone(),
            board: self.board,
            firmware: self.firmware.clone(),
            lease_ms: self.lease_ms,
            poll_ms: self.poll_ms,
            capabilities: self.capabilities,
            device_id: self.device_id,
            build: self.build,
        }
    }

    /// What the device is, in one line: serial when it has one, product otherwise.
    pub fn label(&self) -> &str {
        if self.serial.is_empty() {
            &self.product
        } else {
            &self.serial
        }
    }
}

/// Reads Feature 14 from the vendor collection at `path`.
///
/// The serial and product come from the current enumeration; a path that is not
/// in it (a device that arrived a moment ago) leaves them empty. The transport
/// is opened for the feature read and dropped again — no OPEN, no report on the
/// IN endpoint.
pub fn read(path: &str) -> Result<Probe> {
    let entry = Manager::list()
        .unwrap_or_default()
        .into_iter()
        .find(|entry| entry.path == path);
    match entry {
        Some(entry) => read_entry(&entry),
        None => read_from(
            crate::manager::open_transport(&Target::Path(path.to_string()))?,
            String::new(),
            String::new(),
        ),
    }
}

/// Reads Feature 14 from the device `selector` names: a serial number, a HID
/// path, a unique fragment of either, or `None` for the first attached board.
pub fn read_selector(selector: Option<&str>) -> Result<Probe> {
    read_entry(&resolve(selector)?)
}

/// Reads every attached vendor collection: one control transfer each, in the
/// order the platform enumerated them.
///
/// A board that vanishes between the listing and its own read is skipped rather
/// than failing the call — the rest of the bus still answered. An empty bus is
/// `Ok(vec![])`; a bus that cannot be enumerated at all is an `Err`.
pub fn all() -> Result<Vec<Probe>> {
    let mut probes = Vec::new();
    for entry in Manager::list()? {
        match read_entry(&entry) {
            Ok(probe) => probes.push(probe),
            Err(error) => crate::sdk_log!(
                Level::Warn,
                "probe skipped path={} code={} ({})",
                entry.path,
                error.code().as_u8(),
                error.message
            ),
        }
    }
    Ok(probes)
}

/// Reads Feature 14 over a transport the caller already has.
///
/// The seam the tests use, and the escape hatch for a host that owns its own HID
/// backend: it must hand over a transport that only answers `feature()`. Nothing
/// here writes, reads the IN endpoint or opens a session.
pub fn read_from(
    mut transport: Box<dyn Transport>,
    serial: impl Into<String>,
    product: impl Into<String>,
) -> Result<Probe> {
    let report = transport
        .feature()?
        .ok_or_else(|| Error::new(ErrorCode::Disconnected, "no Feature 14 reply"))?;
    let capabilities = Capabilities::parse(&report).map_err(|error| {
        Error::new(
            ErrorCode::Framing,
            format!("Feature 14 did not parse: {error}"),
        )
    })?;
    Ok(Probe {
        serial: serial.into(),
        product: product.into(),
        board: capabilities.board,
        firmware: capabilities.firmware,
        lease_ms: capabilities.lease_ms,
        poll_ms: capabilities.poll_ms,
        capabilities: capabilities.flags,
        device_id: capabilities.device_id,
        build: capabilities.build,
    })
}

fn read_entry(entry: &DeviceEntry) -> Result<Probe> {
    read_from(
        crate::manager::open_transport(&Target::Path(entry.path.clone()))?,
        entry.serial.clone(),
        entry.product.clone(),
    )
}

/// The selector rules of [`Manager::open`](crate::Manager::open), so a host that
/// can open a board can also find it without opening one.
fn resolve(selector: Option<&str>) -> Result<DeviceEntry> {
    let devices = Manager::list()?;
    match selector {
        Some(wanted) => devices
            .iter()
            .find(|entry| entry.serial == wanted || entry.path == wanted)
            .or_else(|| devices.iter().find(|entry| entry.serial.contains(wanted)))
            .cloned()
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::Disconnected,
                    format!("no GDBOX device matches {wanted:?}"),
                )
            }),
        None => devices
            .into_iter()
            .next()
            .ok_or_else(|| Error::new(ErrorCode::Disconnected, "no GDBOX device found")),
    }
}

/// What an [`Observer`] saw on one poll.
#[derive(Clone, Debug)]
pub enum ProbeEvent {
    /// The board answered Feature 14: either it is new, or its identity changed.
    Present(Probe),
    /// It did not answer, or it is no longer attached. The observer keeps the
    /// last probe it saw, so a UI does not have to blank itself.
    Gone(Error),
}

/// A presence handle: the board's identity on a timer, and no way to command it.
///
/// This is what a host uses when it must not disturb the program that owns the
/// session — a second tool, a dashboard, a remote host. It polls
/// [`read_selector`] on one thread and holds the answer; every command method is
/// `Unsupported`, because a command would consume the IN queue the owner is
/// reading.
///
/// An unchanged probe produces no event, and a probe that keeps failing produces
/// one `Gone` rather than one per interval. `stop` joins the thread and is safe
/// to call twice; dropping the handle stops it too.
#[derive(Debug)]
pub struct Observer {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

#[derive(Debug)]
struct Shared {
    probe: Mutex<Probe>,
    state: AtomicU8,
    error: Mutex<Option<Error>>,
    control: Control,
}

#[derive(Debug)]
struct Control {
    stopped: Mutex<bool>,
    wake: Condvar,
}

impl Default for Control {
    fn default() -> Self {
        Self {
            stopped: Mutex::new(false),
            wake: Condvar::new(),
        }
    }
}

impl Control {
    fn stop(&self) {
        *lock(&self.stopped) = true;
        self.wake.notify_all();
    }

    fn is_stopped(&self) -> bool {
        *lock(&self.stopped)
    }

    /// Sleeps one interval, returning as soon as `stop` is called.
    fn wait(&self, interval: Duration) {
        let stopped = lock(&self.stopped);
        let _ = self
            .wake
            .wait_timeout_while(stopped, interval, |stopped| !*stopped)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }
}

impl Observer {
    /// Reads Feature 14 once and holds the answer, without starting a thread.
    pub fn open(selector: Option<&str>) -> Result<Self> {
        Ok(Self::holding(read_selector(selector)?))
    }

    /// Polls the first attached board every `interval` and reports changes.
    pub fn start(
        interval: Duration,
        callback: impl FnMut(&ProbeEvent) + Send + 'static,
    ) -> Result<Self> {
        Self::start_with(move || read_selector(None), interval, callback)
    }

    /// Polls the board `selector` names; the selector is resolved again on every
    /// poll, so a renumbered path after a re-plug is not fatal.
    pub fn start_selector(
        selector: Option<&str>,
        interval: Duration,
        callback: impl FnMut(&ProbeEvent) + Send + 'static,
    ) -> Result<Self> {
        let selector = selector.map(str::to_string);
        Self::start_with(
            move || read_selector(selector.as_deref()),
            interval,
            callback,
        )
    }

    /// Polls an arbitrary source. The seam every test uses, and the escape hatch
    /// for a host that reads Feature 14 through its own HID backend.
    pub fn start_with(
        source: impl Fn() -> Result<Probe> + Send + 'static,
        interval: Duration,
        callback: impl FnMut(&ProbeEvent) + Send + 'static,
    ) -> Result<Self> {
        // The first read is the constructor's job: a handle that could not see
        // the board once has nothing to say.
        let first = source()?;
        let mut observer = Self::holding(first);
        let shared = observer.shared.clone();
        let interval = interval.max(MIN_INTERVAL);
        let handle = thread::Builder::new()
            .name("gdb-probe".to_string())
            .spawn(move || run(source, shared, interval, callback))
            .map_err(|error| Error::io(error.to_string()))?;
        observer.thread = Some(handle);
        Ok(observer)
    }

    /// Stops the thread and waits for it. Safe to call twice.
    pub fn stop(&mut self) {
        self.shared.control.stop();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }

    /// True while the polling thread has not been stopped.
    pub fn is_running(&self) -> bool {
        self.thread.is_some()
    }

    /// The last identity the board reported, even when the last poll failed.
    pub fn probe(&self) -> Probe {
        lock(&self.shared.probe).clone()
    }

    /// The same shape `Device::info()` returns, from the last probe.
    pub fn info(&self) -> DeviceInfo {
        self.probe().info()
    }

    /// `Connected` while the last poll answered, `Disconnected` while it failed.
    pub fn state(&self) -> ConnectionState {
        match self.shared.state.load(Ordering::Relaxed) {
            value if value == ConnectionState::Connected as u8 => ConnectionState::Connected,
            _ => ConnectionState::Disconnected,
        }
    }

    /// Why the last poll failed, cleared by the first poll that answers again.
    pub fn last_error(&self) -> Option<Error> {
        lock(&self.shared.error).clone()
    }

    /// The error every command method returns, without calling one.
    pub fn refuse(&self) -> Error {
        Error::unsupported(OBSERVER_REFUSAL)
    }

    /// `Unsupported`: an observer has no session to ask.
    pub fn status(&self) -> Result<Status> {
        Err(self.refuse())
    }

    /// `Unsupported`: an observer has no session to ask.
    pub fn status_ext(&self) -> Result<StatusExt> {
        Err(self.refuse())
    }

    /// `Unsupported`: an observer has no session to ask.
    pub fn sessions(&self) -> Result<Vec<SessionInfo>> {
        Err(self.refuse())
    }

    /// `Unsupported`: a barrier is a packet.
    pub fn barrier(&self) -> Result<()> {
        Err(self.refuse())
    }

    /// `Unsupported`: nothing is held, and the packet would still be one.
    pub fn release_all(&self) -> Result<()> {
        Err(self.refuse())
    }

    /// `Unsupported`: BOOTLOADER is a packet, and the owner would lose its reply.
    pub fn bootloader(&self) -> Result<()> {
        Err(self.refuse())
    }

    /// `Unsupported`: the raw escape hatch is still a packet.
    pub fn command(&self, _command: &Command) -> Result<Option<Packet>> {
        Err(self.refuse())
    }

    /// `Unsupported`: only the process holding the session may reopen it.
    pub fn reconnect(&self) -> Result<()> {
        Err(self.refuse())
    }

    fn holding(probe: Probe) -> Self {
        Self {
            shared: Arc::new(Shared {
                probe: Mutex::new(probe),
                state: AtomicU8::new(ConnectionState::Connected as u8),
                error: Mutex::new(None),
                control: Control::default(),
            }),
            thread: None,
        }
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run(
    source: impl Fn() -> Result<Probe>,
    shared: Arc<Shared>,
    interval: Duration,
    mut callback: impl FnMut(&ProbeEvent),
) {
    let connected = ConnectionState::Connected as u8;
    let disconnected = ConnectionState::Disconnected as u8;
    while !shared.control.is_stopped() {
        match source() {
            Ok(probe) => {
                let changed = {
                    let mut last = lock(&shared.probe);
                    if *last == probe {
                        false
                    } else {
                        *last = probe.clone();
                        true
                    }
                };
                let was = shared.state.swap(connected, Ordering::Relaxed);
                *lock(&shared.error) = None;
                if changed || was != connected {
                    callback(&ProbeEvent::Present(probe));
                }
            }
            Err(error) => {
                let was = shared.state.swap(disconnected, Ordering::Relaxed);
                *lock(&shared.error) = Some(error.clone());
                if was != disconnected {
                    crate::sdk_log!(
                        Level::Warn,
                        "observer lost the board: code={} ({})",
                        error.code().as_u8(),
                        error.message
                    );
                    callback(&ProbeEvent::Gone(error));
                }
            }
        }
        shared.control.wait(interval);
    }
}

fn lock<T>(cell: &Mutex<T>) -> MutexGuard<'_, T> {
    cell.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
