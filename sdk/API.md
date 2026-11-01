# GDBOX API reference

One behaviour, four spellings. Rust is `gdb`, C++ is `gdb::`, Python is `gdb.`,
Go is package `gdb`. Every failure raises/returns the same `ErrorCode`.

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
| log sink | `log::set_sink(fn)` | `set_log_sink(fn)` | `set_log_sink(fn)` | `SetLogSink(fn)` |
| relative move | `move_by(dx, dy, opts)` | `move_by(dx, dy, ms)` | `.move_by(dx, dy, ms)` | `MoveBy(dx, dy, ms, p)` |
| raw relative move (no barrier) | `move_raw(dx, dy)` | — | — | — |
| raw command passthrough | `command(&Command)` | — | — | — |
| absolute move | `move_to(x, y, opts)` | `move_to(x, y, ms)` | `.move_to(x, y, ms)` | `MoveTo(x, y, ms, p)` |
| path | `move_path(&[(x,y)], opts)` | `move_path({{{…}}}, ms)` | `.move_path([(x,y)], ms)` | `MovePath([][2]int32, ms, p)` |
| async move | `start_move_to/by/path` | `start_move_to/by/path` | `.start_move_to/by/path` | `StartMoveTo/By` |
| button | `button(Button, down)` | `button(Button, down)` | `.button(btn, down)` | `Button(btn, down)` |
| click | `click(Button, count)` | `click(Button, n)` | `.click(btn, n)` | `Click(btn, n)` |
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
they keep the held state and the mock honest. There is no C ABI symbol, so C++,
Python and Go have only the typed calls.

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

`Movement`: `cancel()`, `retarget(x, y)`, `wait(timeout)`, `is_finished()`.
Dropping/freeing a movement cancels it.

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
type (`gdb::Error`, `gdb.GdbError`, `*gdb.Error`).

## C ABI

`include/gdb.h` is the contract: `gdb_abi_version() == 1`. Handles are opaque
(`gdb_device`, `gdb_movement`, `gdb_scheduler`); strings returned by the library
must be freed with `gdb_string_free`. Callbacks:

```c
void (*gdb_event_fn)(void *user, int32_t kind, int32_t code, const char *msg);
int32_t (*gdb_task_fn)(void *user);   // 0 = ok, otherwise an ErrorCode
void (*gdb_log_fn)(void *user, int32_t level, const char *msg);
```

Event kinds: `0` state (code = `ConnectionState`), `2` error (code = ErrorCode).

## Logging

Logging is process-wide and silent until both a level and a sink are set:
`log::set_level(Level)` + `log::set_sink(fn)`, `gdb_set_log_level(uint8_t)` +
`gdb_set_log_sink(gdb_log_fn, void *)`, `SetLogLevel` + `SetLogSink`,
`set_log_level(int)` + `set_log_sink(fn)`. Levels are `0 Off, 1 Error, 2 Warn,
3 Info, 4 Debug, 5 Trace`; the SDK currently writes at `Info` (connect, close,
reconnect), `Warn` (a command failed, with its error code) and `Trace`.

`Trace` is the packet log. Frames are logged in `Session`, underneath the C ABI,
so all four bindings get identical lines:

```
out opcode=6 session=1234 sequence=2 payload=[…]
in opcode=134 session=1234 sequence=2 payload=[…]
skip opcode=134 session=4321 sequence=7 (not ours)
```

`skip` is an IN frame that belonged to another session and was dropped while
waiting for a reply. A host that wants a packet window or a log pane subscribes
to the sink instead of opening a second reader on the IN endpoint — two readers
steal each other's replies and break the sequence numbers of both sessions.

## Threading rules

* Heartbeat: every 500 ms while connected; blocked while a command holds the
  session lock.
* Async movement: one thread per movement; `retarget` is thread-safe.
* Scheduler: one worker per scheduler; callbacks never overlap.
* Event callbacks may run on the heartbeat, worker or caller thread; they must
  return quickly and must not call back into the same device synchronously.
