#!/usr/bin/env bash
# Build the pure-Rust XIAO RP2040 firmware and package it as UF2.
#
# Nothing here needs the Arduino toolchain: the boot2 stage comes from
# embassy-rp, the report descriptors come from scripts/gen-usb-descriptors.py,
# and UF2 packaging is done by scripts/elf2uf2.py.
set -euo pipefail

# Windows-native tools (python, cargo) do not understand MSYS paths, so resolve
# the root to a drive-letter path when `pwd -W` is available.
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && { pwd -W 2>/dev/null || pwd; })"
app="$root/firmware/app"
target=thumbv6m-none-eabi
profile=release
python_bin="${PYTHON:-$(command -v python3 || command -v python)}"

"$python_bin" "$root/scripts/gen-usb-descriptors.py" --check
rustup target list --installed | grep -qx "$target" || rustup target add "$target"

cargo build --manifest-path "$app/Cargo.toml" --$profile --target "$target" --features hardware

elf="$app/target/$target/$profile/xiao-firmware"
uf2="$app/target/$target/$profile/xiao-firmware.uf2"
"$python_bin" "$root/scripts/elf2uf2.py" "$elf" "$uf2"
"$python_bin" "$root/scripts/elf-size.py" "$elf"
echo "uf2: $uf2"
