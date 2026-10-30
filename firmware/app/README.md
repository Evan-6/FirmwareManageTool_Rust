# `xiao-firmware` — pure-Rust XIAO RP2040 firmware (3.B)

A second implementation of the same device, with no Arduino core, no TinyUSB and
no C++: the protocol engine, the runtime loop, the USB device stack and the boot
path are all Rust. The C++ sketch under `firmware/boards/rp2040/` stays as the
reference oracle until this build passes hardware regression.

```
USB host
   │
   ├─ interface 0: input HID  (NKRO id 2, consumer id 5, relative mouse id 3)
   └─ interface 1: vendor HID (OUT 12, IN 13, Feature 14)
        │
   embassy-usb (HID class, 64-byte reports, 1 ms poll)
        │
   Runtime::step  (src/runtime.rs, transport-agnostic, six stages)
        │
   firmware-engine (no_std, no heap, fixed capacity)
```

## Build

```bash
rustup target add thumbv6m-none-eabi
bash scripts/build-rust-firmware.sh
```

That runs `scripts/gen-usb-descriptors.py --check`, builds the crate for
`thumbv6m-none-eabi`, packages `xiao-firmware.uf2` with `scripts/elf2uf2.py` and
prints the footprint with `scripts/elf-size.py`. Nothing in that chain needs
`arduino-cli`, `arm-none-eabi-gcc` or `picotool`.

Measured on the PoC (release, no LED, single core):

| image | flash | RAM |
| --- | --- | --- |
| C++ sketch (arduino-pico 6.2.0, TinyUSB) | 90,792 B | 29,712 B |
| Rust `xiao-firmware` | 28,008 B | 17,804 B |

RAM is dominated by the engine (`InputEngine` 12,564 B) and the 32-slot RX queue
(2,016 B); the flash difference is the Arduino core and TinyUSB that are no
longer linked in.

Flashing: put the board in BOOTSEL mode and copy the UF2 to the `RPI-RP2` drive,
or `cargo run --release --features hardware` (the `runner` in
`.cargo/config.toml` is `probe-rs run --chip RP2040`).

## What is verified

- `cargo test` — 5 adapter tests in `tests/runtime.rs` against a mock transport:
  RX budget of four packets per pass, rejected input report → HID fault, bus
  reset → failsafe event + stale RX discarded, framing fault propagating its
  error count, BOOTLOADER reply → 120 ms gate → reboot request.
- `cargo clippy --all-targets -D warnings` on the host and for
  `thumbv6m-none-eabi`, `cargo fmt --check`.
- `cargo build --release --target thumbv6m-none-eabi --features hardware` links
  with boot2 at `0x10000000` (`.boot2` from embassy-rp, `BOOT_LOADER_W25Q080`)
  and produces a 111-block UF2 whose first block targets `0x10000000`.
- Report descriptors: `scripts/gen-usb-descriptors.py --check` compares the Rust
  arrays against `UsbDescriptors.cpp` and the frozen golden bytes.
- Engine behaviour: `firmware-engine` carries the 37-line A/B trace that matches
  the C++ engine byte for byte.

## What is NOT verified

- **No hardware run yet.** USB enumeration, both interfaces, the 1 ms IN
  polling, Feature 14 reads, the HID LED `set_report` path, BOOTSEL reboot,
  and Windows-side latency are all untested on a real XIAO RP2040.
- NeoPixel status indication is not wired: the engine produces `LedState`
  transitions, but nothing drives the WS2812 yet (the C++ build uses core 1).
- All work runs on core 0. The C++ runtime splits LED rendering onto core 1;
  that is a deliberate deferral, not an oversight.
- The serial number now comes from the flash unique id via
  `Flash::blocking_unique_id`; the C++ build uses the same source, but the two
  hashes have not been compared on one board.

## Layout

| file | role |
| --- | --- |
| `src/runtime.rs` | `Transport` trait + `Runtime::step`: the six-stage loop, transport-agnostic |
| `src/usb_descriptors.rs` | generated report descriptors (do not edit) |
| `src/main.rs` | embassy-rp USB device, HID interfaces, `Ticker`-driven engine loop |
| `../engine/` | protocol engine shared with the host tests |
| `memory.x`, `build.rs` | RP2040 linker layout (boot2, 2 MiB flash, 264 KiB RAM) |
