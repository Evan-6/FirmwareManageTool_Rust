#!/usr/bin/env python3
"""Verify the exported minimal clients against the wire spec.

Three checks, none of them need a board:
  1. the generated Python client is imported and driven by a fake transport;
  2. the Rust client compiles with `rustc --crate-type lib` (no deps);
  3. the C++ header passes `g++ -fsyntax-only` when a compiler is installed.

The opcode values themselves come from shared/input-protocol/src/v3.rs via
scripts/sync-input-protocol.py, so this script only proves the clients agree
with that file and with each other.
"""
from pathlib import Path
import importlib.util
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
CLIENTS = ROOT / "clients"

failures = []


def check(name, fn):
    try:
        fn()
    except Exception as error:  # noqa: BLE001 - report every failure, keep going
        failures.append(f"{name}: {error}")
        print(f"FAIL {name}: {error}")
    else:
        print(f"ok   {name}")


def load_python_client():
    spec = importlib.util.spec_from_file_location("hid_v3_client", CLIENTS / "python" / "hid_v3.py")
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class FakeTransport:
    def __init__(self, client_module):
        self.module = client_module
        self.written = []
        self.replies = []

    def feature(self):
        report = bytearray(64)
        report[0] = 14
        report[1:5] = b"FMT3"
        report[5] = 3
        report[6] = 2
        report[7:10] = bytes([3, 2, 0])
        report[10:12] = (2000).to_bytes(2, "little")
        report[14] = 1
        report[15:19] = (15).to_bytes(4, "little")
        return bytes(report)

    def write(self, report):
        self.written.append(report)
        opcode = report[2]
        length = report[3]
        payload = report[13 : 13 + length]
        session = int.from_bytes(report[5:9], "little")
        sequence = int.from_bytes(report[9:13], "little")
        if opcode & 0x80 or opcode in (self.module.KEY, self.module.SNAPSHOT,
                                       self.module.MOVE, self.module.BUTTONS,
                                       self.module.WHEEL, self.module.HEARTBEAT):
            return
        reply = bytearray(64)
        reply[0] = 13
        reply[1] = 3
        reply[2] = opcode | 0x80
        reply[5:9] = session.to_bytes(4, "little")
        reply[9:13] = sequence.to_bytes(4, "little")
        if opcode == self.module.STATUS:
            body = bytearray(51)
            body[1:5] = (sequence).to_bytes(4, "little")
            body[5:9] = (sequence).to_bytes(4, "little")
            body[20:51] = bytes([1] + [0] * 30)
        elif opcode == self.module.STATUS_EXT:
            body = bytearray(23)
            body[1:5] = (1234).to_bytes(4, "little")
            body[12:16] = (0xFFFFFFFF).to_bytes(4, "little")
            body[19:23] = (0xDEADBEEF).to_bytes(4, "little")
        elif opcode == self.module.SESSION_LIST:
            body = bytearray(48)
            body[0:2] = (37).to_bytes(2, "little")
            body[2:4] = (1500).to_bytes(2, "little")
            body[4] = 2
            body[5] = 1
        else:
            body = bytearray(1)
        reply[3] = len(body)
        reply[13 : 13 + len(body)] = body
        self.replies.append(bytes(reply))

    def read(self, timeout_ms):
        return self.replies.pop(0) if self.replies else None


def python_client_test():
    module = load_python_client()
    transport = FakeTransport(module)
    client = module.Client(transport, session=0x1234)
    feature = client.verify()
    assert feature.board == 2, feature.board
    assert feature.firmware == "3.2.0", feature.firmware
    assert feature.lease_ms == 2000, feature.lease_ms
    assert feature.capabilities == 15, feature.capabilities
    assert feature.supports_sessions() and feature.supports_mouse()

    client.open()
    status = client.status()
    assert status.received == 2, status.received
    ext = client.status_ext()
    assert ext.uptime_ms == 1234 and ext.build == 0xDEADBEEF, ext.__dict__
    assert ext.last_fault_age_ms is None
    owners = client.sessions()
    assert owners[0].id == 37 and owners[0].held == 2, owners[0].__dict__

    client.key(7, 4, True)
    client.mouse_move(-10, 20)
    client.buttons(1)
    client.wheel(0, -1)
    client.snapshot(bytes(31))
    client.heartbeat(10)
    client.close()

    frames = transport.written
    assert frames[0][0] == 12 and frames[0][1] == 3, "report id/version"
    assert frames[0][2] == module.OPEN and frames[0][3] == 0, "OPEN frame"
    assert int.from_bytes(frames[0][5:9], "little") == 0x1234
    sequences = [int.from_bytes(f[9:13], "little") for f in frames]
    assert sequences == list(range(1, len(frames) + 1)), sequences
    key_frame = next(f for f in frames if f[2] == module.KEY)
    assert key_frame[3] == 5 and key_frame[13:18] == bytes([7, 0, 4, 0, 1]), key_frame[13:18]
    assert frames[-1][2] == module.CLOSE, "last command is CLOSE"

    # A device event must surface as a failure, not as a silent timeout.
    event = bytearray(64)
    event[0] = 13
    event[1] = 3
    event[2] = module.EVENT
    event[3] = 1
    event[5:9] = (0x1234).to_bytes(4, "little")
    event[13] = 5
    transport.replies.append(bytes(event))
    try:
        client.status()
    except module.Error as error:
        assert "lease" in str(error), error
    else:
        raise AssertionError("EVENT was not raised")


def rust_client_compiles():
    compiler = shutil.which("rustc")
    if not compiler:
        raise RuntimeError("rustc not on PATH")
    with tempfile.TemporaryDirectory() as out:
        result = subprocess.run(
            [compiler, "--edition", "2021", "--crate-type", "lib", "-D", "warnings",
             str(CLIENTS / "rust" / "hid_v3.rs"), "-o", str(Path(out) / "libhid_v3.rlib")],
            capture_output=True, text=True,
        )
    if result.returncode != 0:
        raise RuntimeError(result.stderr.strip())


def cpp_client_parses():
    compiler = shutil.which("g++")
    if not compiler:
        raise RuntimeError("g++ not on PATH (C++ client not compiled)")
    result = subprocess.run(
        [compiler, "-std=c++17", "-fsyntax-only", "-x", "c++-header",
         str(CLIENTS / "cpp" / "hid_v3.hpp")],
        capture_output=True, text=True,
    )
    if result.returncode != 0:
        raise RuntimeError(result.stderr.strip())


if __name__ == "__main__":
    check("python client behaviour", python_client_test)
    check("rust client compiles", rust_client_compiles)
    check("c++ client parses", cpp_client_parses)
    if failures:
        print("\n" + "\n".join(failures), file=sys.stderr)
        sys.exit(1)
    print("clients ok")
