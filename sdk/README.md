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

The ABI is major 1, minor 9 (`GDB_ABI_VERSION_MINOR`); `gdb_abi_version()`
answers the major at run time, `gdb_abi_version_full()` the major and the minor,
`gdb_capabilities()` the abilities behind them, and `gdb_version()` the SDK
string.

## Choose your handle

| Your program… | Handle | C ABI | Rust | What it costs |
| --- | --- | --- | --- | --- |
| is the only one controlling the board | direct, exclusive | `gdb_open` | `Manager::open` | a reply another process takes is one `Timeout` |
| controls the board while others may command it too | direct, shared | `gdb_open_shared` | `Device::open_shared` | a taken reply is one `Busy` naming the command, and the SDK never reopens the session on its own |
| only displays the board's numbers while another program controls it | presence | `gdb_probe`, `gdb_probe_all`, `gdb_observer_start` | `probe::read_selector`, `probe::all`, `Observer::start` | no session and no packet, so no owner slot is taken; every command on an observer answers `Unsupported` |
| sends input to a board another process owns | attached | `gdb_attach`, `gdb_remote_*` | `broker::Client::connect` | the broker's op set only: no absolute move, drag, typing, chord, scheduler or events |
| runs with no hardware at all | mock | `gdb_open_mock`, `gdb_open_mock_shared` | `Manager::open_mock`, `Device::open_mock_shared` | one 1920×1080 virtual screen and a virtual cursor |

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

| Capability | Rust | C ABI, C++, Python, Go |
| --- | --- | --- |
| session: open, close, reconnect, auto-reconnect, state | yes | yes |
| errors: the calling thread's and the device's own record | yes | yes |
| revision and capability list at run time | yes | yes |
| queries: device info, STATUS, STATUS_EXT, SESSION_LIST | yes | yes |
| shared ownership and its `stolen_frames` counter | yes | yes |
| presence: probe, probe-all, observer on a timer | yes | yes |
| pointer: relative, raw, absolute, pursuit, path, screen-targeted, containing anchor, async with retarget and cancel | yes | yes |
| pointer: window-client anchor, window metrics, platform preparation | yes | yes |
| monitor layout and per-monitor DPI | yes | yes |
| buttons, wheel, drag, keys, long press, chords, sequences, text | yes | yes |
| clicks: aim-and-click, caller-timed hold and gap, jitter | yes | yes |
| scheduler: submit, absolute due, every, pause, resume, cancel, wait_idle, taken errors | yes | yes (C++ has no `submit_at` or `elapsed_ms`) |
| metrics: frame counters, latency percentiles, clamped moves | yes | yes |
| watch: arrivals, removals, bootloader volumes | yes | yes |
| broker client: attach, spawn, the op set, typed results | yes | yes |
| raw command passthrough | yes | yes |
| logging: level, sink, `Trace` packet log | yes | yes |
| events: state changes and errors | yes | yes |
| flash: build, UF2 pack, BOOTSEL copy, verify | yes | — |
| broker server: `Server`, `Source`, `run_if_requested` | yes | — |
| replay: `Recorder`, `Recording` | yes | — |
| stray hook: `set_stray_hook` | yes | — |
| custom transports: `over_transport`, `Transport` | yes | — |

Every row is implemented once, in `sdk/rust`. A `—` row has no C symbol, so a
binding over `gdb.dll` cannot reach it. Symbol names per layer:
[symbol map](API.md#symbol-map).

Limits, in the order they bite:

- **Flashing is Rust.** `gdb.h` has no build, pack, copy or verify call, so a
  binding cannot flash a board: link the Rust crate, or ship a small Rust helper
  that calls `gdb::flash`.
- **The broker's server is Rust.** A client attaches from any language, but the
  process that owns the board embeds `gdb::broker`.
- **Replay is Rust.** A recording is a plain JSON file, so another language can
  read one only by re-implementing the format.
- **`set_stray_hook` is Rust.** A shared handle in every other language still
  counts `stolen_frames`; it cannot hand a foreign frame on.
- **Named pipes are Windows-only.** `Server::new`, `Client::connect` and
  `gdb_attach` answer `Unsupported` elsewhere.
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
bash scripts/check-sdk.sh               # Rust, C++, Python and (when found) Go, end to end
```

## Where to go next

- [API.md](API.md) — every symbol, signature, field and error code.
- [docs/architecture.md](../docs/architecture.md) — where the code lives and
  what a change touches.
- [firmware/protocol-v3.md](../firmware/protocol-v3.md) — the wire protocol
  underneath the SDK.
