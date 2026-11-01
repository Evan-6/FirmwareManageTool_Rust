//! One device, many processes.
//!
//! Only one process may read the vendor collection; a second reader steals the
//! IN replies and the loser's session dies. A broker owns the device and hands
//! it out over a local pipe so several programs can drive it at once.
//!
//! ```text
//! tool ───┐
//! script ─┼── \\.\pipe\gdbox-default ── Server ── Device ── vendor HID
//! test ───┘
//! ```
//!
//! # Wire format
//!
//! One JSON object per line, UTF-8, terminated by `\n`. A request carries an
//! `id` that the reply echoes; events carry no `id` and may arrive at any time.
//!
//! ```text
//! request  {"id":1,"op":"status","args":{}}
//! reply    {"id":1,"ok":true,"result":{...}}
//! reply    {"id":1,"ok":false,"error":{"code":7,"message":"device queue overflow"}}
//! event    {"event":"state","state":"connected"}
//! event    {"event":"error","code":3,"message":"device disconnected"}
//! ```
//!
//! `code` is the numeric [`ErrorCode`] the C ABI uses, so every binding maps it
//! the same way. `state` is `disconnected`, `connecting`, `connected` or
//! `error`.
//!
//! | op              | args                                            | result                    |
//! |-----------------|-------------------------------------------------|---------------------------|
//! | `info`          | `{}`                                            | device info               |
//! | `status`        | `{}`                                            | STATUS block              |
//! | `status_ext`    | `{}`                                            | STATUS_EXT block          |
//! | `sessions`      | `{}`                                            | `[session, ...]`          |
//! | `key`           | `{"page":7,"usage":4,"down":true}`              | `null`                    |
//! | `mouse_move`    | `{"dx":10,"dy":-4}`                             | `null`                    |
//! | `move_by`       | `{"dx":10,"dy":-4,"durationMs":120}`            | `null`                    |
//! | `move_raw`      | `{"dx":800,"dy":-300}`                          | `null`                    |
//! | `mouse_buttons` | `{"buttons":1}`                                 | `null`                    |
//! | `wheel`         | `{"vertical":-120,"horizontal":0}`              | `null`                    |
//! | `barrier`       | `{}`                                            | `null`                    |
//! | `release_all`   | `{}`                                            | `null`                    |
//! | `command`       | `{"command":{"kind":"wheel","vertical":1,...}}`  | `{"packet":packet\|null}` |
//! | `close`         | `{}`                                            | `null`, then the server hangs up |
//!
//! The device info object is `serial, product, board, firmware, leaseMs,
//! pollMs, capabilities, deviceId, build`. The status block is `received,
//! completed, pending, leaseMs, rx, tx, hid, failsafe, state`, with `state` =
//! `{modifiers, keys:[28], consumer, buttons}`. STATUS_EXT blocks and session
//! entries are the camelCase shapes of [`input_protocol::v3::StatusExt`] and
//! [`input_protocol::v3::SessionInfo`]. A packet is `opcode, flags, session,
//! sequence, payload:[u8]`.
//!
//! A `command` is tagged by `kind`: `open`, `close`, `heartbeat`, `status`,
//! `barrier`, `release_all`, `status_ext`, `session_list`, `bootloader`,
//! `key {page,usage,down}`, `snapshot {state}`, `mouse_move {dx,dy}`,
//! `mouse_buttons {buttons}`, `wheel {vertical,horizontal}`. `key` defaults
//! `page` to 7 and `down` to true, so `{"usage":4}` is a tap.
//!
//! A line the server cannot parse ends that client's connection and nothing
//! else; an op it does not know is answered with `code: 8` and the connection
//! stays up. Every new connection is greeted with the current state event
//! before any reply, so a late joiner knows where the device is.
//!
//! # Fairness
//!
//! Every command runs to completion while holding one lock, so each command is
//! atomic but a *sequence* of commands from one client is not: another client's
//! commands may run in between. The device session is single — all clients
//! share the broker's one owner slot — so "hold key A, move, release key A"
//! must be ordered by the client, usually with [`Client::barrier`] in between.
//!
//! # What a second process loses by design
//!
//! A client gets no raw HID reports, no `GET_FEATURE` capabilities scan, no
//! host-side motion worker (`start_move_to`, drags, chords built from text), no
//! flashing and no bootloader: those need the collection itself or the process
//! that owns it. What it keeps is every operation in the table above plus the
//! connection state and error events. Long moves are paced inside the broker,
//! so a client that disconnects mid-move does not stop the move; it only loses
//! the reply.

use crate::device::{Device, DeviceInfo, DeviceOptions};
use crate::error::{Error, ErrorCode, Result};
use crate::event::{ConnectionState, EventHandler};
use crate::log::Level;
use crate::manager::Manager;
use crate::motion::MotionOptions;
use input_protocol::v3::{Command, Packet, SessionInfo, Status, StatusExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Used when a caller does not name the pipe.
pub const DEFAULT_NAME: &str = "default";

/// The pipe path a broker name maps to, e.g. `\\.\pipe\gdbox-default`.
pub fn pipe_path(name: &str) -> String {
    format!(r"\\.\pipe\gdbox-{name}")
}

/// How long a call waits for its reply before reporting a timeout.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// A client that never reads must not stall the broker's broadcasts forever.
const EVENT_WRITE_MS: u32 = 250;
/// A reply is small; if this much cannot be written, the peer is gone.
const REPLY_WRITE_MS: u32 = 2_000;

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name.contains('\\') || name.contains('/') || name.contains('\0') {
        return Err(Error::invalid(format!(
            "broker name {name:?} must be non-empty and free of path separators"
        )));
    }
    Ok(())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn json_error(error: serde_json::Error) -> Error {
    Error::new(ErrorCode::Framing, error.to_string())
}

fn state_name(state: ConnectionState) -> &'static str {
    match state {
        ConnectionState::Disconnected => "disconnected",
        ConnectionState::Connecting => "connecting",
        ConnectionState::Connected => "connected",
        ConnectionState::Error => "error",
    }
}

fn state_from_name(name: &str) -> ConnectionState {
    match name {
        "connected" => ConnectionState::Connected,
        "connecting" => ConnectionState::Connecting,
        "error" => ConnectionState::Error,
        _ => ConnectionState::Disconnected,
    }
}

fn state_line(state: ConnectionState) -> String {
    json!({"event": "state", "state": state_name(state)}).to_string()
}

fn error_line(error: &Error) -> String {
    json!({"event": "error", "code": error.code.as_u8(), "message": error.message}).to_string()
}

fn ok_line(id: u64, result: Value) -> String {
    json!({"id": id, "ok": true, "result": result}).to_string()
}

fn err_line(id: u64, error: &Error) -> String {
    json!({
        "id": id,
        "ok": false,
        "error": {"code": error.code.as_u8(), "message": error.message},
    })
    .to_string()
}

// ---------------------------------------------------------------- wire types

#[derive(Deserialize)]
struct Request {
    #[serde(default)]
    id: u64,
    op: String,
    #[serde(default)]
    args: Value,
}

fn parse_args<T: serde::de::DeserializeOwned>(args: &Value) -> Result<T> {
    let value = match args {
        Value::Null => Value::Object(serde_json::Map::new()),
        other => other.clone(),
    };
    serde_json::from_value(value).map_err(|error| Error::invalid(format!("bad arguments: {error}")))
}

fn keyboard_page() -> u16 {
    7
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
struct KeyArgs {
    #[serde(default = "keyboard_page")]
    page: u16,
    usage: u16,
    #[serde(default = "yes")]
    down: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MouseMoveArgs {
    dx: i16,
    dy: i16,
}

#[derive(Deserialize)]
struct DeltaArgs {
    dx: i32,
    dy: i32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MoveByArgs {
    dx: i32,
    dy: i32,
    #[serde(default)]
    duration_ms: u32,
    #[serde(default)]
    profile: Option<gdb_motion::HumanProfile>,
}

#[derive(Deserialize)]
struct ButtonsArgs {
    buttons: u8,
}

#[derive(Deserialize)]
struct WheelArgs {
    vertical: i32,
    horizontal: i32,
}

#[derive(Deserialize)]
struct CommandArgs {
    command: CommandWire,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InfoWire {
    serial: String,
    product: String,
    board: u8,
    firmware: String,
    lease_ms: u16,
    poll_ms: u8,
    capabilities: u32,
    device_id: [u8; 8],
    build: u32,
}

impl From<DeviceInfo> for InfoWire {
    fn from(info: DeviceInfo) -> Self {
        Self {
            serial: info.serial,
            product: info.product,
            board: info.board,
            firmware: info.firmware,
            lease_ms: info.lease_ms,
            poll_ms: info.poll_ms,
            capabilities: info.capabilities,
            device_id: info.device_id,
            build: info.build,
        }
    }
}

impl From<InfoWire> for DeviceInfo {
    fn from(info: InfoWire) -> Self {
        Self {
            serial: info.serial,
            product: info.product,
            board: info.board,
            firmware: info.firmware,
            lease_ms: info.lease_ms,
            poll_ms: info.poll_ms,
            capabilities: info.capabilities,
            device_id: info.device_id,
            build: info.build,
        }
    }
}

/// [`Status`] keeps no serde derives; this is its wire twin.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatusWire {
    received: u32,
    completed: u32,
    pending: u8,
    lease_ms: u16,
    rx: u16,
    tx: u16,
    hid: u16,
    failsafe: u16,
    state: input_protocol::v3::InputState,
}

impl From<Status> for StatusWire {
    fn from(status: Status) -> Self {
        Self {
            received: status.received,
            completed: status.completed,
            pending: status.pending,
            lease_ms: status.lease_ms,
            rx: status.rx,
            tx: status.tx,
            hid: status.hid,
            failsafe: status.failsafe,
            state: status.state,
        }
    }
}

impl From<StatusWire> for Status {
    fn from(status: StatusWire) -> Self {
        Self {
            received: status.received,
            completed: status.completed,
            pending: status.pending,
            lease_ms: status.lease_ms,
            rx: status.rx,
            tx: status.tx,
            hid: status.hid,
            failsafe: status.failsafe,
            state: status.state,
        }
    }
}

/// [`Packet`] keeps no serde derives; this is its wire twin.
#[derive(Serialize, Deserialize)]
struct PacketWire {
    opcode: u8,
    flags: u8,
    session: u32,
    sequence: u32,
    payload: Vec<u8>,
}

impl From<Packet> for PacketWire {
    fn from(packet: Packet) -> Self {
        Self {
            opcode: packet.opcode,
            flags: packet.flags,
            session: packet.session,
            sequence: packet.sequence,
            payload: packet.payload,
        }
    }
}

impl From<PacketWire> for Packet {
    fn from(packet: PacketWire) -> Self {
        Self {
            opcode: packet.opcode,
            flags: packet.flags,
            session: packet.session,
            sequence: packet.sequence,
            payload: packet.payload,
        }
    }
}

/// [`Command`] keeps no serde derives; this is its wire twin, tagged by `kind`.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CommandWire {
    Open,
    Close,
    Heartbeat,
    Status,
    Barrier,
    ReleaseAll,
    StatusExt,
    SessionList,
    Bootloader,
    Key {
        page: u16,
        usage: u16,
        down: bool,
    },
    Snapshot {
        state: input_protocol::v3::InputState,
    },
    MouseMove {
        dx: i16,
        dy: i16,
    },
    MouseButtons {
        buttons: u8,
    },
    Wheel {
        vertical: i16,
        horizontal: i16,
    },
}

impl From<&Command> for CommandWire {
    fn from(command: &Command) -> Self {
        match command {
            Command::Open => Self::Open,
            Command::Close => Self::Close,
            Command::Heartbeat => Self::Heartbeat,
            Command::Status => Self::Status,
            Command::Barrier => Self::Barrier,
            Command::ReleaseAll => Self::ReleaseAll,
            Command::StatusExt => Self::StatusExt,
            Command::SessionList => Self::SessionList,
            Command::Bootloader => Self::Bootloader,
            Command::Key(key, down) => Self::Key {
                page: key.usage_page,
                usage: key.usage,
                down: *down,
            },
            Command::Snapshot(state) => Self::Snapshot {
                state: state.clone(),
            },
            Command::MouseMove(dx, dy) => Self::MouseMove { dx: *dx, dy: *dy },
            Command::MouseButtons(buttons) => Self::MouseButtons { buttons: *buttons },
            Command::Wheel(vertical, horizontal) => Self::Wheel {
                vertical: *vertical,
                horizontal: *horizontal,
            },
        }
    }
}

impl From<CommandWire> for Command {
    fn from(command: CommandWire) -> Self {
        match command {
            CommandWire::Open => Self::Open,
            CommandWire::Close => Self::Close,
            CommandWire::Heartbeat => Self::Heartbeat,
            CommandWire::Status => Self::Status,
            CommandWire::Barrier => Self::Barrier,
            CommandWire::ReleaseAll => Self::ReleaseAll,
            CommandWire::StatusExt => Self::StatusExt,
            CommandWire::SessionList => Self::SessionList,
            CommandWire::Bootloader => Self::Bootloader,
            CommandWire::Key { page, usage, down } => Self::Key(
                input_protocol::KeyId {
                    usage_page: page,
                    usage,
                },
                down,
            ),
            CommandWire::Snapshot { state } => Self::Snapshot(state),
            CommandWire::MouseMove { dx, dy } => Self::MouseMove(dx, dy),
            CommandWire::MouseButtons { buttons } => Self::MouseButtons(buttons),
            CommandWire::Wheel {
                vertical,
                horizontal,
            } => Self::Wheel(vertical, horizontal),
        }
    }
}

// --------------------------------------------------------------------- events

/// Something the broker told the client, or the end of the connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    State(ConnectionState),
    Error(Error),
    /// The pipe ended: the broker shut down, or this client closed.
    Closed,
}

impl Event {
    fn parse(value: &Value) -> Option<Self> {
        match value.get("event")?.as_str()? {
            "state" => Some(Self::State(state_from_name(value.get("state")?.as_str()?))),
            "error" => {
                let code = value.get("code").and_then(Value::as_u64).unwrap_or(10) as u8;
                let message = value
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                Some(Self::Error(Error::new(ErrorCode::from_u8(code), message)))
            }
            "closed" => Some(Self::Closed),
            _ => None,
        }
    }
}

#[derive(Default)]
struct EventQueueState {
    events: VecDeque<Event>,
    closed: bool,
}

#[derive(Default)]
struct EventQueue {
    state: Mutex<EventQueueState>,
    ready: Condvar,
}

impl EventQueue {
    fn push(&self, event: Event) {
        let mut state = lock(&self.state);
        if state.closed {
            return;
        }
        state.events.push_back(event);
        drop(state);
        self.ready.notify_all();
    }

    fn close(&self) {
        lock(&self.state).closed = true;
        self.ready.notify_all();
    }

    fn pop(&self, timeout: Option<Duration>) -> Option<Event> {
        let mut state = lock(&self.state);
        let deadline = timeout.map(|timeout| Instant::now() + timeout);
        loop {
            if let Some(event) = state.events.pop_front() {
                return Some(event);
            }
            if state.closed {
                return None;
            }
            state = match deadline {
                Some(deadline) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return None;
                    }
                    let (guard, waited) = self
                        .ready
                        .wait_timeout(state, remaining)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if waited.timed_out() && guard.events.is_empty() {
                        return None;
                    }
                    guard
                }
                None => self
                    .ready
                    .wait(state)
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            };
        }
    }
}

/// The event half of a [`Client`]; clone it and read it from any thread.
#[derive(Clone)]
pub struct EventReceiver {
    queue: Arc<EventQueue>,
}

impl EventReceiver {
    /// Blocks until an event arrives. `None` once the connection is gone and
    /// every queued event has been read.
    pub fn recv(&self) -> Option<Event> {
        self.queue.pop(None)
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Option<Event> {
        self.queue.pop(Some(timeout))
    }

    pub fn try_recv(&self) -> Option<Event> {
        self.queue.pop(Some(Duration::ZERO))
    }
}

// --------------------------------------------------------------------- server

/// Where the broker's one device comes from.
pub enum Source {
    /// The first matching vendor-HID device, retried until one is present.
    Auto,
    /// The device whose serial or HID path matches, retried until it appears.
    Selector(String),
    /// An already-open device, typically a mock or an in-process handle.
    Device(Device),
}

struct ClientEntry {
    token: u64,
    conn: Arc<pipe::Connection>,
}

struct Inner {
    name: String,
    device: Mutex<Option<Device>>,
    clients: Mutex<Vec<ClientEntry>>,
    stop: pipe::Stop,
    state: AtomicU32,
    accept: Mutex<Option<JoinHandle<()>>>,
    workers: Mutex<Vec<JoinHandle<()>>>,
    tokens: AtomicU64,
}

impl Inner {
    fn broadcast(&self, line: &str) {
        let clients: Vec<Arc<pipe::Connection>> = lock(&self.clients)
            .iter()
            .map(|entry| entry.conn.clone())
            .collect();
        for conn in clients {
            // A client that cannot take 250 ms of bytes is gone; its own thread
            // notices and removes it.
            let _ = conn.write_line(line, EVENT_WRITE_MS);
        }
    }

    fn set_state(&self, state: ConnectionState) {
        let code = match state {
            ConnectionState::Disconnected => 0,
            ConnectionState::Connecting => 1,
            ConnectionState::Connected => 2,
            ConnectionState::Error => 3,
        };
        self.state.store(code, Ordering::Relaxed);
    }

    fn state(&self) -> ConnectionState {
        match self.state.load(Ordering::Relaxed) {
            1 => ConnectionState::Connecting,
            2 => ConnectionState::Connected,
            3 => ConnectionState::Error,
            _ => ConnectionState::Disconnected,
        }
    }

    fn forget(&self, token: u64) {
        lock(&self.clients).retain(|entry| entry.token != token);
    }

    /// One command, start to finish, behind the device lock.
    fn dispatch(&self, op: &str, args: &Value) -> Result<Value> {
        if op == "close" {
            return Ok(Value::Null);
        }
        let slot = lock(&self.device);
        let device = slot
            .as_ref()
            .ok_or_else(|| Error::disconnected("broker has no device yet"))?;
        match op {
            "info" => Ok(serde_json::to_value(InfoWire::from(device.info())).map_err(json_error)?),
            "status" => {
                let status = device.status()?;
                Ok(serde_json::to_value(StatusWire::from(status)).map_err(json_error)?)
            }
            "status_ext" => {
                let ext = device.status_ext()?;
                Ok(serde_json::to_value(ext).map_err(json_error)?)
            }
            "sessions" => {
                let sessions = device.sessions()?;
                Ok(serde_json::to_value(sessions).map_err(json_error)?)
            }
            "key" => {
                let args: KeyArgs = parse_args(args)?;
                let keyboard = device.keyboard();
                if args.down {
                    keyboard.key_down(args.page, args.usage)?;
                } else {
                    keyboard.key_up(args.page, args.usage)?;
                }
                Ok(Value::Null)
            }
            "mouse_move" => {
                let args: MouseMoveArgs = parse_args(args)?;
                device.command(&Command::MouseMove(args.dx, args.dy))?;
                Ok(Value::Null)
            }
            "move_raw" => {
                let args: DeltaArgs = parse_args(args)?;
                device.mouse().move_raw(args.dx, args.dy)?;
                Ok(Value::Null)
            }
            "move_by" => {
                let args: MoveByArgs = parse_args(args)?;
                // Relative moves ignore the screen anchor, so it keeps its
                // default here.
                let options = MotionOptions {
                    duration_ms: args.duration_ms,
                    profile: args.profile.unwrap_or_default(),
                    ..MotionOptions::default()
                };
                device.mouse().move_by(args.dx, args.dy, options)?;
                Ok(Value::Null)
            }
            "mouse_buttons" => {
                let args: ButtonsArgs = parse_args(args)?;
                // Keep the device's own mask in step so a later SDK-side click
                // computes from the truth, not from a stale copy.
                device.set_buttons(args.buttons)?;
                device.send_input(&Command::MouseButtons(args.buttons))?;
                Ok(Value::Null)
            }
            "wheel" => {
                let args: WheelArgs = parse_args(args)?;
                device.mouse().wheel(args.vertical, args.horizontal)?;
                Ok(Value::Null)
            }
            "barrier" => {
                device.barrier()?;
                Ok(Value::Null)
            }
            "release_all" => {
                device.release_all()?;
                Ok(Value::Null)
            }
            "command" => {
                let args: CommandArgs = parse_args(args)?;
                let reply = device.command(&Command::from(args.command))?;
                let packet = match reply {
                    Some(packet) => {
                        serde_json::to_value(PacketWire::from(packet)).map_err(json_error)?
                    }
                    None => Value::Null,
                };
                Ok(json!({ "packet": packet }))
            }
            other => Err(Error::unsupported(format!("unknown broker op {other:?}"))),
        }
    }
}

/// The device's own notifications, fanned out to every connected client.
struct Broadcast {
    inner: Weak<Inner>,
}

impl EventHandler for Broadcast {
    fn state(&self, state: ConnectionState) {
        if let Some(inner) = self.inner.upgrade() {
            inner.set_state(state);
            inner.broadcast(&state_line(state));
        }
    }

    fn error(&self, error: &Error) {
        if let Some(inner) = self.inner.upgrade() {
            inner.broadcast(&error_line(error));
        }
    }
}

/// One thread per client: read a line, run one command, answer, repeat.
fn serve(inner: &Arc<Inner>, conn: &Arc<pipe::Connection>) {
    let mut buffer = Vec::new();
    while !inner.stop.is_set() {
        let line = match conn.read_line(&mut buffer, &inner.stop) {
            Ok(Some(line)) => line,
            Ok(None) | Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let request: Request = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(_) => {
                // Not a request: this peer speaks something else. Drop it, and
                // only it.
                crate::sdk_log!(Level::Warn, "broker: dropping a client that sent garbage");
                break;
            }
        };
        let closing = request.op == "close";
        let reply = match inner.dispatch(&request.op, &request.args) {
            Ok(result) => ok_line(request.id, result),
            Err(error) => err_line(request.id, &error),
        };
        if conn.write_line(&reply, REPLY_WRITE_MS).is_err() {
            break;
        }
        if closing {
            break;
        }
    }
    conn.close();
}

fn accept_loop(inner: Arc<Inner>, mut listener: pipe::Listener) {
    while !inner.stop.is_set() {
        let conn = match listener.accept(&inner.stop) {
            Ok(conn) => conn,
            Err(error) => {
                if inner.stop.is_set() || error.code() == ErrorCode::Cancelled {
                    break;
                }
                crate::sdk_log!(Level::Warn, "broker: accept failed: {error}");
                inner.stop.wait(Duration::from_millis(100));
                continue;
            }
        };
        if inner.stop.is_set() {
            conn.close();
            break;
        }
        let token = inner.tokens.fetch_add(1, Ordering::Relaxed);
        let conn = Arc::new(conn);
        lock(&inner.clients).push(ClientEntry {
            token,
            conn: conn.clone(),
        });
        // A client that joins late still learns where the device is.
        let _ = conn.write_line(&state_line(inner.state()), EVENT_WRITE_MS);
        let worker = inner.clone();
        let handle = thread::spawn(move || {
            serve(&worker, &conn);
            worker.forget(token);
        });
        lock(&inner.workers).push(handle);
    }
}

fn connect_loop(inner: Arc<Inner>, selector: Option<String>) {
    while !inner.stop.is_set() {
        match Manager::open(selector.as_deref(), DeviceOptions::default()) {
            Ok(device) => {
                device.set_event_handler(Some(Arc::new(Broadcast {
                    inner: Arc::downgrade(&inner),
                })));
                *lock(&inner.device) = Some(device);
                inner.set_state(ConnectionState::Connected);
                inner.broadcast(&state_line(ConnectionState::Connected));
                return;
            }
            Err(error) => {
                crate::sdk_log!(Level::Info, "broker: no device yet ({error}); retrying");
                inner.stop.wait(Duration::from_millis(500));
            }
        }
    }
}

/// A running broker: it owns the device and serves the pipe.
///
/// The device keeps its own 500 ms heartbeat and reconnect path; the broker adds
/// the pipe, the command serialisation and the event fan-out.
pub struct Server {
    inner: Arc<Inner>,
}

impl Server {
    /// Creates the pipe and starts serving. The pipe exists when this returns,
    /// so [`Client::connect`] immediately afterwards succeeds.
    pub fn new(name: impl Into<String>, source: Source) -> Result<Self> {
        let name = name.into();
        validate_name(&name)?;
        let listener = pipe::Listener::new(&pipe_path(&name))?;
        let stop = pipe::Stop::new()?;
        let inner = Arc::new(Inner {
            name,
            device: Mutex::new(None),
            clients: Mutex::new(Vec::new()),
            stop,
            state: AtomicU32::new(0),
            accept: Mutex::new(None),
            workers: Mutex::new(Vec::new()),
            tokens: AtomicU64::new(1),
        });
        match source {
            Source::Device(device) => {
                device.set_event_handler(Some(Arc::new(Broadcast {
                    inner: Arc::downgrade(&inner),
                })));
                inner.set_state(ConnectionState::Connected);
                *lock(&inner.device) = Some(device);
            }
            Source::Auto => {
                let connector = inner.clone();
                let handle = thread::spawn(move || connect_loop(connector, None));
                lock(&inner.workers).push(handle);
            }
            Source::Selector(selector) => {
                let connector = inner.clone();
                let handle = thread::spawn(move || connect_loop(connector, Some(selector)));
                lock(&inner.workers).push(handle);
            }
        }
        let serving = inner.clone();
        let handle = thread::spawn(move || accept_loop(serving, listener));
        *lock(&inner.accept) = Some(handle);
        crate::sdk_log!(Level::Info, "broker: serving {}", inner.name);
        Ok(Self { inner })
    }

    pub fn name(&self) -> &str {
        &self.inner.name
    }

    pub fn state(&self) -> ConnectionState {
        self.inner.state()
    }

    pub fn clients(&self) -> usize {
        lock(&self.inner.clients).len()
    }

    /// Blocks until the broker stops serving.
    pub fn wait(&self) {
        let accept = lock(&self.inner.accept).take();
        if let Some(handle) = accept
            && thread::current().id() != handle.thread().id()
        {
            let _ = handle.join();
        }
        let workers = std::mem::take(&mut *lock(&self.inner.workers));
        for handle in workers {
            if thread::current().id() != handle.thread().id() {
                let _ = handle.join();
            }
        }
    }

    /// Stops accepting, ends every client connection and releases the device.
    /// Idempotent.
    pub fn shutdown(&self) {
        self.inner.stop.signal();
        self.wait();
        if let Some(device) = lock(&self.inner.device).take() {
            let _ = device.close();
        }
        lock(&self.inner.clients).clear();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// --------------------------------------------------------------------- client

enum Reply {
    Ok(Value),
    Err(Error),
}

struct Shared {
    conn: pipe::Connection,
    stop: pipe::Stop,
    events: Arc<EventQueue>,
    pending: Mutex<HashMap<u64, mpsc::Sender<Reply>>>,
    closed: AtomicBool,
}

impl Shared {
    fn closed_error() -> Error {
        Error::disconnected("broker closed the connection")
    }

    fn fail_pending(&self, error: &Error) {
        let waiters: Vec<mpsc::Sender<Reply>> = lock(&self.pending)
            .drain()
            .map(|(_, sender)| sender)
            .collect();
        for sender in waiters {
            let _ = sender.send(Reply::Err(error.clone()));
        }
    }

    fn handle_line(&self, line: &str) {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return;
        };
        if value.get("event").is_some() {
            if let Some(event) = Event::parse(&value) {
                self.events.push(event);
            }
            return;
        }
        let Some(id) = value.get("id").and_then(Value::as_u64) else {
            return;
        };
        let Some(sender) = lock(&self.pending).remove(&id) else {
            return;
        };
        let reply = if value.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            Reply::Ok(value.get("result").cloned().unwrap_or(Value::Null))
        } else {
            let code = value
                .get("error")
                .and_then(|error| error.get("code"))
                .and_then(Value::as_u64)
                .unwrap_or(10) as u8;
            let message = value
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("broker reported an error")
                .to_string();
            Reply::Err(Error::new(ErrorCode::from_u8(code), message))
        };
        let _ = sender.send(reply);
    }

    fn read_loop(self: Arc<Self>) {
        let mut buffer = Vec::new();
        while let Ok(Some(line)) = self.conn.read_line(&mut buffer, &self.stop) {
            self.handle_line(&line);
        }
        self.closed.store(true, Ordering::SeqCst);
        self.fail_pending(&Self::closed_error());
        self.events.push(Event::Closed);
        self.events.close();
        self.conn.close();
    }
}

/// A handle on a broker process.
///
/// Every method takes `&self`, so one client can be shared between threads;
/// replies are matched by request id, not by arrival order.
pub struct Client {
    shared: Arc<Shared>,
    reader: Mutex<Option<JoinHandle<()>>>,
    next_id: AtomicU64,
    timeout: Duration,
}

impl Client {
    /// Connects to `\\.\pipe\gdbox-<name>`.
    pub fn connect(name: impl AsRef<str>) -> Result<Self> {
        Self::connect_timeout(name, Duration::from_millis(250))
    }

    /// Connects, waiting up to `wait` for a busy pipe to free an instance.
    pub fn connect_timeout(name: impl AsRef<str>, wait: Duration) -> Result<Self> {
        let name = name.as_ref();
        validate_name(name)?;
        let conn = pipe::connect(&pipe_path(name), wait)?;
        let stop = pipe::Stop::new()?;
        let shared = Arc::new(Shared {
            conn,
            stop,
            events: Arc::new(EventQueue::default()),
            pending: Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
        });
        let reading = shared.clone();
        let reader = thread::spawn(move || reading.read_loop());
        crate::sdk_log!(Level::Info, "broker: client attached to {name}");
        Ok(Self {
            shared,
            reader: Mutex::new(Some(reader)),
            next_id: AtomicU64::new(1),
            timeout: CALL_TIMEOUT,
        })
    }

    /// True when a broker is listening on that name.
    pub fn available(name: impl AsRef<str>) -> bool {
        let name = name.as_ref();
        if validate_name(name).is_err() {
            return false;
        }
        pipe::exists(&pipe_path(name))
    }

    /// Connects to `name`, or starts `exe --gdbox-broker <name>` first.
    ///
    /// The child is detached, so it outlives the caller; the pipe is retried for
    /// up to three seconds.
    pub fn connect_or_spawn(name: impl AsRef<str>, exe: impl AsRef<Path>) -> Result<Self> {
        let name = name.as_ref();
        validate_name(name)?;
        match Self::connect(name) {
            Ok(client) => return Ok(client),
            Err(error) if error.code() != ErrorCode::Disconnected => return Err(error),
            Err(_) => {}
        }
        spawn_broker(exe.as_ref(), name)?;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match Self::connect(name) {
                Ok(client) => return Ok(client),
                Err(error) => {
                    if Instant::now() >= deadline {
                        return Err(Error::timeout(format!(
                            "no broker answered on {name} within 3 s: {error}"
                        )));
                    }
                    thread::sleep(Duration::from_millis(50));
                }
            }
        }
    }

    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    pub fn events(&self) -> EventReceiver {
        EventReceiver {
            queue: self.shared.events.clone(),
        }
    }

    pub fn is_closed(&self) -> bool {
        self.shared.closed.load(Ordering::SeqCst)
    }

    /// Ends this client's connection. Idempotent, and safe while another thread
    /// waits for a reply: that call fails with [`ErrorCode::Disconnected`].
    pub fn close(&self) {
        if self.shared.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.shared.stop.signal();
        let reader = lock(&self.reader).take();
        if let Some(handle) = reader
            && thread::current().id() != handle.thread().id()
        {
            let _ = handle.join();
        }
        self.shared.fail_pending(&Shared::closed_error());
        self.shared.events.close();
        self.shared.conn.close();
    }

    fn call(&self, op: &str, args: Value) -> Result<Value> {
        if self.is_closed() {
            return Err(Error::disconnected("broker client is closed"));
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::channel();
        lock(&self.shared.pending).insert(id, sender);
        let line = json!({"id": id, "op": op, "args": args}).to_string();
        if let Err(error) = self.shared.conn.write_line(&line, REPLY_WRITE_MS) {
            lock(&self.shared.pending).remove(&id);
            return Err(error);
        }
        match receiver.recv_timeout(self.timeout) {
            Ok(Reply::Ok(result)) => Ok(result),
            Ok(Reply::Err(error)) => Err(error),
            Err(RecvTimeoutError::Timeout) => {
                lock(&self.shared.pending).remove(&id);
                Err(Error::timeout(format!(
                    "no reply to {op:?} within {:?}",
                    self.timeout
                )))
            }
            Err(RecvTimeoutError::Disconnected) => Err(Shared::closed_error()),
        }
    }

    pub fn info(&self) -> Result<DeviceInfo> {
        let value = self.call("info", json!({}))?;
        let info: InfoWire = serde_json::from_value(value).map_err(json_error)?;
        Ok(info.into())
    }

    pub fn status(&self) -> Result<Status> {
        let value = self.call("status", json!({}))?;
        let status: StatusWire = serde_json::from_value(value).map_err(json_error)?;
        Ok(status.into())
    }

    pub fn status_ext(&self) -> Result<StatusExt> {
        let value = self.call("status_ext", json!({}))?;
        serde_json::from_value(value).map_err(json_error)
    }

    pub fn sessions(&self) -> Result<Vec<SessionInfo>> {
        let value = self.call("sessions", json!({}))?;
        serde_json::from_value(value).map_err(json_error)
    }

    pub fn key(&self, page: u16, usage: u16, down: bool) -> Result<()> {
        self.call("key", json!({"page": page, "usage": usage, "down": down}))
            .map(|_| ())
    }

    /// One MOUSE_MOVE report; the deltas must fit the protocol's +/-1024.
    pub fn mouse_move(&self, dx: i16, dy: i16) -> Result<()> {
        self.call("mouse_move", json!({"dx": dx, "dy": dy}))
            .map(|_| ())
    }

    /// A paced relative move; `duration_ms: 0` picks a natural speed from the
    /// distance. Relative moves ignore `options.screen`, so it stays home.
    pub fn move_by(&self, dx: i32, dy: i32, options: MotionOptions) -> Result<()> {
        self.call(
            "move_by",
            json!({
                "dx": dx,
                "dy": dy,
                "durationMs": options.duration_ms,
                "profile": options.profile,
            }),
        )
        .map(|_| ())
    }

    /// A relative move of any size, split at the +/-1024 report limit.
    pub fn move_raw(&self, dx: i32, dy: i32) -> Result<()> {
        self.call("move_raw", json!({"dx": dx, "dy": dy}))
            .map(|_| ())
    }

    /// The raw HID button mask: left=1, right=2, middle=4, back=8, forward=16.
    pub fn mouse_buttons(&self, buttons: u8) -> Result<()> {
        self.call("mouse_buttons", json!({"buttons": buttons}))
            .map(|_| ())
    }

    pub fn wheel(&self, vertical: i32, horizontal: i32) -> Result<()> {
        self.call(
            "wheel",
            json!({"vertical": vertical, "horizontal": horizontal}),
        )
        .map(|_| ())
    }

    /// Waits until the device has completed everything queued before it.
    pub fn barrier(&self) -> Result<()> {
        self.call("barrier", json!({})).map(|_| ())
    }

    pub fn release_all(&self) -> Result<()> {
        self.call("release_all", json!({})).map(|_| ())
    }

    /// The raw escape hatch: any command, exactly as the wire defines it.
    pub fn command(&self, command: &Command) -> Result<Option<Packet>> {
        let value = self.call("command", json!({"command": CommandWire::from(command)}))?;
        let packet = value.get("packet").cloned().unwrap_or(Value::Null);
        if packet.is_null() {
            return Ok(None);
        }
        let packet: PacketWire = serde_json::from_value(packet).map_err(json_error)?;
        Ok(Some(packet.into()))
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.close();
    }
}

impl std::fmt::Debug for Client {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Client")
            .field("closed", &self.is_closed())
            .finish()
    }
}

// ---------------------------------------------------------------- auto-spawn

fn requested_name(args: &[String]) -> Option<String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--gdbox-broker" {
            let name = iter.next().filter(|next| !next.starts_with('-'));
            return Some(match name {
                Some(name) => name.clone(),
                None => DEFAULT_NAME.to_string(),
            });
        }
        if let Some(value) = arg.strip_prefix("--gdbox-broker=") {
            return Some(if value.is_empty() {
                DEFAULT_NAME.to_string()
            } else {
                value.to_string()
            });
        }
    }
    None
}

/// Turns this process into the broker when `--gdbox-broker [name]` is on the
/// command line, then exits. Returns `false` — and touches nothing — otherwise,
/// so a tool's `main` can call it first thing and carry on.
pub fn run_if_requested() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(name) = requested_name(&args) else {
        return false;
    };
    match Server::new(&name, Source::Auto) {
        Ok(server) => {
            eprintln!("gdb broker: serving {} ({name})", pipe_path(&name));
            server.wait();
            server.shutdown();
        }
        Err(error) => {
            eprintln!("gdb broker: cannot serve {name}: {error}");
            std::process::exit(1);
        }
    }
    std::process::exit(0);
}

#[cfg(windows)]
fn spawn_broker(exe: &Path, name: &str) -> Result<()> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    Command::new(exe)
        .arg("--gdbox-broker")
        .arg(name)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .map(|_| ())
        .map_err(|error| Error::io(format!("cannot spawn broker {}: {error}", exe.display())))
}

#[cfg(not(windows))]
fn spawn_broker(_exe: &Path, _name: &str) -> Result<()> {
    Err(Error::unsupported("the broker needs Windows named pipes"))
}

// ---------------------------------------------------------------- named pipes

#[cfg(windows)]
mod pipe {
    //! Named-pipe I/O without a new dependency.
    //!
    //! `std` does not expose pipes and this crate's `windows-sys` features stop
    //! short of `Win32_System_Pipes`, so the handful of calls used here are
    //! declared below. Handles are `isize`, which keeps every struct `Send` and
    //! `Sync` without unsafe impls; pointers and integers are passed identically
    //! on both Windows targets.

    use super::{Error, ErrorCode, Result};
    use std::ffi::c_void;
    use std::ptr;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    const INVALID: isize = -1;
    const ERROR_FILE_NOT_FOUND: u32 = 2;
    const ERROR_PATH_NOT_FOUND: u32 = 3;
    const ERROR_ACCESS_DENIED: u32 = 5;
    const ERROR_BROKEN_PIPE: u32 = 109;
    const ERROR_PIPE_BUSY: u32 = 231;
    const ERROR_NO_DATA: u32 = 232;
    const ERROR_PIPE_NOT_CONNECTED: u32 = 233;
    const ERROR_IO_PENDING: u32 = 997;
    const ERROR_PIPE_CONNECTED: u32 = 535;
    const ERROR_OPERATION_ABORTED: u32 = 995;
    const WAIT_OBJECT_0: u32 = 0;
    const WAIT_TIMEOUT: u32 = 258;
    const INFINITE: u32 = u32::MAX;

    const PIPE_ACCESS_DUPLEX: u32 = 0x3;
    const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;
    const PIPE_TYPE_BYTE: u32 = 0x0;
    const PIPE_READMODE_BYTE: u32 = 0x0;
    const PIPE_REJECT_REMOTE_CLIENTS: u32 = 0x8;
    const PIPE_UNLIMITED_INSTANCES: u32 = 255;
    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const FILE_SHARE_READ: u32 = 0x1;
    const FILE_SHARE_WRITE: u32 = 0x2;
    const OPEN_EXISTING: u32 = 3;

    /// A request line longer than this is not a request.
    const MAX_LINE: usize = 256 * 1024;
    const BUFFER_SIZE: u32 = 64 * 1024;

    #[repr(C)]
    #[derive(Default)]
    struct Overlapped {
        internal: usize,
        internal_high: usize,
        offset: u32,
        offset_high: u32,
        event: isize,
    }

    #[link(name = "kernel32")]
    #[allow(clippy::too_many_arguments)]
    unsafe extern "system" {
        fn CreateNamedPipeW(
            name: *const u16,
            open_mode: u32,
            pipe_mode: u32,
            max_instances: u32,
            out_buffer: u32,
            in_buffer: u32,
            default_timeout: u32,
            security: *const c_void,
        ) -> isize;
        fn ConnectNamedPipe(handle: isize, overlapped: *mut Overlapped) -> i32;
        fn DisconnectNamedPipe(handle: isize) -> i32;
        fn ReadFile(
            handle: isize,
            buffer: *mut u8,
            length: u32,
            read: *mut u32,
            overlapped: *mut Overlapped,
        ) -> i32;
        fn WriteFile(
            handle: isize,
            buffer: *const u8,
            length: u32,
            written: *mut u32,
            overlapped: *mut Overlapped,
        ) -> i32;
        fn GetOverlappedResult(
            handle: isize,
            overlapped: *mut Overlapped,
            transferred: *mut u32,
            wait: i32,
        ) -> i32;
        fn CancelIoEx(handle: isize, overlapped: *mut Overlapped) -> i32;
        fn CreateEventW(
            attributes: *const c_void,
            manual_reset: i32,
            initial: i32,
            name: *const u16,
        ) -> isize;
        fn SetEvent(handle: isize) -> i32;
        fn ResetEvent(handle: isize) -> i32;
        fn WaitForSingleObject(handle: isize, milliseconds: u32) -> u32;
        fn WaitForMultipleObjects(
            count: u32,
            handles: *const isize,
            wait_all: i32,
            milliseconds: u32,
        ) -> u32;
        fn CloseHandle(handle: isize) -> i32;
        fn GetLastError() -> u32;
        fn CreateFileW(
            name: *const u16,
            access: u32,
            share: u32,
            security: *const c_void,
            creation: u32,
            flags: u32,
            template: isize,
        ) -> isize;
        fn WaitNamedPipeW(name: *const u16, timeout: u32) -> i32;
    }

    fn last_error() -> u32 {
        unsafe { GetLastError() }
    }

    fn io_error(what: &str, code: u32) -> Error {
        let message = format!("{what} on the broker pipe failed (windows error {code})");
        match code {
            ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND | ERROR_PIPE_BUSY => Error::new(
                ErrorCode::Disconnected,
                format!("no broker is listening: {message}"),
            ),
            ERROR_BROKEN_PIPE | ERROR_NO_DATA | ERROR_PIPE_NOT_CONNECTED => {
                Error::new(ErrorCode::Disconnected, message)
            }
            ERROR_OPERATION_ABORTED => Error::cancelled(),
            ERROR_ACCESS_DENIED => Error::new(ErrorCode::Io, message),
            _ => Error::new(ErrorCode::Io, message),
        }
    }

    fn wide(path: &str) -> Result<Vec<u16>> {
        if path.contains('\0') {
            return Err(Error::invalid("pipe name contains a NUL byte"));
        }
        Ok(path.encode_utf16().chain(std::iter::once(0)).collect())
    }

    /// One pending operation. The OVERLAPPED must not move while it is in
    /// flight, so it lives in a box, and it carries the event the I/O manager
    /// signals on completion — a separate event would never be set.
    struct Io {
        overlapped: Box<Overlapped>,
        event: isize,
    }

    impl Io {
        fn new() -> Result<Self> {
            let event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
            if event == 0 {
                return Err(io_error("CreateEventW", last_error()));
            }
            let mut overlapped: Box<Overlapped> = Box::default();
            overlapped.event = event;
            Ok(Self { overlapped, event })
        }
    }

    impl Drop for Io {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.event);
            }
        }
    }

    /// The stop switch shared by a server and its clients: a manual-reset event
    /// plus a flag, so waits end immediately instead of polling.
    pub(super) struct Stop {
        event: isize,
        done: AtomicBool,
    }

    impl Stop {
        pub(super) fn new() -> Result<Self> {
            let event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
            if event == 0 {
                return Err(io_error("CreateEventW", last_error()));
            }
            Ok(Self {
                event,
                done: AtomicBool::new(false),
            })
        }

        pub(super) fn signal(&self) {
            if !self.done.swap(true, Ordering::SeqCst) {
                unsafe {
                    SetEvent(self.event);
                }
            }
        }

        pub(super) fn is_set(&self) -> bool {
            self.done.load(Ordering::SeqCst)
        }

        pub(super) fn wait(&self, timeout: Duration) {
            unsafe {
                WaitForSingleObject(self.event, timeout.as_millis() as u32);
            }
        }
    }

    impl Drop for Stop {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.event);
            }
        }
    }

    /// One connected pipe end. Reads and writes have their own OVERLAPPED and
    /// their own lock, so a broadcast never disturbs the owner's read.
    pub(super) struct Connection {
        handle: isize,
        read: Mutex<Io>,
        write: Mutex<Io>,
        closed: AtomicBool,
    }

    impl Connection {
        fn new(handle: isize) -> Result<Self> {
            Ok(Self {
                handle,
                read: Mutex::new(Io::new()?),
                write: Mutex::new(Io::new()?),
                closed: AtomicBool::new(false),
            })
        }

        /// One line, without its `\n`. `Ok(None)` is a clean end of stream.
        pub(super) fn read_line(
            &self,
            buffer: &mut Vec<u8>,
            stop: &Stop,
        ) -> Result<Option<String>> {
            loop {
                if let Some(line) = take_line(buffer)? {
                    return Ok(Some(line));
                }
                let mut chunk = [0u8; 4096];
                let count = self.read_some(&mut chunk, stop)?;
                if count == 0 {
                    return Ok(None);
                }
                buffer.extend_from_slice(&chunk[..count]);
                if buffer.len() > MAX_LINE {
                    return Err(Error::new(ErrorCode::Overflow, "broker line too long"));
                }
            }
        }

        pub(super) fn write_line(&self, line: &str, timeout_ms: u32) -> Result<()> {
            let mut framed = Vec::with_capacity(line.len() + 1);
            framed.extend_from_slice(line.as_bytes());
            framed.push(b'\n');
            self.write_all(&framed, timeout_ms)
        }

        fn read_some(&self, chunk: &mut [u8], stop: &Stop) -> Result<usize> {
            let mut io = super::lock(&self.read);
            let mut got: u32 = 0;
            unsafe {
                ResetEvent(io.event);
                if ReadFile(
                    self.handle,
                    chunk.as_mut_ptr(),
                    chunk.len() as u32,
                    &mut got,
                    &mut *io.overlapped,
                ) != 0
                {
                    return Ok(got as usize);
                }
                let code = last_error();
                if code != ERROR_IO_PENDING {
                    return Err(io_error("ReadFile", code));
                }
                let handles = [io.event, stop.event];
                match WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) {
                    WAIT_OBJECT_0 => {
                        if GetOverlappedResult(self.handle, &mut *io.overlapped, &mut got, 0) == 0 {
                            let code = last_error();
                            if code == ERROR_OPERATION_ABORTED
                                || code == ERROR_BROKEN_PIPE
                                || code == ERROR_PIPE_NOT_CONNECTED
                            {
                                return Ok(0);
                            }
                            return Err(io_error("GetOverlappedResult", code));
                        }
                        Ok(got as usize)
                    }
                    code if code == WAIT_OBJECT_0 + 1 => {
                        // Stopping: cancel, reap, then report the end of stream.
                        CancelIoEx(self.handle, &mut *io.overlapped);
                        let mut reaped = 0u32;
                        GetOverlappedResult(self.handle, &mut *io.overlapped, &mut reaped, 1);
                        Err(Error::cancelled())
                    }
                    other => Err(Error::io(format!("broker pipe wait failed ({other})"))),
                }
            }
        }

        fn write_all(&self, data: &[u8], timeout_ms: u32) -> Result<()> {
            let mut io = super::lock(&self.write);
            let mut written = 0usize;
            while written < data.len() {
                let chunk = &data[written..];
                let mut got: u32 = 0;
                unsafe {
                    ResetEvent(io.event);
                    if WriteFile(
                        self.handle,
                        chunk.as_ptr(),
                        chunk.len() as u32,
                        &mut got,
                        &mut *io.overlapped,
                    ) != 0
                    {
                        if got == 0 {
                            return Err(Error::io("broker pipe write made no progress"));
                        }
                        written += got as usize;
                        continue;
                    }
                    let code = last_error();
                    if code != ERROR_IO_PENDING {
                        return Err(io_error("WriteFile", code));
                    }
                    match WaitForSingleObject(io.event, timeout_ms) {
                        WAIT_OBJECT_0 => {
                            if GetOverlappedResult(self.handle, &mut *io.overlapped, &mut got, 0)
                                == 0
                            {
                                return Err(io_error("GetOverlappedResult", last_error()));
                            }
                            if got == 0 {
                                return Err(Error::io("broker pipe write made no progress"));
                            }
                            written += got as usize;
                        }
                        WAIT_TIMEOUT => {
                            CancelIoEx(self.handle, &mut *io.overlapped);
                            return Err(Error::timeout("broker pipe write timed out"));
                        }
                        other => {
                            return Err(Error::io(format!(
                                "broker pipe write wait failed ({other})"
                            )));
                        }
                    }
                }
            }
            Ok(())
        }

        pub(super) fn close(&self) {
            if self.closed.swap(true, Ordering::SeqCst) {
                return;
            }
            unsafe {
                DisconnectNamedPipe(self.handle);
                CloseHandle(self.handle);
            }
        }
    }

    impl Drop for Connection {
        fn drop(&mut self) {
            self.close();
        }
    }

    fn take_line(buffer: &mut Vec<u8>) -> Result<Option<String>> {
        let Some(end) = buffer.iter().position(|byte| *byte == b'\n') else {
            return Ok(None);
        };
        let mut line: Vec<u8> = buffer.drain(..=end).collect();
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        let text = String::from_utf8(line)
            .map_err(|_| Error::new(ErrorCode::Framing, "broker line was not UTF-8"))?;
        Ok(Some(text))
    }

    /// Listens on `path`, one instance at a time, always keeping the next one
    /// ready so a client never sees a momentary `PIPE_BUSY`.
    pub(super) struct Listener {
        name: Vec<u16>,
        pending: Option<isize>,
        connect: Io,
    }

    impl Listener {
        pub(super) fn new(path: &str) -> Result<Self> {
            let name = wide(path)?;
            let connect = Io::new()?;
            let mut listener = Self {
                name,
                pending: None,
                connect,
            };
            listener.pending = Some(listener.create()?);
            Ok(listener)
        }

        fn create(&self) -> Result<isize> {
            let handle = unsafe {
                CreateNamedPipeW(
                    self.name.as_ptr(),
                    PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_REJECT_REMOTE_CLIENTS,
                    PIPE_UNLIMITED_INSTANCES,
                    BUFFER_SIZE,
                    BUFFER_SIZE,
                    0,
                    ptr::null(),
                )
            };
            if handle == INVALID {
                return Err(io_error("CreateNamedPipeW", last_error()));
            }
            Ok(handle)
        }

        fn discard(&self, handle: isize) {
            unsafe {
                DisconnectNamedPipe(handle);
                CloseHandle(handle);
            }
        }

        pub(super) fn accept(&mut self, stop: &Stop) -> Result<Connection> {
            if stop.is_set() {
                return Err(Error::cancelled());
            }
            let handle = match self.pending.take() {
                Some(handle) => handle,
                None => self.create()?,
            };
            unsafe {
                ResetEvent(self.connect.event);
            }
            if unsafe { ConnectNamedPipe(handle, &mut *self.connect.overlapped) } == 0 {
                let code = last_error();
                if code == ERROR_PIPE_CONNECTED {
                    // Connected between CreateNamedPipeW and here: already done.
                } else if code == ERROR_IO_PENDING {
                    let handles = [self.connect.event, stop.event];
                    match unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) } {
                        WAIT_OBJECT_0 => {
                            let mut got = 0u32;
                            if unsafe {
                                GetOverlappedResult(
                                    handle,
                                    &mut *self.connect.overlapped,
                                    &mut got,
                                    0,
                                )
                            } == 0
                            {
                                let error = io_error("ConnectNamedPipe", last_error());
                                self.discard(handle);
                                return Err(error);
                            }
                        }
                        _ => {
                            unsafe {
                                CancelIoEx(handle, &mut *self.connect.overlapped);
                                let mut got = 0u32;
                                GetOverlappedResult(
                                    handle,
                                    &mut *self.connect.overlapped,
                                    &mut got,
                                    1,
                                );
                            }
                            self.discard(handle);
                            return Err(Error::cancelled());
                        }
                    }
                } else {
                    let error = io_error("ConnectNamedPipe", code);
                    self.discard(handle);
                    return Err(error);
                }
            }
            // This instance is busy now, so a fresh one is the only one a client
            // can reach: exactly one listening instance at any time.
            self.pending = self.create().ok();
            match Connection::new(handle) {
                Ok(connection) => Ok(connection),
                Err(error) => {
                    self.discard(handle);
                    Err(error)
                }
            }
        }
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            if let Some(handle) = self.pending.take() {
                self.discard(handle);
            }
        }
    }

    pub(super) fn connect(path: &str, wait: Duration) -> Result<Connection> {
        let name = wide(path)?;
        let deadline = Instant::now() + wait;
        loop {
            let handle = unsafe {
                CreateFileW(
                    name.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    ptr::null(),
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED,
                    0,
                )
            };
            if handle != INVALID {
                return match Connection::new(handle) {
                    Ok(connection) => Ok(connection),
                    Err(error) => {
                        unsafe {
                            CloseHandle(handle);
                        }
                        Err(error)
                    }
                };
            }
            let code = last_error();
            if code != ERROR_PIPE_BUSY {
                return Err(io_error("CreateFileW", code));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::timeout(format!("broker pipe {path} is busy")));
            }
            let slice = remaining.min(Duration::from_millis(100));
            unsafe {
                WaitNamedPipeW(name.as_ptr(), slice.as_millis() as u32);
            }
        }
    }

    /// Probes without keeping the connection: is a broker listening on `path`?
    pub(super) fn exists(path: &str) -> bool {
        let Ok(name) = wide(path) else {
            return false;
        };
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                ptr::null(),
                OPEN_EXISTING,
                0,
                0,
            )
        };
        if handle == INVALID {
            // A busy pipe is still a live broker.
            return last_error() == ERROR_PIPE_BUSY;
        }
        unsafe {
            CloseHandle(handle);
        }
        true
    }
}

#[cfg(not(windows))]
mod pipe {
    //! Named pipes are a Windows feature; the broker reports that plainly.

    use super::{Error, Result};
    use std::time::Duration;

    pub(super) struct Stop;

    impl Stop {
        pub(super) fn new() -> Result<Self> {
            Err(unsupported())
        }
        pub(super) fn signal(&self) {}
        pub(super) fn is_set(&self) -> bool {
            true
        }
        pub(super) fn wait(&self, _timeout: Duration) {}
    }

    pub(super) struct Connection;

    impl Connection {
        pub(super) fn read_line(
            &self,
            _buffer: &mut Vec<u8>,
            _stop: &Stop,
        ) -> Result<Option<String>> {
            Err(unsupported())
        }
        pub(super) fn write_line(&self, _line: &str, _timeout_ms: u32) -> Result<()> {
            Err(unsupported())
        }
        pub(super) fn close(&self) {}
    }

    pub(super) struct Listener;

    impl Listener {
        pub(super) fn new(_path: &str) -> Result<Self> {
            Err(unsupported())
        }
        pub(super) fn accept(&mut self, _stop: &Stop) -> Result<Connection> {
            Err(unsupported())
        }
    }

    pub(super) fn connect(_path: &str, _wait: Duration) -> Result<Connection> {
        Err(unsupported())
    }

    pub(super) fn exists(_path: &str) -> bool {
        false
    }

    fn unsupported() -> Error {
        Error::unsupported("the broker needs Windows named pipes")
    }
}

#[cfg(test)]
mod tests {
    use super::{CommandWire, DEFAULT_NAME, pipe_path, requested_name};
    use input_protocol::v3::Command;

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn pipe_names_are_namespaced_by_broker_name() {
        assert_eq!(pipe_path("default"), r"\\.\pipe\gdbox-default");
        assert_eq!(pipe_path("work"), r"\\.\pipe\gdbox-work");
    }

    #[test]
    fn reads_the_broker_flag_and_its_optional_name() {
        assert_eq!(requested_name(&args("")), None);
        assert_eq!(requested_name(&args("--other")), None);
        assert_eq!(
            requested_name(&args("--gdbox-broker")),
            Some(DEFAULT_NAME.into())
        );
        assert_eq!(
            requested_name(&args("--gdbox-broker work")),
            Some("work".into())
        );
        assert_eq!(
            requested_name(&args("--ui --gdbox-broker=work")),
            Some("work".into())
        );
        assert_eq!(
            requested_name(&args("--gdbox-broker --theme dark")),
            Some(DEFAULT_NAME.into())
        );
    }

    #[test]
    fn commands_round_trip_through_their_wire_form() {
        let commands = [
            Command::Open,
            Command::ReleaseAll,
            Command::Bootloader,
            Command::Key(
                input_protocol::KeyId {
                    usage_page: 7,
                    usage: 0x04,
                },
                true,
            ),
            Command::MouseMove(-1024, 1024),
            Command::MouseButtons(0b101),
            Command::Wheel(1, -1),
            Command::Snapshot(input_protocol::v3::InputState::default()),
        ];
        for command in commands {
            let wire = serde_json::to_value(CommandWire::from(&command)).unwrap();
            let wire: CommandWire = serde_json::from_value(wire).unwrap();
            let restored: Command = wire.into();
            assert_eq!(command.wire().unwrap(), restored.wire().unwrap());
        }
    }
}
