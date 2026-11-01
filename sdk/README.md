# GDBOX SDK

Host-side control and automation for the RP2040 GDBOX firmware. The firmware
only moves USB HID bytes; everything above that — device management, absolute
coordinates, motion profiles, scheduling — lives here.

```
sdk/
├── motion/          gdb-motion   pure motion math (integer-friendly, no I/O)
├── rust/            gdb          native Rust SDK        (feature complete)
├── ffi/             gdb-ffi      C ABI + include/gdb.h  (one implementation)
├── cpp/             gdb.hpp      header-only C++17 binding
├── python/          gdb.py       ctypes binding
└── go/              gdb.go       Windows binding (no cgo)
```

Rust is the reference implementation. The other three bindings call the same C
ABI, so they cannot drift in behaviour — only in naming style.

## Build

```bash
cargo build -p gdb-ffi --release      # target/release/gdb.dll (or libgdb.so)
cargo test  -p gdb -p gdb-motion      # 23 tests, mock device, no hardware
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
device->mouse().move_to(640, 360, 250);
device->keyboard().type_text("hello");
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

| Requirement | Status | Notes |
| --- | --- | --- |
| Connect / disconnect | done | Feature 14 handshake, OPEN, CLOSE |
| Connection state | done | `Disconnected/Connecting/Connected/Error` |
| Auto reconnect | done | once per failed call; held keys are not replayed |
| Multi-device | done | `Manager::list` + open by serial or path |
| Device info | done | board, firmware, lease, poll, capabilities, device id, build |
| Timeouts / errors | done | 11-code enum, 100 ms floor, 1000 ms default |
| Relative move | done | split into ±1024 HID chunks |
| Absolute move | done | host reads the cursor, closes the loop per step |
| Buttons, click, double, drag, wheel, release-all | done | firmware owns the union of button owners |
| Arrive within a time budget | done | `duration_ms`, 0 = natural duration |
| Linear / ease in / ease out / in-out / jerk / bezier | done | `Ease` enum + `CubicBezier` |
| Speed and acceleration | done | duration plus the easing curve |
| Custom curve / multi-segment path | done | piecewise bezier, `move_path` |
| Cancel / retarget mid-flight | done | `Movement::cancel` / `retarget` |
| Key down/up, press, chord, long press, sequence, text | done | US layout; unsupported characters are dropped |
| Task queue, delay, sync/async, pause/resume/cancel, priorities | done | one worker thread per scheduler |
| Periodic scheduling | done | `every(interval)` until cancelled |
| Unified error codes | done | C ABI enum shared by all four bindings |
| Logging / debug mode | done | level + sink (`log::set_level`, `gdb_set_log_sink`); `Trace` logs every frame in both directions |
| Mock device | done | full protocol mock with a virtual 1920×1080 cursor |
| Raw command passthrough | Rust only | `Device::command`; no C ABI symbol, so the other bindings keep the typed calls |
| Event callbacks | done | state changes and errors |
| Custom HID commands | out of scope for v1 | `Device::command` passes existing opcodes only; the firmware rejects unknown ones |
| API docs / examples / unit tests | done | this file, `API.md`, one example per language |

## Behaviour contract

- **Errors** — every call returns `ErrorCode`; there are no silent failures.
  `Ok=0, Invalid=1, Timeout=2, Disconnected=3, Busy=4, Lease=5, Framing=6,
  Overflow=7, Unsupported=8, Cancelled=9, Io=10`.
- **Recovery** — `Disconnected`, `Io` and `Lease` failures trigger one reconnect
  and one retry when auto-reconnect is on. A `Timeout` never retries, because
  retrying a half-delivered input command is worse than reporting it.
- **Threads** — a heartbeat thread every 500 ms; movement runs on its own thread
  when started asynchronously; each scheduler owns a worker thread. Callbacks
  run on those threads and must not block.
- **Absolute moves** — the SDK reads the physical cursor and feeds the delta back
  through `RelativeTracker`, so a real device needs the cursor on the primary
  monitor. The mock uses its own virtual cursor instead.
- **Scheduling order** — earliest due time, then higher priority, then submission
  order. A cancel wins only if the callback has not started.
- **Close** — `close` releases the session, stops the heartbeat and clears held
  input; the handle is then unusable.

## Platform support

| | Physical HID | Mock |
| --- | --- | --- |
| Windows | yes (hidapi, `GetPhysicalCursorPos`) | yes |
| Linux / macOS | compiles, returns `Unsupported` | yes |

The C ABI and all four bindings work on every platform for the mock; the Go
binding currently uses the Windows loader directly and is Windows-only.

## Verification

```
cargo test -p gdb-motion -p gdb                  # 23 tests
python sdk/python/test_gdb.py                    # 12 tests through ctypes
cd sdk/go && go test ./...                       # 5 tests (Windows)
g++ -std=c++17 -I sdk/ffi/include -I sdk/cpp sdk/cpp/examples/mock.cpp
bash scripts/check-sdk.sh                        # all of the above, run end to end
```

`scripts/check-sdk.sh` is also part of `scripts/preflight.sh` and locates a Go
toolchain outside PATH when needed (`~/sdk/go*/go/bin/go`, scoop, Program Files).
