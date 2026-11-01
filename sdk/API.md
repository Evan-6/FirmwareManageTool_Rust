# GDBOX API reference

`include/gdb.h` is the contract; this file names the same operations in every
implementation. Rust is `gdb`, C++ is `gdb::`, Python is module `gdb`, Go is
package `gdb`. Every failure raises or returns the same `ErrorCode`.

C++, Python and Go each ship two implementations: the binding over `gdb.dll`
(`sdk/cpp/gdb.hpp`, `sdk/python/gdb.py`, `sdk/go/gdb.go`) and a native one that
speaks vendor HID itself (`sdk/cpp/native/gdb_native.hpp` in namespace
`gdb::native`, `sdk/python/native/gdb.py`, package `gdbnative` in
`sdk/go/native`). The names below are the binding's; the native ones differ only
where the second table says so, return typed values instead of JSON strings, and
add `command`, `move_raw` and the flashing sequence.

## Naming map

| Concept | Rust | C++ | Python | Go |
| --- | --- | --- | --- | --- |
| open by selector | `Manager::open(sel, opts)` | `Device::open(sel)` | `Device.open(sel)` | `Open(sel, …)` |
| open the mock | `Manager::open_mock_with_handle` | `Device::open_mock` | `Device.open_mock` | `OpenMock` |
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
| log sink | `log::set_sink(fn)` | — (native `set_log_sink`) | `set_log_sink(fn)` | `SetLogSink(fn)` |
| relative move | `move_by(dx, dy, opts)` | `move_by(dx, dy, ms)` | `.move_by(dx, dy, ms)` | `MoveBy(dx, dy, ms, p)` |
| raw relative move (no barrier) | `move_raw(dx, dy)` | — | native `move_raw(dx, dy)` | native `MoveRaw(dx, dy)` |
| raw command passthrough | `command(&Command)` | — | native `command(opcode, payload, wait)` | native `Command(opcode, payload)` |
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

## Native naming

Same shape, typed results, plus the three operations the C ABI does not carry.
Only the differences:

| Concept | Python `gdb` (native) | Go `gdbnative` | C++ `gdb::native` |
| --- | --- | --- | --- |
| open | `Device.open(selector, …)` | `Open` / `OpenWithTransport` | `Device::open(selector, …)` |
| mock | `Device.open_mock` | `OpenMock` / `OpenMockOptions` | `Device::open_mock` / `open_fake` |
| list | `list_devices()` | `List()` / `ListJSON()` | `device_list()` / `Device::list_json()` |
| info / status | `.info`, `.status` (typed, also `["camelCase"]`) | `Info()`, `Status()` | `info()`, `status()` |
| raw command | `Device.command(opcode, payload, expects_reply=None)` | `Device.Command(opcode, payload)` | `Device::command(opcode, payload, wait_for_reply)` |
| raw move | `Mouse.move_raw(dx, dy)` | `Mouse.MoveRaw(dx, dy)` | `Mouse::move_raw(dx, dy)` |
| log level / sink | `set_log_level` / `set_log_sink` | `SetLogLevel` / `SetLogSink` | `log::set_level` / `log::set_sink` |
| flash | `gdb.flash` (`.build`, `.push`, `.uf2.pack`) | `flash.Build` / `flash.Push` / `UFPack` | `flash::build` / `flash::push` / `flash::uf2_pack` |
| errors | `GdbError` (`code`, `message`) | `error` / `*Error` with `Code` | `gdb::native::Error` |

C++ over the DLL (`sdk/cpp/gdb.hpp`) exposes `set_log_level` only: the header
loads `gdb_set_log_sink` but does not wrap it. `sdk/go/gdb.go` has no
`StartMovePath`.

## Device

```rust
pub struct DeviceOptions { pub auto_reconnect: bool, pub timeout_ms: u32, pub heartbeat: bool }
// default: true, 1000, true

DeviceInfo { serial, product, board, firmware, lease_ms, poll_ms,
             capabilities, device_id: [u8; 8], build: u32 }
```

`DeviceEntry` (device list): `{ path, serial, product, manufacturer, vid, pid }`.

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

`gdb.h` has no symbol for it, so the bindings over the DLL have only the typed
calls. The native implementations carry it with an opcode and a payload instead
of a `Command` value:

| | Signature | When no reply is expected |
| --- | --- | --- |
| Rust | `Device::command(&Command) -> Result<Option<Packet>>` | `None` |
| C++ native | `Device::command(opcode, payload, wait_for_reply)` → `std::optional<Packet>` | `std::nullopt` |
| Python native | `Device.command(opcode, payload=b"", expects_reply=None)` | `None` |
| Go native | `Device.Command(opcode, payload) (Packet, error)` | zero `Packet` |

Python infers the reply from the opcode when `expects_reply` is `None`
(`REPLY_OPCODES` = OPEN, CLOSE, STATUS, BARRIER, RELEASE_ALL, STATUS_EXT,
SESSION_LIST, BOOTLOADER); C++ asks every call with `wait_for_reply`. All four
keep the same session, sequence counter, timeout and reconnect path as
`status()`, and all four leave the SDK's bookkeeping untouched.

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
pub struct MotionOptions { pub duration_ms: u32, pub profile: HumanProfile }
// duration_ms = 0 → natural duration from the distance

pub struct HumanProfile {
  curve: f64, jitter_px: f64, overshoot_pct: f64, settle_ms: u32,
  ease: Ease, min_step_px: i32,
}
pub enum Ease { Jerk, EaseInOut, Linear, EaseIn, EaseOut,
                CubicBezier { x1, y1, x2, y2 } }
```

C ABI `gdb_profile { ease, bezier[4], curve, jitter_px, overshoot_pct,
settle_ms, min_step_px }`, ease codes `0 jerk, 1 in-out, 2 linear, 3 in,
4 out, 5 cubic-bezier`. A NULL profile means the SDK default.

`Movement`: `cancel()`, `retarget(x, y)`, `is_finished()`, `wait_timeout(Duration)`
(returns `Timeout` and leaves the movement running) and `wait()` (consumes it).
Dropping a movement cancels it. In the C ABI the same handle answers
`gdb_movement_cancel`, `gdb_movement_retarget`, `gdb_movement_is_finished`,
`gdb_movement_wait(timeout_ms)` and `gdb_movement_free`, which cancels.

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

## Errors

| Code | Name | Meaning |
| --- | --- | --- |
| 0 | Ok | — |
| 1 | Invalid | bad argument, unknown usage, wrong session id |
| 2 | Timeout | no reply / movement did not settle |
| 3 | Disconnected | device gone, no match for the selector |
| 4 | Busy | eight owners, or the session id is already open |
| 5 | Lease | the 2 s lease expired |
| 6 | Framing | malformed frame or response |
| 7 | Overflow | device queue overflow / stale output |
| 8 | Unsupported | platform or firmware lacks the feature |
| 9 | Cancelled | the operation was cancelled |
| 10 | Io | HID read/write failed, or a panic at the ABI boundary |

C ABI errors live in `gdb_last_error(buffer, capacity)`; the value returned by a
failing call equals the code. Bindings convert this into their native exception
type: `gdb::Error` and `gdb::native::Error`, `gdb.GdbError` (both Python
modules), `*gdb.Error` and `*gdbnative.Error`.

## C ABI

`include/gdb.h` is the contract: `gdb_abi_version() == 1`, `gdb_version()` is the
SDK version string. Handles are opaque (`gdb_device`, `gdb_movement`,
`gdb_scheduler`); strings returned by the library must be freed with
`gdb_string_free`. Callbacks:

```c
void (*gdb_event_fn)(void *user, int32_t kind, int32_t code, const char *msg);
int32_t (*gdb_task_fn)(void *user);   // 0 = ok, otherwise an ErrorCode
void (*gdb_log_fn)(void *user, int32_t level, const char *msg);
```

Event kinds: `0` state (code = `ConnectionState`), `2` error (code = ErrorCode).
The header has no symbol for a raw command, a raw move or flashing — those live
in the native implementations, not in `gdb.dll`.

## Logging

Logging is process-wide and silent until both a level and a sink are set:

| | Level | Sink |
| --- | --- | --- |
| Rust | `log::set_level(Level)` | `log::set_sink(Some(Box::new(f)))` |
| C ABI | `gdb_set_log_level(uint8_t)` | `gdb_set_log_sink(gdb_log_fn, void *)` |
| C++ native | `log::set_level(int32_t)` | `log::set_sink(fn)` |
| C++ over the DLL | `set_log_level(int32_t)` | — (the symbol is loaded, not wrapped) |
| Python (both) | `set_log_level(int)` | `set_log_sink(fn)` |
| Go (both) | `SetLogLevel(uint8_t)` | `SetLogSink(fn)` |

Levels are `0 Off, 1 Error, 2 Warn, 3 Info, 4 Debug, 5 Trace`. The SDK writes at
`Info` (connect, close, reconnect), `Warn` (a command failed, with its code) and
`Trace`; nothing emits `Error` or `Debug` today.

`Trace` is the packet log. Frames are logged in `Session`, underneath the C ABI,
so all four bindings get identical lines, and the native packages print the same
text from their own session loop:

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
does not expose any of it, so flashing is Rust or one of the native packages.

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
the runtime session, waits for a volume that was not mounted before (30 s, 15 s
in Rust), copies in 16 KiB chunks and then waits (15 s) for the runtime HID to
re-enumerate so it can read Feature 14.

`Handshake` is `Verified(Verified { board, firmware, lease_ms })` or
`Unverified(Error)`: `Err` from `push` means the transfer failed, `Unverified`
means the bytes are on the flash but the board never answered. The native
packages model the same outcome as `gdb.flash.Outcome` (`verified`, `board`,
`firmware`, `lease_ms`, `message`, `error`), `flash.Outcome { Verified,
Unverified }` with `OK()` and `Label()`, and `flash::Handshake { ok, verified,
failure }`.

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
* Heartbeat: every 500 ms while connected. The Rust and Python loops take the
  session lock without blocking and skip a tick while a command holds it; the
  C++ heartbeat waits for the same mutex.
* Async movement: one thread per movement; `retarget` is thread-safe.
* Scheduler: one worker per scheduler; callbacks never overlap.
* Event callbacks may run on the heartbeat, worker or caller thread; they must
  return quickly and must not call back into the same device synchronously.
