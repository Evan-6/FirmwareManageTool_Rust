# GDBOX API reference

`include/gdb.h` is the contract; this file names the same operations in every
layer. Rust is `gdb`, C++ is `gdb::`, Python is module `gdb`, Go is package
`gdb`. Every failure raises or returns the same `ErrorCode`.

Rust is the implementation and `include/gdb.h` is the ABI every other language
goes through. The C++ header (`sdk/cpp/gdb.hpp`), the Python module
(`sdk/python/gdb.py`) and the Go package (`sdk/go/gdb.go`) are name translation
over that header: same calls, adapted spellings, no behaviour of their own. A
language without a binding calls the `gdb_*` functions directly — C#, JNI, Node
and Godot do exactly what the three bindings do.

`watch`, `metrics` and `replay` are Rust-only, as is choosing a screen for an
absolute move, and so is the broker's server side. They are under their own
headings below and have no counterpart in `gdb.h`, the C++ header, the Python
module or the Go package. A broker *client* does have one: `gdb_attach` and the
`gdb_remote_*` calls carry it, wrapped as `gdb::Remote`, `gdb.Remote` and
`gdb.Attach`. So does presence: `gdb_probe`, `gdb_probe_all` and the
`gdb_observer_*` handle in every layer.

## Naming map

| Concept | Rust | C++ | Python | Go |
| --- | --- | --- | --- | --- |
| open by selector | `Manager::open(sel, opts)` | `Device::open(sel)` | `Device.open(sel)` | `Open(sel, …)` |
| open the mock | `Manager::open_mock_with_handle` | `Device::open_mock` | `Device.open_mock` | `OpenMock` |
| open for a board others drive too | `Device::open_shared(sel, opts)` | `Device::open_shared(sel)` | `Device.open(sel, shared=True)` | `OpenShared(sel, …)` |
| open the shared mock | `Device::open_mock_shared(opts)` | `Device::open_mock_shared()` | `Device.open_mock(shared=True)` | `OpenMockShared(…)` |
| replies that belonged to somebody else | `device.stolen_frames()` | `stolen_frames()` | `.stolen_frames` | `StolenFrames()` |
| info | `device.info()` | `info_json()` | `.info` | `InfoJSON()` |
| status | `device.status()` | `status_json()` | `.status` | `StatusJSON()` |
| status ext | `device.status_ext()` | `status_ext_json()` | `.status_ext` | `StatusExtJSON()` |
| sessions | `device.sessions()` | `sessions_json()` | `.sessions` | `SessionsJSON()` |
| state | `device.state()` | `state()` | `.state` | `State()` |
| reconnect | `device.reconnect()` | `reconnect()` | `.reconnect()` | `Reconnect()` |
| release all | `device.release_all()` | `release_all()` | `.release_all()` | `ReleaseAll()` |
| barrier | `device.barrier()` | `barrier()` | `.barrier()` | `Barrier()` |
| bootloader | `device.bootloader()` | `bootloader()` | `.bootloader()` | `Bootloader()` |
| events | `set_event_handler(Arc<dyn EventHandler>)` | `set_event_handler(fn)` | `.set_event_handler(fn)` | `SetEventHandler(fn)` |
| log level | `log::set_level(Level)` | `set_log_level(int)` | `set_log_level(int)` | `SetLogLevel(uint8)` |
| log sink | `log::set_sink(fn)` | `set_log_sink(fn)` | `set_log_sink(fn)` | `SetLogSink(fn)` |
| relative move | `move_by(dx, dy, opts)` | `move_by(dx, dy, ms)` | `.move_by(dx, dy, ms)` | `MoveBy(dx, dy, ms, p)` |
| raw relative move (no barrier) | `move_raw(dx, dy)` | `move_raw(dx, dy)` | `.move_raw(dx, dy)` | `MoveRaw(dx, dy)` |
| raw command passthrough | `command(&Command)` | `command(opcode, payload)` | `.command(opcode, payload)` | `Command(opcode, payload)` |
| absolute move | `move_to(x, y, opts)` | `move_to(x, y, ms)` | `.move_to(x, y, ms)` | `MoveTo(x, y, ms, p)` |
| path | `move_path(&[(x,y)], opts)` | `move_path({{{…}}}, ms)` | `.move_path([(x,y)], ms)` | `MovePath([][2]int32, ms, p)` |
| async move | `start_move_to/by/path` | `start_move_to/by/path` | `.start_move_to/by/path` | `StartMoveTo/By` |
| button | `button(Button, down)` | `button(Button, down)` | `.button(btn, down)` | `Button(btn, down)` |
| click | `click(Button, count)` | `click(Button, n)` | `.click(btn, n)` | `Click(btn, n)` |
| double click | `double_click(Button)` | `double_click(Button)` | `.double_click(btn)` | `DoubleClick(btn)` |
| wheel | `wheel(v, h)` | `wheel(v, h)` | `.wheel(v, h)` | `Wheel(v, h)` |
| drag | `drag(Button, x, y, opts)` | `drag(Button, x, y, ms)` | `.drag(btn, x, y, ms)` | `Drag(btn, x, y, ms, p)` |
| key down/up | `key_down/up(page, usage)` | same | same | `KeyDown/KeyUp` |
| press / long press | `press`, `long_press(page, usage, ms)` | same | same | `Press`, `LongPress` |
| chord | `chord(&[usage], hold_ms)` | `chord({…}, hold_ms)` | `.chord([usage], hold_ms)` | `Chord([]uint16, ms)` |
| sequence | `sequence(&[KeyStep])` | `sequence({…})` | `.sequence([KeyStep])` | `Sequence([]KeyStep)` |
| text | `type_text(&TypeTextConfig)` | `type_text(s, interval, hold, jitter)` | `.type_text(s, interval, hold, jitter)` | `TypeText(s, interval, hold, jitter)` |
| scheduler | `device.scheduler()` | `device.scheduler()` | `.scheduler()` | `Scheduler()` |
| submit / every | `submit(delay, Priority, f)` | `submit(ms, prio, f)` | `.submit(ms, prio, f)` | `Submit(ms, prio, f)` |
| cancel / pause / resume | `cancel(id)`, `pause()`, `resume()` | same | same | same |
| wait idle | `wait_idle(dur)` | `wait_idle(ms)` | `.wait_idle(ms)` | `WaitIdle(ms)` |
| take errors | `take_errors()` | `take_errors_json()` | `.take_errors()` | `TakeErrors()` |
| counters | `device.metrics()` | — | — | — |
| record / replay | `replay::Recorder`, `Recording` | — | — | — |
| attach to a broker | `broker::Client::connect(name)` | `Remote::attach(name)` | `Remote.attach(name)` | `Attach(name)` |
| start a broker if none is listening | `Client::connect_or_spawn(name, exe)` | `Remote::spawn(name)` | `Remote.spawn(name)` | `Spawn(name)` |
| presence, no session | `probe::read_selector(sel)` | `probe(sel)` | `probe(sel)` | `Probe(sel)` |
| presence for the whole bus | `probe::all()` | `probe_all()` | `probe_all()` | `ProbeAll()` |
| presence on a timer | `Observer::start` | `Observer::start` | `Observer(...)` | `Observe(sel, ms)` |
| monitor or window as the move target | `MotionOptions::screen` | — | — | — |

## Device

```rust
pub struct DeviceOptions { pub auto_reconnect: bool, pub timeout_ms: u32, pub heartbeat: bool }
// default: true, 1000, true

DeviceInfo { serial, product, board, firmware, lease_ms, poll_ms,
             capabilities, device_id: [u8; 8], build: u32 }
```

`DeviceEntry` (device list): `{ path, serial, product, manufacturer, vid, pid }`.

### Sharing a board

```rust
pub enum Ownership { Exclusive, Shared }   // default: Exclusive

impl Device {
    pub fn open_shared(selector: Option<&str>, options: DeviceOptions) -> Result<Device>;
    pub fn open_mock_shared(options: DeviceOptions) -> Result<(Device, MockHandle)>;
    pub fn over_transport(transport: Box<dyn Transport>, target: Target,
                          options: DeviceOptions, ownership: Ownership) -> Result<Device>;
    pub fn ownership(&self) -> Ownership;
    pub fn set_ownership(&self, ownership: Ownership);          // for commands not yet sent
    pub fn stolen_frames(&self) -> u64;
    pub fn set_stray_hook(&self, hook: Arc<dyn Fn(&[u8]) + Send + Sync>);
    pub fn clear_stray_hook(&self);
}
```

Ownership is not a field of `DeviceOptions`: it decides whether the SDK may
reopen the session at all, so it is fixed when the device is opened, and
`open_shared` is the constructor that says so.

The device answers on one IN queue and the protocol does not address a reply to a
handle, so two controllers occasionally take each other's frames. Nothing here
arbitrates that — no lock, no queue, no broker; staying out of each other's way
is the hosts' discipline. What sharing changes is what a collision costs:

| | `Exclusive` | `Shared` |
| --- | --- | --- |
| a reply that never arrives | `Timeout` | `Busy`, message names the command: `STATUS got no reply; another process may have taken it (no reply to opcode 6 within 120 ms)` |
| the SDK reopens the session on its own | yes, for a fault it believes it can fix | never — `reconnect()` is the host's call |
| heartbeat whose reply was taken | reconnects, as above | retried on the next tick, no CLOSE |
| a frame for another session | dropped | counted in `stolen_frames`, offered to the stray hook |
| sequence numbers, lease, counters | unchanged | unchanged |

`stolen_frames` counts every frame the reply loop looked past because it answered
another session or another sequence — a subset of `retries`, which also counts
reads that returned nothing. `gdb_stolen_frames(dev)`, `device.stolen_frames` and
`Device.StolenFrames()` are the same number in the other layers.

The stray hook runs on the thread that is reading the queue, with that device's
session busy, so it must not call back into the same `Device`. It is the seam a
cooperating host uses to hand a frame on; the default, no hook, drops the frame as
before. Only a foreign session reaches it — a late reply to this session's own
earlier command moves the counter and nothing else.

`over_transport` is the same constructor for a transport the caller owns — a
proxy, a capture, a test fake — and takes the `Ownership` explicitly, which is how
`sdk/rust/tests/shared.rs` puts two devices on one board.

### Raw command passthrough

`device.command(&Command) -> Result<Option<Packet>>` sends one protocol command
exactly as given. It uses the same session, sequence counter, timeout and
reconnect path as `status()` or `barrier()`, but none of the SDK's own
bookkeeping: held buttons are not recorded and a mock device's virtual cursor is
not advanced. A `Command::MouseMove` sent this way therefore leaves the mock
cursor where it was, and `device`'s opinion of what is held no longer matches
what was actually sent. Control opcodes (OPEN, CLOSE, STATUS, BARRIER,
RELEASE_ALL, STATUS_EXT, SESSION_LIST, BOOTLOADER) return `Some(Packet)`; input
opcodes return `None` without waiting for a reply. `Command` and `Packet` are
`input_protocol::v3` types, so the caller needs `input-protocol` as a dependency.

Reach for it when the caller already speaks the wire protocol and needs one
opcode that the SDK does not wrap — a debug pane, a packet log, or a host porting
an existing motion loop. Prefer the typed methods for anything the SDK covers;
they keep the held state and the mock honest.

`gdb.h` carries it as `gdb_command(device, opcode, payload, len, gdb_packet *out)`,
with an opcode and a payload instead of a `Command` value; the packet's payload
is released with `gdb_packet_free`, and an opcode the protocol does not define
fails with `Invalid` before anything is sent:

| | Signature | When no reply is expected |
| --- | --- | --- |
| Rust | `Device::command(&Command) -> Result<Option<Packet>>` | `None` |
| C ABI | `gdb_command(device, opcode, payload, len, gdb_packet *out)` | `*out` zeroed, `opcode == 0` |
| C++ | `Device::command(opcode, payload)` → `std::optional<Packet>` | `std::nullopt` |
| Python | `Device.command(opcode, payload=b"")` → `Packet \| None` | `None` |
| Go | `Device.Command(opcode, payload) (*Packet, error)` | `nil, nil` |

Every row keeps the same session, sequence counter, timeout and reconnect path as
`status()`, and none of them touches the SDK's bookkeeping. The reply is
inferred from the opcode: OPEN, CLOSE, STATUS, BARRIER, RELEASE_ALL, STATUS_EXT,
SESSION_LIST and BOOTLOADER answer, every input opcode is fire and forget.

JSON returned by the bindings:

```json
{"serial":"MOCK","product":"GDBOX mock","board":2,"firmware":"3.2.0",
 "leaseMs":2000,"pollMs":1,"capabilities":15,"deviceId":[66,80,51,48,52,66,56,51],"build":229638145}
```

```json
{"received":143,"completed":143,"pending":0,"leaseMs":2000,"rx":0,"tx":0,
 "hid":0,"failsafe":0,
 "state":{"modifiers":0,"keys":[…28 bytes…],"consumer":0,"buttons":0}}
```

```json
{"uptimeMs":1000,"lateMaxUs":0,"outDepth":0,"rxDepth":0,"txDepth":0,"sessions":1,
 "lastFault":0,"lastFaultAgeMs":null,"resetReason":3,"bootCount":1,"build":229638145}
```

`SESSIONS` → array of `{ "id": 1234, "leaseMs": 2000, "held": 0, "flags": 0 }`.

## Motion

```rust
pub struct MotionOptions {
    pub duration_ms: u32,
    pub profile: HumanProfile,
    pub screen: platform::Screen,      // Primary by default
}
// duration_ms = 0 → natural duration from the distance

impl MotionOptions {
    pub fn linear(duration_ms: u32) -> Self;             // linear profile
    pub fn with_screen(self, screen: platform::Screen) -> Self;
}

pub struct HumanProfile {
  curve: f64, jitter_px: f64, overshoot_pct: f64, settle_ms: u32,
  ease: Ease, min_step_px: i32,
}
pub enum Ease { Jerk, EaseInOut, Linear, EaseIn, EaseOut,
                CubicBezier { x1, y1, x2, y2 } }
```

Relative moves ignore `screen`; only `move_to` and `start_move_to` read it.

C ABI `gdb_profile { ease, bezier[4], curve, jitter_px, overshoot_pct,
settle_ms, min_step_px }`, ease codes `0 jerk, 1 in-out, 2 linear, 3 in,
4 out, 5 cubic-bezier`. A NULL profile means the SDK default.

`Movement`: `cancel()`, `retarget(x, y)`, `is_finished()`, `wait_timeout(Duration)`
(returns `Timeout` and leaves the movement running) and `wait()` (consumes it).
Dropping a movement cancels it. In the C ABI the same handle answers
`gdb_movement_cancel`, `gdb_movement_retarget`, `gdb_movement_is_finished`,
`gdb_movement_wait(timeout_ms)` and `gdb_movement_free`, which cancels.

### Screen targets

```rust
pub enum platform::Screen {
    Primary,           // the default
    Monitor(usize),    // index in EnumDisplayMonitors order
    Window(isize),     // HWND as an integer
}
```

The target is the rectangle an absolute move is clamped into. Coordinates are
physical pixels in virtual-desktop space — `GetMonitorInfoW`'s space, primary
top-left at `(0, 0)` — and the process is per-monitor DPI aware before
`GetPhysicalCursorPos` is read, so nothing is scaled. `Primary` follows the
primary flag, which need not be index 0; `Monitor(i)` is enumeration order, not
the display number Windows settings shows; `Window(hwnd)` is `GetWindowRect`'s
rectangle. The caller's point is clamped into the rectangle, so a coarse target
lands on the edge; the only refusal is a point no monitor covers (a gap between
monitors, or past the last edge), which returns `Unsupported` and names the target
and the desktop's union. An index past the end, a window that does not exist and a
window with an empty rectangle fail the same way before the pointer moves. The
mock reports one 1920×1080 screen, where `Primary` and `Monitor(0)` work; a
non-Windows host has no cursor API, so absolute moves are `Unsupported` there.

The C ABI takes no target: `gdb_mouse_move_to` and `gdb_mouse_drag` use the default
options, so a binding over `gdb.dll` always moves inside the primary monitor.

## Keyboard

`page`/`usage` are HID usage ids: page 7 for the keyboard, page 12 for consumer
keys. Usages outside the firmware catalog fail with `Invalid` before anything is
sent.

`TypeTextConfig { text, interval_ms, hold_ms, jitter_ms }` (default 45/18/30,
jitter is uniform per character). Newlines (`\n`) and tabs map to Enter/Tab;
unsupported characters are dropped, so the stroke count is not the character
count.

`KeyStep { page, usage, hold_ms, gap_ms }`.

## Scheduler

```rust
let scheduler = device.scheduler();
let id = scheduler.submit(Duration::from_millis(250), Priority(5), |device| {
    device.mouse().click(Button::Left, 1)
})?;
scheduler.pause();
scheduler.resume();
scheduler.cancel(id);
scheduler.wait_idle(Duration::from_secs(5))?;
let errors = scheduler.take_errors();
```

Callbacks receive the device and return `Result<()>`. Failures are collected
(32 max) instead of panicking the worker. A repeating task is `every(interval,
priority, f)`; cancel it to stop. `wait_idle` returns `Timeout` if repeating
tasks keep the queue busy.

## Broker

One process owns the device; the others drive it over a named pipe. This is the
answer to the single-reader rule, not a second protocol: the broker process holds
the `Device`, every op runs on it, and clients see typed results.

```rust
pub const DEFAULT_NAME: &str = "default";
pub fn pipe_path(name: &str) -> String;                 // \\.\pipe\gdbox-<name>
pub fn run_if_requested() -> bool;                      // main() hook for --gdbox-broker

pub enum Source { Auto, Selector(String), Device(Device) }
pub struct Server;                                      // Server::new(name, source)
    // name(), state(), clients(), wait(), shutdown()  — and Drop shuts down

pub struct Client;                                      // Client::connect(name)
    // connect_timeout(name, wait), available(name), connect_or_spawn(name, exe),
    // set_timeout(Duration), events(), is_closed(), close()  — and Drop closes

pub enum Event { State(ConnectionState), Error(Error), Closed }
```

`Server::new` creates the pipe before it returns, so a `Client::connect`
immediately afterwards succeeds. `Source::Auto` and `Source::Selector` retry every
500 ms until a device appears; `Source::Device` takes a handle the caller already
owns. `run_if_requested` reads `--gdbox-broker [name]` / `--gdbox-broker=<name>`,
serves that name with `Source::Auto` and exits the process; with no such argument
it returns `false` and does nothing. `connect_or_spawn` spawns
`exe --gdbox-broker <name>` detached (`DETACHED_PROCESS`) and retries the pipe for
three seconds. A call waits 30 s for its reply unless `set_timeout` says otherwise;
`connect` waits 250 ms for a busy pipe.

Client methods: `info`, `status`, `status_ext`, `sessions`, `key(page, usage, down)`,
`mouse_move(dx, dy)`, `move_by(dx, dy, options)`, `move_raw(dx, dy)`,
`mouse_buttons(mask)`, `wheel(vertical, horizontal)`, `barrier`, `release_all`,
`command(&Command) -> Result<Option<Packet>>`. There is no `move_to`, no
`start_move_*`, no `drag`, no keyboard text or chord helper and no metrics op.

Wire format: one JSON object per line, UTF-8, `\n`-terminated, `id` echoed in the
reply, events without `id`, `code` = numeric `ErrorCode`.

| op | args | result |
| --- | --- | --- |
| `info` | `{}` | `{serial, product, board, firmware, leaseMs, pollMs, capabilities, deviceId, build}` |
| `status` | `{}` | `{received, completed, pending, leaseMs, rx, tx, hid, failsafe, state}` |
| `status_ext` | `{}` | `StatusExt` in camelCase |
| `sessions` | `{}` | `[{id, leaseMs, held, flags}, …]` |
| `key` | `{page?, usage, down?}` | `null` |
| `mouse_move` | `{dx, dy}` (i16) | `null` |
| `move_by` | `{dx, dy, durationMs?, profile?}` | `null` |
| `move_raw` | `{dx, dy}` (i32) | `null` |
| `mouse_buttons` | `{buttons}` | `null` |
| `wheel` | `{vertical, horizontal}` | `null` |
| `barrier` | `{}` | `null` |
| `release_all` | `{}` | `null` |
| `command` | `{command: {kind, …}}` | `{packet: {opcode, flags, session, sequence, payload} \| null}` |
| `close` | `{}` | `null`, then the server hangs up |

`key` defaults `page` to 7 and `down` to true. `command.kind` is one of `open`,
`close`, `heartbeat`, `status`, `barrier`, `release_all`, `status_ext`,
`session_list`, `bootloader`, `key {page, usage, down}`, `snapshot {state}`,
`mouse_move {dx, dy}`, `mouse_buttons {buttons}`, `wheel {vertical, horizontal}`.
Unparseable input ends that connection only; an unknown op is answered with
`code: 8`; every connection is greeted with the current state event first.

Fairness: one command at a time behind the device lock, so a command is atomic and
a sequence of them is not — `barrier` is how a client draws the line. Missing by
design: raw HID reports, the `GET_FEATURE` capabilities scan, the host-side motion
worker, flashing (BOOTLOADER is a `command`). Windows named pipes only; elsewhere
`Server::new` and `Client::connect` return `Unsupported`.

### Attached clients outside Rust

`gdb.h` carries a client under a second handle type, so C++/Python/Go/C#/JNI
programs can attach without reimplementing the pipe:

```c
gdb_remote *remote = NULL;
if (gdb_attach("default", &remote) != GDB_OK) { /* nothing listening */ }
char *status = gdb_remote_status(remote);   /* the gdb_status object */
gdb_remote_move_raw(remote, 400, -120);
gdb_remote_barrier(remote);
gdb_remote_free(remote);
```

`gdb_attach` takes the name (`NULL`/`""` for `default`), fills a `gdb_remote` and
reports `Disconnected` when nothing serves that pipe; `gdb_remote_spawn` starts a
broker first, from `GDBOX_BROKER_EXE`, and reports `Unsupported` when that
variable names nothing. The rest is `gdb_remote_info`, `_status`, `_status_ext`,
`_sessions`, `_key`, `_mouse_move`, `_move_by`, `_move_raw`, `_mouse_buttons`,
`_wheel`, `_barrier`, `_release_all`, `_command` and `_close`. Results reuse the
direct shapes, so a parser written for `gdb_device` reads an attached reply
unchanged. What an attached handle has not: `move_to`, `start_move_*`, `drag`,
`type_text`, `keys_sequence`/`keys_chord`, the scheduler, events and metrics —
those need the collection or the process that owns it. `gdb::Remote`,
`gdb.Remote` and `gdb.Attach` are the bindings' spelling of these calls.

## Presence

```rust
pub struct Probe {                    // Serialize: camelCase, DeviceInfo's keys
    pub serial: String, pub product: String, pub board: u8, pub firmware: String,
    pub lease_ms: u16, pub poll_ms: u8, pub capabilities: u32,
    pub device_id: [u8; 8], pub build: u32,
}

pub fn probe::read(path: &str) -> Result<Probe>;
pub fn probe::read_selector(selector: Option<&str>) -> Result<Probe>;
pub fn probe::read_from(transport: Box<dyn Transport>, serial: &str, product: &str) -> Result<Probe>;
pub fn probe::all() -> Result<Vec<Probe>>;
// Manager::probe() and Manager::probe_all() are the same two calls without the path.

pub enum ProbeEvent { Present(Probe), Gone(Error) }

impl Observer {
    pub fn open(selector: Option<&str>) -> Result<Self>;          // one read, no thread
    pub fn start(interval: Duration,
                 callback: impl FnMut(&ProbeEvent) + Send + 'static) -> Result<Self>;
    pub fn start_selector(selector: Option<&str>, interval: Duration, callback: …) -> Result<Self>;
    pub fn start_with(source: impl Fn() -> Result<Probe> + Send + 'static,
                      interval: Duration, callback: …) -> Result<Self>;
    pub fn stop(&mut self);                                       // and Drop stops
    pub fn is_running(&self) -> bool;
    pub fn probe(&self) -> Probe;                                 // the last one, kept on failure
    pub fn info(&self) -> DeviceInfo;                             // the Device::info() shape
    pub fn state(&self) -> ConnectionState;
    pub fn last_error(&self) -> Option<Error>;
    // status, status_ext, sessions, barrier, release_all, bootloader, command, reconnect:
    // every one is Err(Unsupported) with probe::OBSERVER_REFUSAL as the message.
}
```

The identity a session needs — board id, firmware, lease, poll interval, capability
bits, device id, build — arrives in `GET_FEATURE(14)`, the report `Session::open`
reads before OPEN. A feature report is a control transfer: it never touches the
interrupt IN queue, so a second process can read it at any time. Nothing else in
the protocol is like that. A command consumes the shared IN queue, and the reader
that drains a reply the owner was waiting for costs that owner one error — `Busy`
if it opened shared, `Timeout` if it did not; OPEN takes one of the eight owner
slots. This is why `probe` opens no session and writes no packet —
`sdk/rust/tests/probe.rs` asserts the transport's write count is zero — and why an
`Observer` has no command that can succeed instead of a command that happens to be
harmless today. `probe` stays the answer for a program that only wants to read the
board's numbers; [`Ownership::Shared`](#sharing-a-board) is for one that also
wants to send.

`probe::all` skips a board that vanishes mid-listing and returns `Ok(vec![])` on
an empty bus; a bus that cannot be enumerated is an `Err`. A selector that matches
nothing is `Disconnected`. The observer resolves its selector again on every poll,
so a renumbered HID path after a re-plug is not fatal, and it reports `Present`
once when the identity is new, `Gone` once when polls start failing, and nothing
while the answer is unchanged. `start_with` is the seam the tests use and the way
a host that owns its own HID backend reads Feature 14 through it.

What a host must still not do, even with this module: open an exclusive `Device`
while another program holds the session, read the IN endpoint itself, or send
`BOOTLOADER`.

## Watch

```rust
pub const DEFAULT_INTERVAL: Duration;            // 500 ms
pub enum WatchEvent { Arrived(DeviceEntry), Removed(DeviceEntry), BootloaderArrived(PathBuf) }
pub struct Listing { pub devices: Vec<DeviceEntry>, pub volumes: Vec<PathBuf> }
pub trait Inventory: Send + 'static { fn listing(&self) -> Result<Listing>; }
pub struct SystemInventory;                      // Manager::list + flash::drives

impl Watch {
    pub fn start(interval: Duration,
                 callback: impl Fn(&[DeviceEntry], WatchEvent) + Send + 'static) -> Result<Self>;
    pub fn start_with(inventory: Box<dyn Inventory>, interval: Duration,
                      callback: impl Fn(&[DeviceEntry], WatchEvent) + Send + 'static) -> Result<Self>;
    pub fn stop(&mut self);
    pub fn is_running(&self) -> bool;             // and Drop stops
}
```

The callback gets the whole current listing plus the one event and runs on the
watch thread. The first poll reports everything already present as an arrival;
an unchanged listing emits nothing; a bootloader volume that stays mounted is
reported once and its removal is not an event. Devices are keyed by serial, or by
HID path when the serial is empty. An interval below 10 ms is rounded up; a failed
enumeration keeps the previous listing and logs once. `stop` wakes the sleep, joins
and is safe twice.

## Metrics

```rust
pub struct MetricsSnapshot {
    pub frames_sent: u64,       // output reports, handshake and closing CLOSE included
    pub frames_received: u64,   // accepted input reports for this session
    pub retries: u64,           // reply-loop read retries (empty read, foreign frame)
    pub stolen_frames: u64,     // the subset of retries for another session or sequence
    pub reconnects: u64,        // transports reopened after a fault or expired lease
    pub lease_faults: u64,      // commands answered with a lease fault
    pub timeouts: u64,          // commands whose reply never arrived
    pub latency: LatencyStats,  // count, min_us, p50_us, p95_us, max_us
}

impl Device {
    pub fn metrics(&self) -> MetricsSnapshot;
    pub fn reset_metrics(&self);
}
```

`retries` counts reads the reply loop had to look past — a read that returned
nothing while the deadline was still running, or a frame for another session or
sequence. The SDK never re-sends a command, so it is not a retransmission count.
Counters are per device, not per session, so `reconnects` and `lease_faults` keep
adding up across one.

`stolen_frames` is the second half of that: `retries` minus the reads that came
back empty. On a board several programs read, it is how a collision is told from a
board that is merely slow — `stolen_frames` moves only when a frame that belonged
to somebody else arrived, and a shared session reports `Busy` for exactly the
command whose reply never came back. `device.stolen_frames()` reads the one atomic
without walking the histogram.

Recording is one relaxed atomic add per counter and per latency sample: no lock,
no allocation, no formatting, which is why it can sit on the report path. A sample
is taken only for a command that waited for a reply; input opcodes count as frames
and timeouts count as timeouts, neither adds a sample. The histogram is 4176
buckets — one per microsecond below 4096 µs, then eight per doubling up to 4 MiB,
clamped at the top. Below 4 ms `min_us`/`max_us` are exact; above, they are the
bucket's edges and percentiles are its midpoint, within 6%. `snapshot()` reads
atomics and walks the buckets without taking the session lock, so a UI may poll it
freely, though two fields may be one report apart. `reset_metrics` zeroes
everything with relaxed stores: a report racing the reset may be lost, and nothing
blocks the sender.

## Replay

```rust
pub const VERSION: u32 = 1;

pub enum Op {
    Key { page: u16, usage: u16, down: bool },
    Buttons { bits: u8 },
    Wheel { dx: i16, dy: i16 },        // dx horizontal, dy vertical
    MoveBy { dx: i32, dy: i32 },       // interpolated, barrier at the end
    MoveRaw { dx: i32, dy: i32 },      // one report, no barrier
    Barrier, ReleaseAll, Wait,
}

pub struct Entry { pub at_ms: u64, pub op: Op }
pub struct Recording;                  // new(seed, (w, h)), push, save, load, play, len, entries()
pub struct Recorder;                   // new(seed, (w, h)), record, record_at, save, load, play, clear
pub struct Outcome { pub played: usize, pub skipped: usize, pub elapsed_ms: u64 }
pub struct NoCancel;                   // a Cancel that never fires
```

A file is one JSON object; every field:

| Field | Type | Meaning |
| --- | --- | --- |
| `version` | number | format version; `1` is the only one `load` accepts, anything else (or a missing field) is `Invalid` |
| `seed` | number | the u64 PRNG seed the gesture was captured with |
| `screen` | `[width, height]` | the screen the session was recorded on, pixels |
| `started_at_ms` | number | Unix milliseconds when recording started |
| `entries` | array | capture order, one object per call |
| `entries[].at_ms` | number | milliseconds since `started_at_ms` |
| `entries[].op` | string | `key`, `buttons`, `wheel`, `move_by`, `move_raw`, `barrier`, `release_all`, `wait` |
| `key` fields | `page`, `usage`, `down` | HID page/usage and direction |
| `buttons` fields | `bits` | whole button mask, not a delta |
| `wheel` fields | `dx`, `dy` | horizontal, vertical |
| `move_by` / `move_raw` fields | `dx`, `dy` | relative pixels |

Keys are snake_case and appear in that order. `save` writes pretty JSON with a
trailing newline; two saves of the same recording are byte for byte identical.
Recording stores intent, not bytes, so a recording outlives a report-layout change,
and `play` rebuilds the frames through `Device` at run time.

`play(&mut Device, speed: f64, cancel: &dyn Cancel) -> Result<Outcome>`. `speed` is
`0.001..=1000` and divides every `at_ms` gap. Playback is deterministic: entries
apply in order from a fixed timeline with no random source, and `Outcome::is_complete`
is `skipped == 0`. Cancellation is checked between entries and inside each wait; a
cancelled playback sends RELEASE_ALL when at least one entry had played and returns
`Ok` with the rest as `skipped`, while a device failure is an `Err`.

## Errors

| Code | Name | Meaning |
| --- | --- | --- |
| 0 | Ok | — |
| 1 | Invalid | bad argument, unknown usage, wrong session id |
| 2 | Timeout | no reply / movement did not settle |
| 3 | Disconnected | device gone, no match for the selector |
| 4 | Busy | eight owners, the session id is already open, or — on a shared device — a reply another process took |
| 5 | Lease | the 2 s lease expired |
| 6 | Framing | malformed frame or response |
| 7 | Overflow | device queue overflow / stale output |
| 8 | Unsupported | platform or firmware lacks the feature |
| 9 | Cancelled | the operation was cancelled |
| 10 | Io | HID read/write failed, or a panic at the ABI boundary |

C ABI errors live in `gdb_last_error(buffer, capacity)`; the value returned by a
failing call equals the code. Bindings convert this into their own exception
type: `gdb::Error`, `gdb.GdbError`, `*gdb.Error`.

## C ABI

`include/gdb.h` is the contract: `gdb_abi_version() == 1` with
`GDB_ABI_VERSION_MINOR == 7`, and `gdb_version()` is the SDK version string.
Minor 1 added the attached handle, minor 2 the presence probe and the observer,
minor 3 `gdb_open_shared`, `gdb_open_mock_shared` and `gdb_stolen_frames`, 4
`gdb_scheduler_submit_at` and `gdb_scheduler_elapsed_ms`, 5 the metrics snapshot,
6 the screen-targeted absolute moves and 7 the arrival watch; no existing symbol
changed shape. Handles are opaque (`gdb_device`, `gdb_remote`, `gdb_observer`,
`gdb_watch`, `gdb_movement`, `gdb_scheduler`); strings returned by the library
must be freed with `gdb_string_free`, and a `gdb_packet` from `gdb_command` with
`gdb_packet_free`. Callbacks:

```c
void (*gdb_event_fn)(void *user, int32_t kind, int32_t code, const char *msg);
int32_t (*gdb_task_fn)(void *user);   // 0 = ok, otherwise an ErrorCode
void (*gdb_log_fn)(void *user, int32_t level, const char *msg);
```

Event kinds: `0` state (code = `ConnectionState`), `2` error (code = ErrorCode).

The header is the compatibility layer for a language with no implementation of
its own, and it is deliberately narrower than the SDK. A direct handle
(`gdb_device`) has the session, the typed input calls, the queries, errors,
logging, `gdb_command` and `gdb_move_raw`; an attached handle (`gdb_remote`,
minor version 1) has `gdb_attach`, `gdb_remote_spawn` and the broker's op set
listed under [Broker](#broker); a probe (`gdb_probe`, `gdb_probe_all`) and an
observer handle (`gdb_observer`, minor version 2) have the identity with no
session at all; a shared device is the same `gdb_device` handle from
`gdb_open_shared` (minor version 3), with `gdb_stolen_frames` for its counter.
An observer's commands are all refusals — `gdb_observer_status`, `gdb_observer_barrier`,
`gdb_observer_release_all` and `gdb_observer_command` exist only to return
`GDB_UNSUPPORTED` with the reason. What the header has no symbol for: the flashing
sequence, the broker's server side, the arrival watch, the rest of the counters,
the stray hook, recording, a screen target — so a binding over `gdb.dll` has none
of them either. Those are Rust-only. `gdb_stolen_frames` is the one counter that
crossed, because a host sharing a board has to be able to tell a collision from a
slow board.

## Logging

Logging is process-wide and silent until both a level and a sink are set:

| | Level | Sink |
| --- | --- | --- |
| Rust | `log::set_level(Level)` | `log::set_sink(Some(Box::new(f)))` |
| C ABI | `gdb_set_log_level(uint8_t)` | `gdb_set_log_sink(gdb_log_fn, void *)` |
| C++ | `set_log_level(int32_t)` | `set_log_sink(fn)` |
| Python | `set_log_level(int)` | `set_log_sink(fn)` |
| Go | `SetLogLevel(uint8_t)` | `SetLogSink(fn)` |

Levels are `0 Off, 1 Error, 2 Warn, 3 Info, 4 Debug, 5 Trace`. The SDK writes at
`Info` (connect, close, reconnect), `Warn` (a command failed, with its code) and
`Trace`; nothing emits `Error` or `Debug` today.

`Trace` is the packet log. Frames are logged in `Session`, underneath the C ABI,
so every binding prints identical lines:

```
out opcode=6 session=1234 sequence=2 payload=[…]
in opcode=134 session=1234 sequence=2 payload=[…]
skip opcode=134 session=4321 sequence=7 (not ours)
```

`skip` is an IN frame that belonged to another session and was dropped while
waiting for a reply. A host that wants a packet window or a log pane subscribes
to the sink instead of opening a second reader on the IN endpoint — two readers
steal each other's replies and break the sequence numbers of both sessions.

## Flashing

`gdb::flash` builds the firmware, packs it and copies it to the board; the C ABI
does not expose any of it, so flashing is Rust only.

```rust
gdb::flash::Config              // cargo, firmware dir, auto_bootloader, boot_drive
gdb::flash::build(&config, &runner, &cancel, &mut report) -> Result<Image>
gdb::flash::push(&config, Some(&device), &image, &cancel, &mut report) -> Result<Handshake>
gdb::flash::Host                 // volumes() + devices() + verify(): what push reads
gdb::flash::SystemHost           // the real one: drives(), Manager::list(), open and check
gdb::flash::Waits { drive, handshake }   // 15 s each; push runs on the defaults
gdb::flash::uf2::pack(&elf) -> Result<Vec<u8>>          // the packer on its own
gdb::flash::write_image(path, bytes, &cancel) -> Result<()>
gdb::flash::drives() -> Result<Vec<PathBuf>>            // mounted RPI-RP2 volumes
```

`Image { bytes, blocks }`. `build` runs cargo for `thumbv6m-none-eabi` with the
`hardware` feature and packs the ELF; `push` sends BOOTLOADER (opcode 9), closes
the runtime session, waits for a volume that was not mounted before (15 s),
copies in 16 KiB chunks and then waits (15 s) for the runtime HID to re-enumerate
so it can read Feature 14.

`Handshake` is `Verified(Verified { board, firmware, lease_ms })` or
`Unverified(Error)`: `Err` from `push` means the transfer failed, `Unverified`
means the bytes are on the flash but the board never answered.

`Host`, `Waits`, `Runner`, `Reporter` and `Cancel` are the seams: the host
supplies the volumes and devices it reads, the two waits, the process runner, the
progress sink and the cancel token, so the sequence needs no GUI and no drive to
be tested.

## Threading rules

* Lease: the device renews only on OPEN, HEARTBEAT, RELEASE_ALL, BOOTLOADER and
  valid input; STATUS, BARRIER, STATUS_EXT and SESSION_LIST do not renew. The SDK
  tracks the last renewing command itself and sends HEARTBEAT after 500 ms of
  silence, against a 2000 ms lease — so a STATUS, STATUS_EXT or SESSION_LIST poll
  is not a heartbeat.
* Heartbeat: every 500 ms while connected. The heartbeat thread takes the session
  lock without blocking and skips a tick while a command holds it.
* Async movement: one thread per movement; `retarget` is thread-safe.
* Scheduler: one worker per scheduler; callbacks never overlap.
* Event callbacks may run on the heartbeat, worker or caller thread; they must
  return quickly and must not call back into the same device synchronously.
* Broker: one command at a time behind the device lock, one reader thread per
  client, and a client's `EventReceiver` is `Clone` and readable from any thread.
* Watch: the callback runs on the watch thread, so a slow one delays the next
  poll; `stop` joins that thread.
