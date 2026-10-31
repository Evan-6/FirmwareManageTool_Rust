# `xiao-firmware` — pure-Rust XIAO RP2040 firmware (3.B)

The only implementation of this device on this branch: no Arduino core, no
TinyUSB, no C++. The protocol engine, the runtime loop, the USB device stack and
the boot path are all Rust. The C++ sketch that used to live in
`firmware/boards/rp2040/` was deleted once this build passed the hardware
regression; its behaviour survives as the recorded trace golden.

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

That builds the crate for `thumbv6m-none-eabi`, packages `xiao-firmware.uf2`
with `uf2pack` (the same Rust code the GUI runs) and prints the footprint with
`scripts/elf-size.py`. Nothing in that chain needs `arduino-cli`,
`arm-none-eabi-gcc` or `picotool`.

Measured on the PoC (release, no LED, single core):

| image | flash | RAM |
| --- | --- | --- |
| C++ sketch (arduino-pico 6.2.0, TinyUSB — deleted) | 90,792 B | 29,712 B |
| Rust `xiao-firmware` | 33,488 B | 30,796 B |

RAM is dominated by the engine (`InputEngine` 12,564 B) and the 32-slot RX queue
(2,016 B); the flash difference is the Arduino core and TinyUSB that are no
longer linked in.

Flashing: put the board in BOOTSEL mode and copy the UF2 to the `RPI-RP2` drive,
or `cargo run --release --features hardware` (the `runner` in
`.cargo/config.toml` is `probe-rs run --chip RP2040`).

## Hardware test

`examples/probe.rs` drives the same host path the GUI uses, and
`examples/boot.rs` can reboot the board with raw vendor reports when Feature 14
is broken:

```bash
cargo run --release --no-default-features --example probe                # Feature 14 → OPEN → STATUS → HEARTBEAT → BARRIER
cargo run --release --no-default-features --example probe -- --keys      # inject 'a', watch the keyboard report complete
cargo run --release --no-default-features --example probe -- --lease     # idle past lease_ms, device must drop the session
cargo run --release --no-default-features --example probe -- --bootloader  # reboot into BOOTSEL and check it leaves the bus
cargo run --release --no-default-features --example boot                 # fallback BOOTSEL without touching Feature 14
```

Results on a XIAO RP2040 (serial `4250304B38353817`, 2026-10-31): all four modes
pass. `--keys` shows `keys="a"`, `completed` advancing 2 → 4 and `p=1`, then
`keys=""` and `p=0` after release — i.e. the keyboard report actually completes
on the bus, not merely "accepted". `--lease` reports fault `[5]` after 2000 ms
and the host session is invalidated. `--bootloader` produces the `RPI-RP2` drive
within a second, and the reflashed image enumerates again with `fs=0`.

Windows refuses a raw `ReadFile` on the keyboard collection (`ERROR_ACCESS_DENIED`)
because it owns that collection, so the input path is proven from the device side:
STATUS counters plus report completion. Typing into a focused window is the only
end-to-end check of the OS side and is not automated yet.

## What is verified

- `cargo test` — 5 adapter tests in `tests/runtime.rs` against a mock transport:
  RX budget of four packets per pass, rejected input report → HID fault, bus
  reset → failsafe event + stale RX discarded, framing fault propagating its
  error count, BOOTLOADER reply → 120 ms gate → reboot request.
- `cargo clippy --all-targets -D warnings` on the host and for
  `thumbv6m-none-eabi`, `cargo fmt --check`.
- `cargo build --release --target thumbv6m-none-eabi --features hardware` links
  with boot2 at `0x10000000` (`.boot2` from embassy-rp, `BOOT_LOADER_W25Q080`)
  and produces a 110-block UF2, every block 256-byte aligned, first at
  `0x10000000`.
- Report descriptors: `tests/descriptors.rs` pins the frozen bytes with a hash, so an
  edit that changes what Windows sees fails the build instead of drifting.
- Engine behaviour: `firmware-engine` carries the 37-line A/B trace that matches
  the C++ engine byte for byte.
- On hardware: enumeration (both collections, same VID/PID/serial as the C++
  build), Feature 14, OPEN/STATUS/HEARTBEAT/BARRIER, key injection with report
  completion, lease expiry, BOOTSEL reboot, and reflashing by copying the UF2.

## What is NOT verified

- HID LED `set_report` (lock keys) is accepted and ignored, but nothing observes
  whether Windows actually sends those reports.
- Windows-side latency has not been measured against the C++ build.
- NeoPixel status indication is not wired: the engine produces `LedState`
  transitions, but nothing drives the WS2812 yet (the C++ build uses core 1).
- All work runs on core 0. The C++ runtime splits LED rendering onto core 1;
  that is a deliberate deferral, not an oversight.
- The serial number comes from the flash unique id via
  `Flash::blocking_unique_id`; it matches the C++ build on this board
  (`4250304B38353817`), but that was a single sample.

## Layout

| file | role |
| --- | --- |
| `src/runtime.rs` | `Transport` trait + `Runtime::step`: the six-stage loop, transport-agnostic |
| `src/usb_descriptors.rs` | generated report descriptors (do not edit) |
| `src/main.rs` | embassy-rp USB device, HID interfaces, `Ticker`-driven engine loop |
| `../engine/` | protocol engine shared with the host tests |
| `memory.x`, `build.rs` | RP2040 linker layout (boot2, 2 MiB flash, 264 KiB RAM) |
