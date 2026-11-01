# AGENTS.md

Windows desktop tool + host SDK for one vendor-HID device: a Seeed XIAO RP2040
running Rust firmware (`03F0:0024`, usage `FF60:0061`, protocol 3.2.0).

## Layout

```
firmware/               RP2040 firmware (embassy; firmware/app is its own workspace)
firmware/protocol-v3.md wire protocol: opcodes, payloads, lease rules
shared/input-protocol   protocol constants, codecs, canonical client, test fake
sdk/motion              gdb-motion: pure movement maths, no I/O
sdk/rust                gdb: device, input, scheduler, broker, watch, metrics, replay, flash
sdk/ffi                 C ABI (include/gdb.h) + cdylib
sdk/cpp|python|go       thin bindings over the C ABI: names only, no logic
clients/                generated per-language protocol clients (hid_v3.*)
docs/                   architecture: where things live and what a change touches
examples/               hardware smoke test for the host path the GUI uses
src/                    tool library: session adapter, flashing, latency, uf2, worker
src-tauri/              IPC shell (commands in generate_handler!, events in UiEvent)
ui/                     React 19 + Vite, five pages
scripts/                preflight, check-sdk, sync-input-protocol, cdp-check, packaging
```

Dependencies point one way: `ui → src-tauri → src → gdb → input-protocol`.
The SDK never depends on the tool.

## Build and verify

```sh
cargo fmt --all -- --check
cargo clippy --workspace --exclude firmware-manage-tool-app --all-targets -- -D warnings
cargo test  --workspace --exclude firmware-manage-tool-app --all-targets
python scripts/sync-input-protocol.py --check
python scripts/check-clients.py
bash scripts/check-sdk.sh
node scripts/check-ipc.mjs
npm --prefix ui ci && npm --prefix ui run build      # required before src-tauri builds
bash scripts/preflight.sh                            # all of the above, in order
```

Host commands never pass `--target`: `target/debug` and `target/release` are it.

## Rules

- One implementation per behaviour: the Rust crate `gdb` is it. The protocol
  lives in `shared/input-protocol` and the SDK; the tool adapts, it does not
  reimplement. A feature lands in `sdk/rust`, and reaches another language
  through the C ABI — never as a second implementation.
- `Device::command` is the only raw escape hatch; it skips SDK bookkeeping
  (held buttons, mock cursor), so prefer the typed methods.
- The lease renews only on OPEN, HEARTBEAT, RELEASE_ALL, BOOTLOADER and valid
  input; CLOSE, STATUS, BARRIER, STATUS_EXT, SESSION_LIST and EVENT do not. Never
  treat "some call succeeded" as a heartbeat.
- Keep the IPC contract stable: `node scripts/check-ipc.mjs` compares
  `generate_handler![...]` / `enum UiEvent` with `ui/src/lib/ipc.ts` and
  `types.ts` in both directions.
- Documents describe the current state only. No changelogs, no migration notes,
  no "previously we…" — git holds the history.
- Chinese for user-facing text and comments, English for SDK docs and API names.

## Traps worth knowing

- Board firmware and tool must agree on the opcode table; a board flashed from an
  older commit answers the same bytes with different meanings.
- Replies carry no address, so only one reader can take one: a second reader
  steals it. `Ownership::Shared` expects that (one `Busy`, no reconnect) and
  `probe` never touches the queue.
- `ui/dist` is embedded at compile time; rebuild the shell after touching `ui/`.
- FFI pointers are the caller's contract; strings returned by the library must be
  freed with `gdb_string_free`.
- Windows paths: native tools (cargo/python/node) do not understand `/d/...`.
