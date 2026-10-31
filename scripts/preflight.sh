#!/usr/bin/env bash
# Everything that must pass before a commit. The Tauri shell embeds ui/dist, so
# the frontend is built first; skipping that makes every later cargo command fail
# on a fresh clone.
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
node scripts/check-ipc.mjs
echo "preflight ok"
