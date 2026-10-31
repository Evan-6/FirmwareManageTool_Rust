#!/usr/bin/env bash
# Everything that must pass before a commit. The Tauri shell embeds ui/dist, so
# the frontend is built first; skipping that makes every later cargo command fail
# on a fresh clone.
#
# Do NOT pass --target here. On Windows the host already IS
# x86_64-pc-windows-msvc, and cargo keeps host-only units (build scripts, proc
# macros) in target/<profile> while target units go to target/<triple>/<profile> —
# so passing the host triple builds every dependency twice and that is how target/
# reached 17 GB. One convention, one tree: plain cargo, target/debug + target/release.
set -euo pipefail
cd "$(dirname "$0")/.."

if [ ! -d ui/node_modules ]; then
  npm --prefix ui ci
fi
npm --prefix ui run build

cargo fmt --all -- --check
cargo clippy --workspace --exclude firmware-manage-tool-app --all-targets -- -D warnings
cargo clippy -p firmware-manage-tool-app -- -D warnings
cargo test --workspace --exclude firmware-manage-tool-app --all-targets
cargo test --manifest-path firmware/app/Cargo.toml
cargo clippy --manifest-path firmware/app/Cargo.toml --target thumbv6m-none-eabi --features hardware -- -D warnings
python scripts/sync-input-protocol.py --check
python scripts/check-clients.py
bash scripts/check-sdk.sh
node scripts/check-ipc.mjs
echo "preflight ok"
