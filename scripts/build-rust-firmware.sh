#!/usr/bin/env bash
# Build the pure-Rust XIAO RP2040 firmware and package it as UF2.
#
# Nothing here needs the Arduino toolchain: boot2 comes from embassy-rp, the
# report descriptors are pinned by firmware/app/tests/descriptors.rs, and UF2
# packaging is the same Rust code the GUI uses (`uf2pack`).
set -euo pipefail

# Windows-native tools (python, cargo) do not understand MSYS paths, so resolve
# the root to a drive-letter path when `pwd -W` is available.
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && { pwd -W 2>/dev/null || pwd; })"
app="$root/firmware/app"
target=thumbv6m-none-eabi
profile=release
python_bin=""
for candidate in "${PYTHON:-}" python3 python; do
    [ -n "$candidate" ] || continue
    # The Microsoft Store `python3` shim exits 0 without running anything.
    if "$candidate" -c 'import sys; sys.exit(0)' >/dev/null 2>&1; then
        python_bin="$candidate"
        break
    fi
done
[ -n "$python_bin" ] || echo 'no working python: 跳過 footprint 報告' >&2

rustup target list --installed | grep -qx "$target" || rustup target add "$target"

cargo build --manifest-path "$app/Cargo.toml" --$profile --target "$target" --features hardware

elf="$app/target/$target/$profile/xiao-firmware"
uf2="$app/target/$target/$profile/xiao-firmware.uf2"
cargo run --quiet --manifest-path "$root/Cargo.toml" --$profile --no-default-features \
    --bin uf2pack -- "$elf" "$uf2"
if [ -n "$python_bin" ]; then
    "$python_bin" "$root/scripts/elf-size.py" "$elf"
fi
echo "uf2: $uf2"
