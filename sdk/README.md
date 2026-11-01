# GDBOX SDK

Host-side control and automation for the RP2040 GDBOX firmware. The firmware
only moves USB HID bytes; everything above that — device management, absolute
coordinates, motion profiles, scheduling — lives here.

```
sdk/
├── motion/          gdb-motion      pure motion math (integer-friendly, no I/O)
├── rust/            gdb             reference SDK: device, mouse, keyboard,
│                                    scheduler, motion, log, flash
├── ffi/             gdb-ffi         C ABI (include/gdb.h) + cdylib
├── cpp/             gdb.hpp         header-only C++17 binding over the DLL
│   └── native/      gdb_native.hpp  C++17 SDK that speaks vendor HID itself
├── python/          gdb.py          ctypes binding over the DLL
│   └── native/      gdb.py          Python SDK that speaks vendor HID itself
└── go/              gdb.go          Windows binding over the DLL (no cgo)
    └── native/      package gdbnative  Go SDK that speaks vendor HID itself
```

## Layers

| Layer | Lives in | Speaks | Depends on |
| --- | --- | --- | --- |
| Reference | `sdk/rust` | vendor HID protocol 3 | `shared/input-protocol`, `sdk/motion` |
| C ABI | `sdk/ffi` | the Rust SDK | `gdb` |
| Bindings | `sdk/cpp/gdb.hpp`, `sdk/python/gdb.py`, `sdk/go/gdb.go` | `gdb.dll` | the C ABI, loaded at runtime |
| Native | `sdk/cpp/native`, `sdk/python/native`, `sdk/go/native` | vendor HID protocol 3 | the OS HID API only |

`gdb` (Rust) is authoritative: a behaviour lands there first. `gdb-ffi` forwards
it across `include/gdb.h`, and the three bindings are name translation over that
header — they have no logic of their own, so they cannot drift, only lag.

The `native/` implementations are independent transcriptions of the same
protocol, for hosts that will not load a DLL: one file to vendor, no C
toolchain, no cgo. Nothing generates them, so three checks keep them honest:

- `shared/input-protocol/vectors.json` — golden 64-byte frames, replayed by
  `shared/input-protocol`'s own tests, `sdk/python/native/test_native.py`,
  `sdk/go/native/wire_test.go` and `sdk/cpp/native/gdb_native_test.cpp`.
- The UF2 packers, compared byte for byte against the Rust packer on the real
  firmware ELF, and `sdk/go/native/testdata/motion.json`, captured from
  `sdk/motion`.
- `scripts/check-sdk.sh`, which builds and runs every suite in one command.

A native implementation adds what the C ABI does not carry — `command`,
`move_raw` and the flashing sequence — and is otherwise the same surface with
typed results instead of JSON strings.

## Build

```bash
cargo build -p gdb-ffi --release      # target/release/gdb.dll (or libgdb.so)
cargo test  -p gdb -p gdb-motion      # 44 tests, mock device, no hardware
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

Rows are capabilities, columns are implementations. A bare name is the binding
over `gdb.dll`; `native` means the implementation under `sdk/*/native`, which
loads no DLL. `—` is a real gap, not an omission.

| Capability | Rust `gdb` | C ABI | C++ | Python | Go |
| --- | --- | --- | --- | --- | --- |
| Connect, OPEN/CLOSE, Feature 14 handshake | `Manager::open`, `close` | `gdb_open`, `gdb_close` | `Device::open`, `close()` | `Device.open`, `.close()` | `Open`, `Close()` |
| Mock device, virtual 1920×1080 cursor | `Manager::open_mock` | `gdb_open_mock` | `open_mock` | `open_mock` | `OpenMock` |
| Device list, open by serial or path | `Manager::list` | `gdb_device_list` | `list_json` | `list_devices` | `ListJSON` |
| Info: board, firmware, lease, poll, capabilities, device id, build | `info()` | `gdb_device_info` | `info_json` | `.info` | `InfoJSON` |
| STATUS / STATUS_EXT / SESSION_LIST | `status`, `status_ext`, `sessions` | `gdb_status`, `gdb_status_ext`, `gdb_sessions` | `*_json()` | `.status`, `.status_ext`, `.sessions` | `*JSON()` |
| Relative move, split at ±1024 per axis | `move_by` | `gdb_mouse_move_by` | `move_by` | `.move_by` | `MoveBy` |
| Raw relative move, no barrier, no curve | `move_raw` | — | native `move_raw` | native `move_raw` | native `MoveRaw` |
| Absolute move, host reads the cursor | `move_to` | `gdb_mouse_move_to` | `move_to` | `.move_to` | `MoveTo` |
| Multi-segment path | `move_path` | `gdb_mouse_move_path` | `move_path` | `.move_path` | `MovePath` |
| Background move, cancel / retarget / wait | `start_move_*`, `Movement` | `gdb_mouse_start_move_*`, `gdb_movement_*` | `start_move_*`, `Movement` | `.start_move_*`, `Movement` | `StartMoveTo/By` |
| Buttons, click, double click, drag, wheel, release-all | `button`, `click`, `drag`, `wheel` | `gdb_mouse_*` | `button`, `click`, `drag`, `wheel` | same | `Button`, `Click`, `Drag`, `Wheel` |
| Keys: down/up, press, long press, chord, sequence, text | `key_down`, `long_press`, `chord`, `type_text` | `gdb_key_*`, `gdb_keys_*`, `gdb_type_text` | same | same | `KeyDown`, `LongPress`, `Chord`, `TypeText` |
| Scheduler: submit, every, pause/resume/cancel, wait_idle, take_errors | `Scheduler` | `gdb_scheduler_*` | `Scheduler` | `.scheduler()` | `Scheduler()` |
| Events: state changes and errors | `set_event_handler` | `gdb_set_event_handler` | `set_event_handler` | `.set_event_handler` | `SetEventHandler` |
| Log level 0–5 | `log::set_level` | `gdb_set_log_level` | `set_log_level` | `set_log_level` | `SetLogLevel` |
| Log sink | `log::set_sink` | `gdb_set_log_sink` | — (native `set_log_sink`) | `set_log_sink` | `SetLogSink` |
| `Trace` frames, identical text in every layer | `Level::Trace` | as the SDK | as the SDK | as the SDK | as the SDK |
| Heartbeat, one reconnect per failed call | built in | `gdb_set_auto_reconnect` | `set_auto_reconnect` | `.set_auto_reconnect` | `SetAutoReconnect` |
| Raw command passthrough | `Device::command` | — | native `Device::command` | native `Device.command` | native `Device.Command` |
| Flash: cargo build, ELF → UF2, BOOTSEL copy, verify | `gdb::flash` | — | native `gdb::native::flash` | native `gdb.flash` | native `flash` |
| Unified error codes | `ErrorCode` (11) | `gdb_error` | `ErrorCode` | `ErrorCode` | `Code` |

Every row is implemented in Rust and in all three native packages. The `—` cells
are the C ABI's: `gdb.h` has no symbol for a raw move, a raw command or flashing,
and `sdk/cpp/gdb.hpp` loads `gdb_set_log_sink` without wrapping it.

Known gaps:

- The C ABI has no flashing, no `command` and no `move_raw`, so the C++/Python/Go
  bindings over `gdb.dll` have none either. Use the native package, or Rust.
- `sdk/cpp/gdb.hpp` loads `gdb_set_log_sink` but exposes only `set_log_level`;
  the native header exposes both.
- `sdk/go/gdb.go` has no `StartMovePath` (the C ABI and `sdk/go/native` do).
- Raw passthrough sends existing opcodes only; the firmware rejects unknown ones.

## Flashing

`gdb::flash` is the one implementation of build-and-flash in the workspace —
`src/uf2.rs` only re-exports its packer for `src/bin/uf2pack.rs`. The desktop
tool drives it through the seams below, and the three native SDKs port the same
sequence; the C ABI does not wrap it.

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
volume that was not mounted before this call (30 s, 15 s in the Rust SDK),
copies `Firmware.uf2` in 16 KiB chunks, then waits for the runtime HID to
re-enumerate and reads Feature 14 on it. Verification is that second
enumeration and board id 2, not the copy's return: `Unverified` means the bytes
are on the flash but the board never answered, `Err` means the transfer itself
failed.

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
  through `RelativeTracker`, so a real device needs the cursor on the primary
  monitor. The mock uses its own virtual cursor instead.
- **Scheduling order** — earliest due time, then higher priority, then submission
  order. A cancel wins only if the callback has not started.
- **Close** — `close` releases the session, stops the heartbeat and clears held
  input; the handle is then unusable.

## Platform support

| | Physical HID | Mock |
| --- | --- | --- |
| Windows | yes (hidapi in Rust, Win32 HID in the native packages) | yes |
| Linux / macOS | compiles, returns `Unsupported` | yes |

The mock lives above the transport, so every layer except the Go DLL binding
runs on any platform without hardware. `sdk/go/gdb.go` is that exception: it
loads `gdb.dll` through `syscall.NewLazyDLL` and is Windows-only, while
`sdk/go/native` keeps its Windows code in `transport_windows.go` behind a build
tag and still builds elsewhere.

## Verification

```
cargo test -p gdb-motion -p gdb                  # 44 tests: 29 + 10 + 5
python sdk/python/test_gdb.py                    # 12 tests through ctypes
python sdk/python/native/test_native.py          # 83 tests, 1 skipped when the ELF and UF2 differ
cd sdk/go && go test ./...                       # 5 through the DLL + 101 native (Windows)
g++ -std=c++17 -I sdk/ffi/include -I sdk/cpp sdk/cpp/examples/mock.cpp
g++ -std=c++17 -I sdk/cpp/native sdk/cpp/native/gdb_native_test.cpp   # one binary, 24 test functions
bash scripts/check-sdk.sh                        # all of the above, run end to end
```

`scripts/check-sdk.sh` is also part of `scripts/preflight.sh` and locates a Go
toolchain outside PATH when needed (`~/sdk/go*/go/bin/go`, scoop, Program Files).
