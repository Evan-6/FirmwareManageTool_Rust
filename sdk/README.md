# GDBOX SDK

Host-side control of the GDBOX board from your own program: open a session, move
the pointer, type, schedule work, attach to a session another process owns. The
firmware only moves USB HID bytes; everything above that is here.

## Two layers

`gdb` (`sdk/rust`) is the SDK: the crate that implements every behaviour.

Everything outside Rust reaches it through the C ABI. `sdk/ffi/include/gdb.h`
declares it, `gdb.dll` exports it, and `sdk/cpp/gdb.hpp`, `sdk/python/gdb.py` and
`sdk/go/gdb.go` translate those names into C++, Python and Go. The bindings are
name translation only, so they cannot drift from the SDK — only lag it. A
language without a binding (C#, JNI, Node, Godot) calls the same `gdb_*`
functions directly, which is all the three bindings do.

The ABI is major 1, minor 10 (`GDB_ABI_VERSION_MINOR`); `gdb_abi_version()`
answers the major at run time, `gdb_abi_version_full()` the major and the minor,
`gdb_capabilities()` the abilities behind them, and `gdb_version()` the SDK
string.

## Choose your handle

| Your program… | Handle | C ABI | Rust | What it costs |
| --- | --- | --- | --- | --- |
| is the only one controlling the board | direct, exclusive | `gdb_open` | `Manager::open` | a reply another process takes is one `Timeout` |
| controls the board while others may command it too | direct, shared | `gdb_open_shared` | `Device::open_shared` | a taken reply is one `Busy` naming the command, and the SDK never reopens the session on its own |
| only displays the board's numbers while another program controls it | presence | `gdb_probe`, `gdb_probe_all`, `gdb_observer_start` | `probe::read_selector`, `probe::all`, `Observer::start` | no session and no packet, so no owner slot is taken; every command on an observer answers `Unsupported` |
| sends input to a board another process owns | attached | `gdb_attach`, `gdb_remote_*` | `broker::Client::connect` | the broker's op set — including the broker-side absolute move — and no drag, typing, chord, scheduler or events |
| runs with no hardware at all | mock | `gdb_open_mock`, `gdb_open_mock_shared`, `gdb_open_mock_layout` | `Manager::open_mock`, `Device::open_mock_shared`, `Manager::open_mock_layout` | one 1920×1080 virtual screen and a virtual cursor, unless the caller describes the desktop |

The first two rows are one `gdb_device` handle; presence and attachment are their
own handle types, so a process may hold a direct handle and an attached one at
the same time. Which of two shared programs reads a reply is a discipline
question, not an API one: nothing arbitrates it.

## Quickstart

Build the library once. C++, Python and Go load it at run time, so none of them
needs an import library or a build step of its own.

```sh
cargo build -p gdb-ffi --release        # target/release/gdb.dll
```

Each snippet opens the first attached board, prints `status`, clicks the left
button and closes. The library is found through `GDBOX_LIBRARY` (a full path to
`gdb.dll`) or next to the executable. Swapping the open call for
`gdb_open_mock(1000, true, true)` / `open_mock()` / `OpenMock()` runs the same
code against a mock device with no board attached.

### C++

```cpp
#include <iostream>
#include "gdb.hpp"

int main() {
    auto device = gdb::Device::open();          // open_mock() needs no board
    std::cout << device.status_json() << "\n";
    device.mouse().click(gdb::Button::Left);    // throws gdb::Error on failure
    device.close();
}
```

`g++ -std=c++17 -I sdk/ffi/include -I sdk/cpp quick.cpp` — nothing to link, the
header loads the DLL itself.

### Python

```python
import gdb

with gdb.Device.open() as device:               # open_mock() needs no board
    print(device.status)                        # dict, the STATUS block
    device.mouse.click(gdb.Button.LEFT)         # raises gdb.GdbError on failure
```

Put `sdk/python` on `PYTHONPATH`; `gdb.py` is the module and ctypes is the
loader.

### Go

```go
package main

import (
	"fmt"
	"log"

	gdb "github.com/nousresearch/gdbox-sdk"
)

func main() {
	device, err := gdb.Open("", 1000, true, true) // gdb.OpenMock() needs no board
	if err != nil {
		log.Fatal(err)
	}
	defer device.Close()

	status, err := device.StatusJSON()
	if err != nil {
		log.Fatal(err)
	}
	fmt.Println(status)

	if err := device.Mouse().Click(gdb.Left, 1); err != nil {
		log.Fatal(err)
	}
}
```

Windows only and no cgo: the package loads `gdb.dll` with
`syscall.NewLazyDLL`.

### C

```c
#include <stdio.h>
#include "gdb.h"

int main(void) {
    gdb_device *device = gdb_open(NULL, 1000, true, true);   /* gdb_open_mock: no board */
    if (device == NULL) {
        char message[256];
        gdb_last_error(message, sizeof message);             /* a NULL open carries its code here */
        fprintf(stderr, "open: %s\n", message);
        return 1;
    }

    char *status = gdb_status(device);                       /* JSON; free with gdb_string_free */
    if (status != NULL) {
        printf("%s\n", status);
        gdb_string_free(status);
    }

    gdb_mouse_click(device, GDB_BUTTON_LEFT, 1);             /* every int32_t return is a gdb_error */
    gdb_close(device);
    gdb_device_free(device);
    return 0;
}
```

`gcc -std=c11 -I sdk/ffi/include quick.c gdb.dll.lib` — the import library sits
next to the DLL. A C program links it once; the bindings load the DLL instead.

## What is covered

The third column is the whole C ABI: a symbol missing from it is a symbol the
table does not claim, and every symbol the library exports is named in exactly
one row. `python scripts/check-docs.py` fails when this table, `gdb.h`,
`API.md` and the library disagree, in either direction.

| Capability | Rust | C ABI symbols | C++, Python, Go |
| --- | --- | --- | --- |
| revision, version and capability list at run time | yes | `gdb_abi_version`, `gdb_abi_version_full`, `gdb_capabilities`, `gdb_version` | yes |
| errors: the calling thread's and the device's own record | yes | `gdb_last_error`, `gdb_device_last_error`, `gdb_string_free` | yes |
| logging: level, sink, `Trace` packet log | yes | `gdb_set_log_level`, `gdb_set_log_sink` | yes |
| session: open, shared open, close, reconnect, auto-reconnect, state, list | yes | `gdb_open`, `gdb_open_shared`, `gdb_open_mock`, `gdb_open_mock_shared`, `gdb_close`, `gdb_reconnect`, `gdb_set_auto_reconnect`, `gdb_device_state`, `gdb_device_free`, `gdb_device_list` | yes |
| mock desktop: monitors, DPI, primary flag, windows, all described by the caller | yes | `gdb_open_mock_layout`, `gdb_open_mock_layout_shared`, `gdb_device_monitors_json` | yes |
| shared ownership and its `stolen_frames` counter | yes | `gdb_stolen_frames` | yes |
| queries: device info, STATUS, STATUS_EXT, SESSION_LIST, barrier, release-all, bootloader | yes | `gdb_device_info`, `gdb_status`, `gdb_status_ext`, `gdb_sessions`, `gdb_barrier`, `gdb_release_all`, `gdb_bootloader` | yes |
| presence: probe, probe-all, observer on a timer | yes | `gdb_probe`, `gdb_probe_all`, `gdb_observer_start`, `gdb_observer_stop`, `gdb_observer_free`, `gdb_observer_info`, `gdb_observer_state`, `gdb_observer_status`, `gdb_observer_barrier`, `gdb_observer_release_all`, `gdb_observer_command` | yes |
| watch: arrivals, removals, bootloader volumes | yes | `gdb_watch_start`, `gdb_watch_stop`, `gdb_watch_free`, `gdb_watch_listing` | yes |
| pointer: relative, raw, absolute, pursuit, path, screen-targeted, containing anchor | yes | `gdb_mouse_move_by`, `gdb_move_raw`, `gdb_mouse_move_to`, `gdb_mouse_move_to_on`, `gdb_mouse_move_to_on_pursuit`, `gdb_mouse_move_path`, `gdb_mouse_move_path_on`, `gdb_monitors_json`, `gdb_platform_prepare` | yes |
| pointer: window-client anchor, window metrics, per-move landing report | yes | `gdb_window_metrics`, `gdb_mouse_move_to_on_report` | yes |
| async moves with retarget and cancel | yes | `gdb_mouse_start_move_to`, `gdb_mouse_start_move_by`, `gdb_mouse_start_move_path`, `gdb_mouse_start_move_to_on`, `gdb_movement_cancel`, `gdb_movement_retarget`, `gdb_movement_is_finished`, `gdb_movement_wait`, `gdb_movement_free` | yes |
| buttons, wheel, drag | yes | `gdb_mouse_button`, `gdb_mouse_wheel`, `gdb_mouse_drag` | yes |
| clicks: aim-and-click, caller-timed hold and gap, jitter | yes | `gdb_mouse_click`, `gdb_mouse_click_timed`, `gdb_mouse_click_at_on`, `gdb_mouse_double_click` | yes |
| keys, long press, chords, sequences, text | yes | `gdb_key_down`, `gdb_key_up`, `gdb_key_press`, `gdb_key_long_press`, `gdb_keys_chord`, `gdb_keys_sequence`, `gdb_type_text` | yes |
| the movement profile the ABI fills | yes | `gdb_profile_default` | yes |
| raw command passthrough | yes | `gdb_command`, `gdb_packet_free` | yes |
| scheduler: submit, absolute due, every, pause, resume, cancel, wait_idle, taken errors | yes | `gdb_scheduler_new`, `gdb_scheduler_free`, `gdb_scheduler_submit`, `gdb_scheduler_submit_at`, `gdb_scheduler_elapsed_ms`, `gdb_scheduler_every`, `gdb_scheduler_cancel`, `gdb_scheduler_pause`, `gdb_scheduler_resume`, `gdb_scheduler_wait_idle`, `gdb_scheduler_take_errors` | yes (C++ has no `submit_at` or `elapsed_ms`) |
| metrics: frame counters, latency percentiles, clamped moves | yes | `gdb_metrics_get`, `gdb_metrics_reset`, `gdb_metrics_clamps` | yes |
| events: state changes and errors | yes | `gdb_set_event_handler` | yes |
| events with a structured JSON payload | yes | `gdb_set_event_handler_json` | yes |
| flash: BOOTSEL volumes, UF2 pack, copy, verify | yes | `gdb_flash_drives`, `gdb_flash_pack`, `gdb_flash_image_stats`, `gdb_flash_write`, `gdb_flash_image_free`, `gdb_flash_verify`, `gdb_flash_build` | yes (build is Rust-only) |
| replay: `Recording`, entry reader and playback | yes | `gdb_replay_open`, `gdb_replay_info`, `gdb_replay_next`, `gdb_replay_play`, `gdb_replay_close`, `gdb_replay_free` | yes (recording is Rust-only) |
| broker client: attach, spawn, the op set, the capability query, typed results | yes | `gdb_attach`, `gdb_remote_spawn`, `gdb_remote_free`, `gdb_remote_info`, `gdb_remote_status`, `gdb_remote_status_ext`, `gdb_remote_sessions`, `gdb_remote_key`, `gdb_remote_mouse_move`, `gdb_remote_move_by`, `gdb_remote_move_raw`, `gdb_remote_mouse_move_to`, `gdb_remote_mouse_move_to_on`, `gdb_remote_caps`, `gdb_remote_mouse_buttons`, `gdb_remote_wheel`, `gdb_remote_barrier`, `gdb_remote_release_all`, `gdb_remote_command`, `gdb_remote_close` | yes |
| broker server: `Server`, `Source`, `run_if_requested` | yes | — | — |
| record a gesture: `Recorder` | yes | — | — |
| stray hook: `set_stray_hook` | yes | — | — |
| custom transports: `over_transport`, `Transport` | yes | — | — |

Every row is implemented once, in `sdk/rust`. A `—` row has no C symbol, so a
binding over `gdb.dll` cannot reach it. Symbol names per layer:
[symbol map](API.md#symbol-map).

Limits, in the order they bite:

- **Building firmware is Rust.** `gdb_flash_build` answers `Unsupported`: it needs
  cargo and a checkout of `firmware/app`, which a library cannot assume. Build
  with your own toolchain, then `gdb_flash_pack` the ELF it produced and
  `gdb_flash_write` it to the drive — packing, copying and verifying are all on
  the C ABI.
- **Recording is Rust.** Reading, inspecting and playing a recording are not:
  `gdb_replay_open` / `_info` / `_next` / `_play` hand the entries over in the
  file's own shape and play them with the SDK's playback. Capturing a gesture
  needs the device handle the recording process already owns, which is why that
  half stays `Recorder`.
- **The broker's server is Rust.** A client attaches from any language, but the
  process that owns the board embeds `gdb::broker`.
- **An attached client sends what the broker answers.** `gdb_remote_caps` is the
  list: today the relative moves, the broker-side absolute move, buttons, wheel,
  keys, the raw command and the queries. Absolute moves need a broker that speaks
  wire revision 2; one from revision 1 answers `Unsupported` for them and for
  `caps` itself, which is why a client that gets no `caps` answer assumes the
  revision-1 set. A drag or a typing loop is several of those ops, so a caller
  that needs them starts them one at a time instead of expecting a host-side
  worker behind the pipe.
- **`set_stray_hook` is Rust.** A shared handle in every other language still
  counts `stolen_frames`; it cannot hand a foreign frame on.
- **Named pipes are Windows-only.** `Server::new`, `Client::connect` and
  `gdb_attach` answer `Unsupported` elsewhere.
- **The mock desktop is one 1920×1080 screen unless you say otherwise.** A mock
  device has no display APIs to read, so a test that wants a second monitor, a
  negative coordinate or a window anchor passes `gdb_open_mock_layout` /
  `Manager::open_mock_layout` a layout; it is fixed for the life of the handle.
- **The Go binding needs Windows and a built DLL**, and has no `StartMovePath`
  where the other three layers do.
- **One reader per IN endpoint.** A second process polling the vendor collection
  takes replies from the first, which is what presence and the broker exist to
  avoid. A program may probe freely, but must not read the IN endpoint itself or
  send `BOOTLOADER` while another one holds the session.
- **Raw passthrough reaches defined opcodes only.** `gdb_command` sends what the
  protocol defines; anything else is `Invalid` before a byte leaves.

## Build and verify

```sh
cargo build -p gdb-ffi --release        # target/release/gdb.dll (or libgdb.so)
cargo test  -p gdb -p gdb-motion        # mock device, no hardware
cargo run   -p gdb --example mock       # one walkthrough of the whole surface
python scripts/check-bindings.py        # every symbol named in all three bindings
python scripts/check-docs.py            # the table, API.md and gdb.h match the library
bash scripts/check-sdk.sh               # Rust, C++, Python and (when found) Go, end to end
```

## Where to go next

- [API.md](API.md) — every symbol, signature, field and error code.
- [docs/architecture.md](../docs/architecture.md) — where the code lives and
  what a change touches.
- [firmware/protocol-v3.md](../firmware/protocol-v3.md) — the wire protocol
  underneath the SDK.
