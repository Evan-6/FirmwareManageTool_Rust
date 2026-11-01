#!/usr/bin/env bash
# Verifies the GDBOX SDK in every binding that can run on this machine.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

echo "== rust =="
cargo test -q -p gdb-motion -p gdb

echo "== c abi =="
cargo build -q -p gdb-ffi
cargo test -q -p gdb-ffi

echo "== abi surface =="
python scripts/check-bindings.py

lib="$root/target/debug/gdb.dll"
[ -f "$lib" ] || lib="$root/target/debug/libgdb.so"
# Native tools (LoadLibrary, ctypes) do not understand MSYS /c/... paths.
lib_env="$lib"
if command -v cygpath >/dev/null 2>&1; then
    lib_env="$(cygpath -w "$lib")"
fi

echo "== python =="
GDBOX_LIBRARY="$lib_env" python sdk/python/test_gdb.py


echo "== c++ =="
if command -v g++ >/dev/null 2>&1; then
    out="target/sdk-check/cpp_mock.exe"
    mkdir -p "$(dirname "$out")"
    g++ -std=c++17 -Wall -Wextra \
        -I sdk/ffi/include -I sdk/cpp \
        sdk/cpp/examples/mock.cpp -o "$out"
    GDBOX_LIBRARY="$lib_env" "$out"

else
    echo "skip: g++ not found"
fi

echo "== go =="
# Go is often installed outside PATH on this machine (the SDK tarball layout).
if ! command -v go >/dev/null 2>&1; then
    for candidate in "$HOME"/sdk/go*/go/bin/go "$HOME"/scoop/apps/go/current/bin/go.exe         "/c/Program Files/Go/bin/go.exe"; do
        if [ -x "$candidate" ]; then
            PATH="$(dirname "$candidate"):$PATH"
            break
        fi
    done
fi
if command -v go >/dev/null 2>&1; then
    # vet first: consumers vendor the binding into their own module, where
    # `go vet ./...` is part of the gate and flags every uintptr -> pointer cast.
    (cd sdk/go && go vet ./... && GDBOX_LIBRARY="$lib_env" go test ./... )
else
    echo "skip: go not found"
fi

echo "sdk ok"
