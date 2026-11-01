# GDBOX SDK

Host-side control and automation for the RP2040 GDBOX firmware. The firmware
only moves USB HID bytes; everything above that — device management, absolute
coordinates, motion profiles, scheduling — lives here.

```
sdk/
├── motion/          gdb-motion      pure motion math (integer-friendly, no I/O)
├── rust/            gdb             the implementation: device, mouse, keyboard,
│                                    scheduler, motion, broker, watch, metrics,
│                                    replay, log, flash
├── ffi/             gdb-ffi         C ABI (include/gdb.h) + cdylib
├── cpp/             gdb.hpp         header-only C++17 binding over the DLL
├── python/          gdb.py          ctypes binding over the DLL
└── go/              gdb.go          Windows binding over the DLL (no cgo)
```

## Layers

| Layer | Lives in | Speaks | Depends on |
| --- | --- | --- | --- |
| Implementation | `sdk/rust` | vendor HID protocol 3 | `shared/input-protocol`, `sdk/motion` |
| C ABI | `sdk/ffi` | the Rust SDK | `gdb` |
| Bindings | `sdk/cpp/gdb.hpp`, `sdk/python/gdb.py`, `sdk/go/gdb.go` | `gdb.dll` | the C ABI, loaded at runtime |

`gdb` (Rust) is the only implementation: one behaviour, one place to fix it.
`gdb-ffi` forwards that crate across `include/gdb.h`, and the three bindings are
name translation over the header — they have no logic of their own, so they
cannot drift, only lag. Every other language takes the same route: C#, JNI,
Node and Godot write the `gdb_*` calls of `include/gdb.h`, which is a C ABI and
nothing more.

## C ABI

`include/gdb.h` is the compatibility layer: it is how a language with no
implementation of its own reaches the SDK. `gdb_abi_version() == 1` pins the
contract, and `GDB_ABI_VERSION_MINOR` (7) is the revision that added the attached
handle, then the presence probe, the shared-ownership opens, the scheduler's
absolute due, the metrics snapshot, the screen-targeted absolute moves and the
arrival watch.

A **direct** handle (`gdb_device`) owns the vendor collection: the session, the
typed input calls, the queries, errors, logging, and the two escape hatches
`gdb_command` and `gdb_move_raw`.

An **attached** handle (`gdb_remote`) is a client of a broker process that owns
the collection: `gdb_attach` to `\\.\pipe\gdbox-<name>`, `gdb_remote_spawn` to
start a broker when none is listening, the same four queries, `key`,
`mouse_move`, `move_by`, `move_raw`, `mouse_buttons`, `wheel`, `barrier`,
`release_all`, `command` and `close`. Its JSON results are the direct calls'
results, so one parser serves both.

An **observer** handle (`gdb_observer`) has no session at all: it reads
Feature 14 on a timer and every command on it returns `GDB_UNSUPPORTED`. It is
what a program uses when another program owns the board.

| In `gdb.h` | Not in `gdb.h` |
| --- | --- |
| direct: `gdb_open`, `gdb_open_mock`, `gdb_open_shared`, `gdb_open_mock_shared`, `gdb_stolen_frames`, `gdb_close`, `gdb_reconnect`, `gdb_set_auto_reconnect`, `gdb_device_state`, `gdb_device_list`, `gdb_device_info`, `gdb_status`, `gdb_status_ext`, `gdb_sessions`, `gdb_release_all`, `gdb_barrier`, `gdb_bootloader`, `gdb_set_event_handler` | flashing: build, UF2 packing, BOOTSEL copy, verification |
| direct: `gdb_mouse_move_by/to/path`, `gdb_move_raw`, `gdb_mouse_start_move_*`, `gdb_movement_*`, `gdb_mouse_button`, `gdb_mouse_click`, `gdb_mouse_double_click`, `gdb_mouse_wheel`, `gdb_mouse_drag`, `gdb_key_*`, `gdb_keys_*`, `gdb_type_text`, `gdb_scheduler_*`, `gdb_profile_default` | the broker's server side: `broker::Server`, `Source`, `run_if_requested` |
| attached: `gdb_attach`, `gdb_remote_spawn`, `gdb_remote_free`, `gdb_remote_info/status/status_ext/sessions`, `gdb_remote_key/mouse_move/move_by/move_raw/mouse_buttons/wheel/barrier/release_all/command/close` | the arrival watch (`gdb::watch`) |
| presence: `gdb_probe`, `gdb_probe_all`, `gdb_observer_start/stop/free/info/state`, `gdb_observer_status/barrier/release_all/command` | the counters (`gdb::metrics`), recording and replay (`gdb::replay`), screen targets (`MotionOptions::screen`) |
| both: `gdb_command`, `gdb_packet_free`, `gdb_last_error`, `gdb_string_free`, `gdb_set_log_level`, `gdb_set_log_sink` | |

A binding over `gdb.dll` can therefore open a device itself or attach to the
process that owns it. What it cannot do: flash, run a broker, watch the bus,
meter a session, record or replay one, or pick a screen for an absolute move —
the `—` cells in the coverage table below. `gdb_command` fills a `gdb_packet`
(`opcode`, `flags`, `session`, `sequence`, `payload`) that the caller releases
with `gdb_packet_free`; an input opcode leaves it zeroed, which is how a binding
reports "no reply".

## Build

```bash
cargo build -p gdb-ffi --release      # target/release/gdb.dll (or libgdb.so)
cargo test  -p gdb -p gdb-motion      # 92 tests, mock device, no hardware
cargo run   -p gdb --example mock     # end-to-end walkthrough
```

The C ABI lives in `sdk/ffi/include/gdb.h`; the shared library is loaded at
runtime, so C++/Python/Go need no import library or build step of their own.
Set `GDBOX_LIBRARY` to the full path, or put `gdb.dll` next to the executable.

## Quick start

Rust (`sdk/rust`):

```rust
use gdb::{DeviceOptions, Manager, MotionOptions};

let (device, _mock) = Manager::open_mock_with_handle(DeviceOptions::default())?;
device.mouse().move_to(640, 360, MotionOptions::linear(250))?;
device.keyboard().type_text(&gdb::TypeTextConfig {
    text: "hello".into(), interval_ms: 45, hold_ms: 18, jitter_ms: 30,
})?;
```

C++ (`sdk/cpp`):

```cpp
#include "gdb.hpp"
auto device = gdb::Device::open_mock();
device.mouse().move_to(640, 360, 250);
device.keyboard().type_text("hello");
```

Python (`sdk/python`):

```python
import gdb
with gdb.Device.open_mock() as device:
    device.mouse.move_to(640, 360, duration_ms=250)
    device.keyboard.type_text("hello")
```

Go (`sdk/go`, Windows):

```go
device, err := gdb.OpenMock()
defer device.Close()
err = device.Mouse().MoveTo(640, 360, 250, nil)
err = device.Keyboard().TypeText("hello", 45, 18, 30)
```

## What is covered

Rows are capabilities, columns are layers. `—` means the layer has no symbol for
the capability at all; a named cell is that layer's call, and a binding's call
goes through the C ABI symbol in the cell beside it.

| Capability | Rust `gdb` | C ABI | C++ | Python | Go |
| --- | --- | --- | --- | --- | --- |
| Connect, OPEN/CLOSE, Feature 14 handshake | `Manager::open`, `close` | `gdb_open`, `gdb_close` | `Device::open`, `close()` | `Device.open`, `.close()` | `Open`, `Close()` |
| Mock device, virtual 1920×1080 cursor | `Manager::open_mock` | `gdb_open_mock` | `open_mock` | `open_mock` | `OpenMock` |
| Device list, open by serial or path | `Manager::list` | `gdb_device_list` | `list_json` | `list_devices` | `ListJSON` |
| Info: board, firmware, lease, poll, capabilities, device id, build | `info()` | `gdb_device_info` | `info_json` | `.info` | `InfoJSON` |
| Presence: that identity with no session and no packet | `probe::read`, `Observer` | `gdb_probe`, `gdb_probe_all`, `gdb_observer_*` | `probe`, `Observer` | `probe`, `Observer` | `Probe`, `Observe` |
| STATUS / STATUS_EXT / SESSION_LIST | `status`, `status_ext`, `sessions` | `gdb_status`, `gdb_status_ext`, `gdb_sessions` | `*_json()` | `.status`, `.status_ext`, `.sessions` | `*JSON()` |
| Relative move, split at ±1024 per axis | `move_by` | `gdb_mouse_move_by` | `move_by` | `.move_by` | `MoveBy` |
| Raw relative move, no barrier, no curve | `move_raw` | `gdb_move_raw` | `move_raw` | `.move_raw` | `MoveRaw` |
| Absolute move, host reads the cursor | `move_to` | `gdb_mouse_move_to` | `move_to` | `.move_to` | `MoveTo` |
| Multi-segment path | `move_path` | `gdb_mouse_move_path` | `move_path` | `.move_path` | `MovePath` |
| Background move, cancel / retarget / wait | `start_move_*`, `Movement` | `gdb_mouse_start_move_*`, `gdb_movement_*` | `start_move_*`, `Movement` | `.start_move_*`, `Movement` | `StartMoveTo/By` |
| Buttons, click, double click, drag, wheel, release-all | `button`, `click`, `drag`, `wheel` | `gdb_mouse_*` | `button`, `click`, `drag`, `wheel` | same | `Button`, `Click`, `Drag`, `Wheel` |
| Keys: down/up, press, long press, chord, sequence, text | `key_down`, `long_press`, `chord`, `type_text` | `gdb_key_*`, `gdb_keys_*`, `gdb_type_text` | same | same | `KeyDown`, `LongPress`, `Chord`, `TypeText` |
| Scheduler: submit, every, pause/resume/cancel, wait_idle, take_errors | `Scheduler` | `gdb_scheduler_*` | `Scheduler` | `.scheduler()` | `Scheduler()` |
| Events: state changes and errors | `set_event_handler` | `gdb_set_event_handler` | `set_event_handler` | `.set_event_handler` | `SetEventHandler` |
| Log level 0–5 | `log::set_level` | `gdb_set_log_level` | `set_log_level` | `set_log_level` | `SetLogLevel` |
| Log sink | `log::set_sink` | `gdb_set_log_sink` | `set_log_sink` | `set_log_sink` | `SetLogSink` |
| `Trace` frames, identical text in every layer | `Level::Trace` | as the SDK | as the SDK | as the SDK | as the SDK |
| Heartbeat, one reconnect per failed call | built in | `gdb_set_auto_reconnect` | `set_auto_reconnect` | `.set_auto_reconnect` | `SetAutoReconnect` |
| Second process on one device: attach a client | `broker::Client::connect` | `gdb_attach`, `gdb_remote_*` | `Remote::attach` | `Remote.attach` | `Attach` |
| Own the device for other processes: broker server | `broker::Server` | — | — | — | — |
| Arrival and removal watch, bootloader volume | `Watch::start` | — | — | — | — |
| Counters and round-trip latency | `metrics()` | — | — | — | — |
| Record and replay a session | `replay::Recorder`, `Recording` | — | — | — | — |
| Move anchored to a monitor or a window | `MotionOptions::screen` | — | — | — | — |
| Raw command passthrough | `Device::command` | `gdb_command` | `command` | `.command` | `Command` |
| Flash: cargo build, ELF → UF2, BOOTSEL copy, verify | `gdb::flash` | — | — | — | — |
| Unified error codes | `ErrorCode` (11) | `gdb_error` | `ErrorCode` | `ErrorCode` | `Code` |

Every row is implemented in Rust once. The broker's server side, the watch, the
counters, the replay and the screen target have no C symbol, so they stay in the
process that speaks Rust — flashing too; those are the `—` cells. Everything a
binding can do, it does by calling `gdb.h`, either on a direct handle it opened
itself or on an attached one.

Known gaps:

- `gdb.h` has no flashing, so no binding over `gdb.dll` can build, pack, flash
  or verify a board. Flashing is the Rust crate (`gdb::flash`).
- The broker's **server** is Rust-only (`Server::new`, `Source`,
  `run_if_requested`). A client attaches from any language, but the process that
  owns the device is a Rust program that embeds `gdb::broker`.
- `gdb::watch`, `gdb::metrics` and `gdb::replay` are Rust-only: the C ABI
  carries neither a meter, a recording nor a bus watch.
- The broker needs Windows named pipes. Elsewhere `Server::new`,
  `Client::connect` and `gdb_attach` return `Unsupported`.
- No absolute move through `gdb.h` chooses a target, so a binding over the DLL
  always moves inside the primary monitor's rectangle — and an attached handle
  has no absolute move at all, only `move_by` and `move_raw`.
- `sdk/go/gdb.go` has no `StartMovePath` (the C ABI, `sdk/cpp/gdb.hpp` and
  `sdk/python/gdb.py` do).
- Raw passthrough sends existing opcodes only; the firmware rejects unknown ones.
  `gdb_command` and the bindings take an opcode and payload, and an opcode the
  protocol does not define fails with `Invalid`.

## Presence without a session

A second program can see the board without disturbing the program that owns it.
The identity — board id, firmware, lease length, poll interval, capability bits,
device id, build — is `GET_FEATURE(14)`, and a feature report is a control
transfer: it never enters the interrupt IN queue, so nothing is taken from the
session that is running.

```rust
let board = gdb::probe::read_selector(None).unwrap();   // first attached board
println!("{} {}", board.board, board.firmware);         // no OPEN, no packet
let all = gdb::probe::all().unwrap();                   // every vendor collection

let mut observer = gdb::Observer::start(Duration::from_millis(500), |event| {
    // Present(Probe) on a new identity, Gone(Error) when polls start failing.
    let _ = event;
})
.unwrap();
observer.info();                                        // same shape as Device::info()
observer.status();                                      // Err(Unsupported)
```

`gdb_probe`, `gdb_probe_all` and the `gdb_observer_*` handle carry the same thing
into every binding. An observer refuses `status`, `barrier`, `release_all` and
`command` with `Unsupported`, because a command would put a frame on the shared
queue; that is also why `probe` writes nothing at all — a test asserts the
transport's write count stays zero. What a program must still not do while
another one holds the session: read the IN endpoint itself, or send `BOOTLOADER`.
A program that also wants to *send* commands opens the board shared
(`Device::open_shared`, `gdb_open_shared`, `shared=True`, `OpenShared`): the two
owners then take each other's replies occasionally, and the one that lost a reply
gets one `Busy` naming the command instead of a timeout it would answer with a new
session. A second program that needs the full surface without sharing is the
[broker](#broker)'s job.

## Broker

Only one process may read the vendor collection, so a second program that opens
the device steals the IN replies; the loser gets one error for the command whose
reply went missing and keeps its session (`Busy` if it opened shared). `gdb::broker` owns
the device in one process and lends it out over the named pipe
`\\.\pipe\gdbox-<name>` (`default` when the caller names nothing):

```rust
use gdb::broker::{Client, Server, Source};

let server = Server::new("default", Source::Auto)?;   // this process owns the device
// …in any other process:
let client = Client::connect("default")?;
let status = client.status()?;
```

`Source` is `Auto` (the first matching device, retried until one appears),
`Selector(name)` (that serial or HID path, retried) or `Device(device)` for a
handle the caller already owns, typically a mock in a test. `Server::shutdown`
stops accepting, ends every connection and releases the device; `wait` blocks
until then; `name`, `state` and `clients` are what a supervisor wants.

### Wire format

One JSON object per line, UTF-8, `\n`-terminated. A request carries an `id` that
the reply echoes; events carry no `id` and may arrive at any time. `code` is the
numeric `ErrorCode` the C ABI uses, so every language maps a failure the same way.

```
request  {"id":1,"op":"status","args":{}}
reply    {"id":1,"ok":true,"result":{…}}
reply    {"id":1,"ok":false,"error":{"code":7,"message":"device queue overflow"}}
event    {"event":"state","state":"connected"}
event    {"event":"error","code":3,"message":"device disconnected"}
```

| op | args | result |
| --- | --- | --- |
| `info`, `status`, `status_ext`, `sessions` | `{}` | device info, STATUS, STATUS_EXT, `[session, …]` |
| `key` | `{"page":7,"usage":4,"down":true}` | `null` |
| `mouse_move` | `{"dx":10,"dy":-4}` | `null` |
| `move_by` | `{"dx":10,"dy":-4,"durationMs":120}` | `null` |
| `move_raw` | `{"dx":800,"dy":-300}` | `null` |
| `mouse_buttons` | `{"buttons":1}` | `null` |
| `wheel` | `{"vertical":-120,"horizontal":0}` | `null` |
| `barrier`, `release_all` | `{}` | `null` |
| `command` | `{"command":{"kind":"wheel","vertical":1,…}}` | `{"packet": packet \| null}` |
| `close` | `{}` | `null`, then the server hangs up |

Device info is `serial, product, board, firmware, leaseMs, pollMs, capabilities,
deviceId, build`; the status block is `received, completed, pending, leaseMs, rx,
tx, hid, failsafe, state`, with `state = {modifiers, keys:[28], consumer, buttons}`.
A `command` is tagged by `kind` — `open`, `close`, `heartbeat`, `status`,
`barrier`, `release_all`, `status_ext`, `session_list`, `bootloader`,
`key {page,usage,down}`, `snapshot {state}`, `mouse_move {dx,dy}`,
`mouse_buttons {buttons}`, `wheel {vertical,horizontal}` — and `key` defaults
`page` to 7 and `down` to true, so `{"usage":4}` is a tap.

A line the server cannot parse ends that client's connection and nothing else; an
op it does not know is answered with `code: 8` and the connection stays up. Every
new connection is greeted with the current state event before any reply, so a late
joiner knows where the device is, and `Client::events()` hands back an
`EventReceiver` that yields `State`, `Error` and `Closed` from any thread.

### Fairness and limits

Every op runs to completion behind the device lock, so one command is atomic but a
sequence of them is not: another client's commands may land in between. "Hold key
A, move, release A" is the client's ordering problem — `Client::barrier()` is the
place to draw the line.

A client gets no raw HID reports and no `GET_FEATURE` capabilities scan, no
host-side motion worker at all: `start_move_to`, `move_path`, `drag`, `chord`,
`type_text` and absolute moves are not ops, because each needs the collection or
the process that owns it. `move_by` is paced inside the broker, so a client that
disconnects mid-move only loses the reply. Flashing (build, UF2 copy,
verification) is out of scope; BOOTLOADER itself is a `command`.

### Starting one

`run_if_requested()` is the first line of a program's `main`: when the command
line carries `--gdbox-broker [name]` (or `--gdbox-broker=<name>`) the process
becomes the broker on that name, serves until it is stopped and exits; otherwise
it returns `false` and touches nothing. `Client::connect_or_spawn(name, exe)`
connects, or starts `exe --gdbox-broker <name>` detached first and retries the
pipe for three seconds; `Client::available(name)` answers whether a broker is
listening. A call waits 30 s for its reply by default (`set_timeout` overrides),
and `connect` gives a busy pipe 250 ms (`connect_timeout` takes the wait).

Named pipes are Windows-only; elsewhere `Server::new` and `Client::connect`
return `Unsupported`.

### Reaching it from another language

The server is Rust, the clients need not be. `gdb_attach(name, &remote)` opens a
`gdb_remote` handle on `\\.\pipe\gdbox-<name>`, and `gdb_remote_info`,
`gdb_remote_status`, `gdb_remote_status_ext`, `gdb_remote_sessions`,
`gdb_remote_key`, `gdb_remote_mouse_move`, `gdb_remote_move_by`,
`gdb_remote_move_raw`, `gdb_remote_mouse_buttons`, `gdb_remote_wheel`,
`gdb_remote_barrier`, `gdb_remote_release_all` and `gdb_remote_command` are the
ops the broker serves. `gdb_remote_spawn(name)` starts a broker first, taking the
executable from `GDBOX_BROKER_EXE` — a host that calls `run_if_requested` and
otherwise does its own job; with the variable unset there is nothing to start and
it reports `Unsupported`. The bindings wrap the same calls as
`gdb::Remote::attach`, `gdb.Remote.attach` and `gdb.Attach`.

An attached handle has no absolute move, no drag, no typing and no chord: those
need the collection, which the broker holds. `gdb_remote_info` returns exactly
what `gdb_device_info` returns, so one parser serves both handles.

## Watch

`Watch::start(interval, callback)` polls the bus on one thread that the caller
does not have to own. `WatchEvent` is `Arrived(DeviceEntry)`,
`Removed(DeviceEntry)` or `BootloaderArrived(PathBuf)`; the callback receives the
whole current listing next to the single event, and it runs on the watch thread,
so a slow callback delays the next diff.

```rust
use gdb::watch::{Watch, WatchEvent};

let watch = Watch::start(gdb::watch::DEFAULT_INTERVAL, |devices, event| match event {
    WatchEvent::Arrived(entry) => { /* entry.serial */ }
    WatchEvent::Removed(entry) => {}
    WatchEvent::BootloaderArrived(root) => { /* an RPI-RP2 volume */ }
})?;
```

`DEFAULT_INTERVAL` is 500 ms. The inventory is `Manager::list` plus
`flash::drives`, and neither reads nor writes a report on the vendor collection,
so watching cannot steal a reply from the session that owns the board. The first
poll reports everything already present as an arrival, so a host that starts late
sees the whole bus instead of waiting for the next plug-in. A device is identified
by its serial, or by its HID path when the serial is empty.

`Inventory` is the seam: `Watch::start_with(Box<dyn Inventory>, interval,
callback)` scripts arrivals and removals without hardware, or plugs a host's own
enumeration in rather than opening a second handle. Polls shorter than 10 ms are
rounded up, so a 1 ms request does not spin, and an enumeration that fails keeps
the previous listing and logs once — a bus that cannot be read is not an empty
bus, so no removals are invented.

`stop()` signals and joins; the sleep ends at once, so a host never waits a whole
interval to shut down. Dropping the watch stops it, `stop` is safe twice, and
`is_running()` reports whether the thread is still up.

## Metrics

`device.metrics()` returns a `MetricsSnapshot`:

| Field | Counts |
| --- | --- |
| `frames_sent` | output reports written, handshake and closing CLOSE included |
| `frames_received` | input reports that belonged to this session and were accepted |
| `retries` | reply-loop read retries: an empty read inside the deadline, or a frame for another session or sequence |
| `reconnects` | transports reopened after a failed command or an expired lease |
| `lease_faults` | commands the device answered with a lease fault |
| `timeouts` | commands whose reply never arrived inside the timeout |
| `latency` | `count, min_us, p50_us, p95_us, max_us` for commands that waited |

`retries` is a read counter, not a retransmission counter: the SDK never re-sends
a command, so it measures how often the reply loop had to look past something.
Counters are per device, not per session — a reconnect keeps adding to the same
numbers, which is what makes `reconnects` and `lease_faults` readable across one.

Recording is one relaxed atomic add: no lock, no allocation, no formatting, which
is why the counters can sit on the path a mouse report takes. Reading is the
expensive half — `snapshot()` walks 4176 buckets (one per microsecond below 4 ms,
then eight per doubling up to 4,194,304 µs) without taking the session lock.
Below 4 ms `min_us` and `max_us` are the exact samples; above it they are the
edges of the bucket, and percentiles are the bucket's midpoint, off by at most
6%. A timeout is counted, never timed, and a fire-and-forget input opcode is a
frame without a sample.

`reset_metrics()` zeroes every counter and bucket and keeps the histogram's
storage; a report that races the reset may be lost, which is the only promise that
does not block the sender.

## Replay

`gdb::replay` records what the caller asked for and plays it back through
`Device` later: macros, regression repro, demos. A recording stores intent —
"key 7:0x04 down", "move by 40,-12", "one barrier" — so no bytes are stored and a
firmware change that keeps the opcodes cannot make it stale. The flip side is that
it cannot say what the device did; use the packet trace (`Level::Trace`) for that.

```rust
use gdb::replay::{NoCancel, Op, Recorder};

let mut recorder = Recorder::new(4, (1920, 1080));
device.keyboard().key_down(7, 0x04)?;
recorder.record(Op::Key { page: 7, usage: 0x04, down: true });
recorder.save("macro.json")?;

let recording = gdb::replay::Recording::load("macro.json")?;
let outcome = recording.play(&mut device, 1.0, &NoCancel)?;   // 2.0 = twice as fast
```

A saved file is one JSON object:

```json
{
  "version": 1,
  "seed": 16909060,
  "screen": [1920, 1080],
  "started_at_ms": 1767225600000,
  "entries": [
    { "at_ms": 0, "op": "move_raw", "dx": 40, "dy": -12 },
    { "at_ms": 12, "op": "key", "page": 7, "usage": 4, "down": true },
    { "at_ms": 96, "op": "wheel", "dx": -40, "dy": 120 },
    { "at_ms": 97, "op": "barrier" }
  ]
}
```

`op` is `key`, `buttons`, `wheel`, `move_by`, `move_raw`, `barrier`,
`release_all` or `wait`. `Wheel` keeps the horizontal axis in `dx` and the
vertical one in `dy`; `Buttons` is the whole bit mask, not a delta. Saving twice
is byte for byte identical — no maps, no timestamp added at save time — and
`version` is 1: a file with any other version, or none, is refused with `Invalid`
before an entry is looked at, so a half-understood macro never reaches the device.

Playback is deterministic: entries apply in order from a fixed timeline, with no
random source of its own (`Recorder` carries the gesture's `seed` for
regenerating it elsewhere). Gaps come from `at_ms` divided by `speed`, which must
be between 0.001 and 1000. Cancellation is checked between entries and inside
every wait; a cancelled playback stops, sends RELEASE_ALL if at least one entry
had played so no key or button stays down, and returns an `Outcome` with the rest
as `skipped` — a stopped macro is a normal ending, a device failure is an `Err`.

## Screens

`MotionOptions::screen` chooses the rectangle an absolute move is anchored to:
`platform::Screen::Primary` (the default), `Monitor(usize)` or `Window(isize)`.

```rust
use gdb::platform::Screen;

let options = gdb::MotionOptions::linear(400).with_screen(Screen::Monitor(1));
device.mouse().move_to(2400, 400, options)?;
```

Coordinates are physical pixels in virtual-desktop space, the shape
`GetMonitorInfoW` reports: the primary monitor's top-left corner is `(0, 0)` and a
second monitor to its right starts at the primary's width. The process is
per-monitor DPI aware before the pointer is read, and the pointer comes from
`GetPhysicalCursorPos`, so nothing is scaled.

`Primary` follows the primary-monitor flag, which need not be index 0.
`Monitor(i)` is the `i`-th rectangle in `EnumDisplayMonitors` order — not the
display number Windows settings shows. `Window(hwnd)` is that window's bounding
rectangle. The point is clamped into the rectangle, so a coarse target lands on
the edge instead of failing; the only point refused is one no monitor covers — a
gap between monitors, or past the last edge — with `Unsupported` and a message
naming both target and desktop:

```
absolute move to (4000, 500) targets monitor 1, but that point is on no screen:
the monitors cover (0, 0)-(3200, 1080)
```

A monitor index past the end, a window that no longer exists and a window with an
empty rectangle are refused before the pointer moves. The mock has one 1920×1080
screen: `Primary` and `Monitor(0)` work, any other index and any window is
`Unsupported`. On a host without the Windows cursor APIs an absolute move is
`Unsupported`. `gdb.h` passes no target, so a move through the DLL always uses the
primary monitor.

## Flashing

`gdb::flash` is the one implementation of build-and-flash in the workspace —
`src/uf2.rs` only re-exports its packer for `src/bin/uf2pack.rs`. The desktop
tool drives it through the seams below. `gdb.h` has no flashing symbol, so this
is the Rust crate's own surface.

```rust
let config = gdb::flash::Config::new("firmware");           // the folder holding app/
let image  = gdb::flash::build(&config, &gdb::flash::SystemRunner, &cancel, &mut report)?;
match gdb::flash::push(&config, Some(&device), &image, &cancel, &mut report)? {
    gdb::flash::Handshake::Verified(board) => {}            // board, firmware, lease
    gdb::flash::Handshake::Unverified(error) => {}          // copied, board stayed silent
}
```

`build` adds the `thumbv6m-none-eabi` target (skippable), runs `cargo build
--release --target thumbv6m-none-eabi --features hardware --manifest-path
firmware/app/Cargo.toml`, and packs the resulting ELF with `flash::uf2::pack`.

The packer flattens the ELF's flash segments (`0x1000_0000..0x1100_0000`) in
address order into one 256-byte aligned image, then writes 512-byte UF2 blocks,
one per 256-byte payload, with family id `0xE48BFF56`. An ELF with no flash
segment is rejected.

`push` sends BOOTLOADER (opcode 9), closes the runtime session — the board
refuses BOOTLOADER while a host keeps polling it — waits for the `RPI-RP2`
volume that was not mounted before this call (15 s), copies `Firmware.uf2` in
16 KiB chunks, then waits for the runtime HID to re-enumerate and reads Feature
14 on it (15 s). Verification is that second enumeration and board id 2, not the
copy's return: `Unverified` means the bytes are on the flash but the board never
answered, `Err` means the transfer itself failed.

`Runner`, `Reporter` and `Cancel` are the seams — cargo comes in through
`Runner`, the five stages and the log lines through `Reporter`, and every wait
polls `Cancel` — so the sequence runs headless in tests with no drive attached.

## Behaviour contract

- **Errors** — every call returns `ErrorCode`; there are no silent failures.
  `Ok=0, Invalid=1, Timeout=2, Disconnected=3, Busy=4, Lease=5, Framing=6,
  Overflow=7, Unsupported=8, Cancelled=9, Io=10`.
- **Recovery** — `Disconnected`, `Io` and `Lease` failures trigger one reconnect
  and one retry when auto-reconnect is on. A `Timeout` never retries, because
  retrying a half-delivered input command is worse than reporting it.
- **Lease** — the board renews the session lease only on OPEN, HEARTBEAT,
  RELEASE_ALL, BOOTLOADER and valid input; STATUS, BARRIER, STATUS_EXT and
  SESSION_LIST do not renew, so a host that polls any of them in a loop would
  still lose the session. The SDK keeps its own clock: every 500 ms without a
  renewing command it sends HEARTBEAT, against the 2000 ms lease.
- **Threads** — a heartbeat thread every 500 ms; movement runs on its own thread
  when started asynchronously; each scheduler owns a worker thread. Callbacks
  run on those threads and must not block.
- **Absolute moves** — the SDK reads the physical cursor and feeds the delta back
  through `RelativeTracker`; the rectangle the move is anchored to comes from
  `MotionOptions::screen` — the primary monitor by default, a monitor index or a
  window's rectangle otherwise. Only a point no monitor covers is refused. The
  mock uses its own virtual cursor on one 1920×1080 screen instead.
- **Broker** — one device per broker, one session behind it; commands are atomic,
  sequences are not (see [Broker](#broker)).
- **Watch** — one thread per watch, a callback on that thread, and nothing read
  from the vendor collection; the first poll reports what is already plugged in.
- **Scheduling order** — earliest due time, then higher priority, then submission
  order. A cancel wins only if the callback has not started.
- **Close** — `close` releases the session, stops the heartbeat and clears held
  input; the handle is then unusable.

## Platform support

| | Physical HID | Mock |
| --- | --- | --- |
| Windows | yes (hidapi) | yes |
| Linux / macOS | compiles, returns `Unsupported` | yes |

The mock lives above the transport, so everything except the Go binding runs on
any platform without hardware. `gdb::watch`, `gdb::metrics` and `gdb::replay`
follow the device and work wherever the mock does; `gdb::broker` needs Windows
named pipes and answers `Unsupported` elsewhere. `sdk/go/gdb.go` is the
exception: it loads `gdb.dll` through `syscall.NewLazyDLL`, so it needs Windows
and a built `gdb.dll`.

## Verification

```
cargo test -p gdb-motion -p gdb                  # 92 tests: 45 unit, 41 integration, 5 motion, 1 doc
cargo test -p gdb-ffi                            # 5 through the C ABI: command, move_raw, log sink, attach
python sdk/python/test_gdb.py                    # 20 tests through ctypes, 5 of them attached
cd sdk/go && go test ./...                       # 11 tests through the DLL
g++ -std=c++17 -I sdk/ffi/include -I sdk/cpp sdk/cpp/examples/mock.cpp   # one program, run it
bash scripts/check-sdk.sh                        # all of the above, run end to end
```

The Rust counts are the ones `cargo test` prints: `gdb`'s unit tests, its five
integration files (`broker` 12, `metrics` 8, `replay` 7, `sdk` 10, `watch` 4), the
`gdb-motion` suite and the one doc example in `gdb::replay`. `sdk/cpp/examples/mock.cpp`
is a single program with assertions rather than a test framework, so it reports
one line — `cpp mock ok` — after walking the whole surface against the mock
device. The Python and Go suites also upgrade-check the ABI: every `gdb_attach`
and `gdb_remote_*` symbol the header declares must be exported by the DLL.

`scripts/check-sdk.sh` is also part of `scripts/preflight.sh` and locates a Go
toolchain outside PATH when needed (`~/sdk/go*/go/bin/go`, scoop, Program Files).
