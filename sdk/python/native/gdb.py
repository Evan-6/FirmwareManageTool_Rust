"""GDBOX host SDK for Python — native implementation.

Speaks vendor HID protocol 3 straight to the device: ``hid.dll`` through
``ctypes``, no ``gdb.dll`` and no C ABI. The public surface mirrors
``sdk/python/gdb.py`` (the ctypes binding) so either module can be swapped for
the other; only the transport and the query objects differ (queries here are
typed instead of JSON dicts, and also answer ``["camelCaseKey"]``).

    import gdb
    with gdb.Device.open_mock() as device:
        print(device.info)
        device.keyboard.type_text("hello")
        device.mouse.move_to(200, 300, duration_ms=250)

No hardware is needed for ``Device.open_mock()``: ``MockTransport`` speaks the
same protocol in memory, including the error paths.

Logging is the SDK's, not a second reply collector: ``set_log_level`` (0 off …
5 trace) and ``set_log_sink(fn)`` are process-wide, Trace lines are the same
``out opcode=… sequence=… payload=[…]`` text the Rust SDK writes, and
``set_event_handler`` receives ``EventKind.STATE``/``EventKind.ERROR``.

Firmware build and flashing lives on ``gdb.flash`` (the Rust ``gdb::flash``)::

    image = gdb.flash.build()                      # cargo → ELF → UF2
    outcome = gdb.flash.push(device, image)        # BOOTSEL → copy → verify
    packed = gdb.flash.uf2.pack(elf_bytes)         # the packer on its own

A transport is anything with three methods (see ``clients/python/hid_v3.py``)::

    class MyTransport:
        def feature(self) -> bytes | None: ...      # Feature 14, id in byte 0
        def write(self, report: bytes) -> None: ... # 64-byte report, id 12
        def read(self, timeout_ms: int) -> bytes | None: ...  # report, id 13
"""

from __future__ import annotations

import ctypes
import heapq
import itertools
from collections import deque
import math
import os
import queue
import shutil
import struct
import subprocess
import sys
import threading
import time
from dataclasses import dataclass
from typing import Callable, Sequence

__all__ = [
    "GdbError",
    "ErrorCode",
    "ConnectionState",
    "Button",
    "Ease",
    "Level",
    "Profile",
    "KeyStep",
    "Point",
    "Node",
    "Screen",
    "Status",
    "StatusExt",
    "SessionInfo",
    "InputState",
    "Capabilities",
    "Packet",
    "Session",
    "Transport",
    "MockTransport",
    "HidTransport",
    "Movement",
    "Mouse",
    "Keyboard",
    "Scheduler",
    "Device",
    "EventKind",
    "flash",
    "Reporter",
    "SplitMix64",
    "split_delta",
    "trajectory_with",
    "strokes",
    "error_from_result",
    "encode_packet",
    "decode_packet",
    "interface_path",
    "list_devices",
    "set_log_level",
    "set_log_sink",
]

# --- protocol 3 (shared/input-protocol/src/v3.rs) --------------------------

VENDOR_ID = 0x03F0
PRODUCT_ID = 0x0024
USAGE_PAGE = 0xFF60
USAGE = 0x61

PROTOCOL_VERSION = 3
OUT_ID = 12
IN_ID = 13
FEATURE_ID = 14
REPORT_SIZE = 64
MAX_PAYLOAD = 51
HEADER = 13

OPEN = 0x01
CLOSE = 0x02
HEARTBEAT = 0x03
BARRIER = 0x04
RELEASE_ALL = 0x05
STATUS = 0x06
STATUS_EXT = 0x07
SESSION_LIST = 0x08
BOOTLOADER = 0x09
KEY = 0x10
SNAPSHOT = 0x11
MOUSE_MOVE = 0x12
MOUSE_BUTTONS = 0x13
WHEEL = 0x14
EVENT = 0x7F

STATUS_BYTES = 51
STATUS_EXT_BYTES = 23
SESSION_ENTRY = 6
SESSION_SLOTS = 8
SESSION_LIST_BYTES = SESSION_ENTRY * SESSION_SLOTS
SNAPSHOT_BYTES = 31

#: Consumer page usages, index = bit in the snapshot consumer byte.
CONSUMERS = (0xE2, 0xE9, 0xEA, 0xCD, 0xB5, 0xB6, 0xB7)

#: Keyboard usages of page 7 accepted by the firmware (``keys.json``).
_KEY_RANGES = (
    (4, 39),
    (40, 44),
    (45, 49),
    (51, 57),
    (58, 69),
    (70, 78),
    (79, 82),
    (83, 100),
    (101, 101),
    (104, 115),
    (135, 139),
    (224, 231),
)
KEYBOARD_USAGES = frozenset(
    usage for first, last in _KEY_RANGES for usage in range(first, last + 1)
)
KEY_CATALOG = frozenset({(7, usage) for usage in KEYBOARD_USAGES}) | frozenset(
    {(12, usage) for usage in CONSUMERS}
)

REPLY_OPCODES = frozenset(
    {OPEN, CLOSE, STATUS, BARRIER, RELEASE_ALL, STATUS_EXT, SESSION_LIST, BOOTLOADER}
)
#: STATUS and BARRIER do not renew the device-side lease (protocol-v3.md:120).
NON_RENEWING = frozenset({STATUS, BARRIER})

HEARTBEAT_MS = 500
LEASE_MS = 2000

HID_INTERFACE_GUID = "{4d1e55b2-f16f-11cf-88cb-001111000030}"

Point = tuple[int, int]


# --- errors (the numeric values are the C ABI enum) ------------------------


class ErrorCode:
    OK = 0
    INVALID = 1
    TIMEOUT = 2
    DISCONNECTED = 3
    BUSY = 4
    LEASE = 5
    FRAMING = 6
    OVERFLOW = 7
    UNSUPPORTED = 8
    CANCELLED = 9
    IO = 10

    NAMES = {
        0: "OK",
        1: "INVALID",
        2: "TIMEOUT",
        3: "DISCONNECTED",
        4: "BUSY",
        5: "LEASE",
        6: "FRAMING",
        7: "OVERFLOW",
        8: "UNSUPPORTED",
        9: "CANCELLED",
        10: "IO",
    }

    MESSAGES = {
        0: "ok",
        1: "invalid argument",
        2: "device did not answer in time",
        3: "device disconnected",
        4: "device busy: same session id or all eight owners taken",
        5: "session lease expired",
        6: "device rejected the frame",
        7: "device queue overflow or stale output",
        8: "not supported on this platform or firmware",
        9: "cancelled",
        10: "HID transfer failed",
    }


class GdbError(Exception):
    """Raised for every non-zero ErrorCode."""

    def __init__(self, code: int, message: str = ""):
        self.code = int(code)
        self.name = ErrorCode.NAMES.get(self.code, str(self.code))
        self.message = message
        super().__init__(f"{self.name}: {message}" if message else self.name)

    def __reduce__(self):  # keeps copy/pickle of exceptions working
        return (GdbError, (self.code, self.message))


_RESULT_CODES = {
    0: ErrorCode.OK,
    1: ErrorCode.FRAMING,
    2: ErrorCode.BUSY,
    3: ErrorCode.INVALID,
    5: ErrorCode.LEASE,
    6: ErrorCode.DISCONNECTED,
    7: ErrorCode.DISCONNECTED,
    8: ErrorCode.OVERFLOW,
    9: ErrorCode.FRAMING,
    10: ErrorCode.FRAMING,
}

_RESULT_MESSAGES = {
    1: "device reported a framing error",
    2: ErrorCode.MESSAGES[4],
    3: "device rejected the input",
    5: ErrorCode.MESSAGES[5],
    6: "device failed to send the HID report",
    7: ErrorCode.MESSAGES[3],
    8: ErrorCode.MESSAGES[7],
    9: "duplicate sequence",
    10: "sequence gap",
}


def error_from_result(code: int) -> GdbError:
    """Maps the protocol ``result`` byte onto the SDK code space."""
    mapped = _RESULT_CODES.get(code, ErrorCode.IO)
    message = _RESULT_MESSAGES.get(code, f"device error code {code}")
    return GdbError(mapped, message)


class ConnectionState:
    DISCONNECTED = 0
    CONNECTING = 1
    CONNECTED = 2
    ERROR = 3


class EventKind:
    """First argument of a ``set_event_handler`` callback.

    ``STATE`` carries a ``ConnectionState`` in ``code`` and an empty message;
    ``ERROR`` carries an ``ErrorCode`` and the SDK's message. The numbers are the
    C ABI's ``gdb_event_kind``; ``STATUS`` is reserved there and not delivered.
    """

    STATE = 0
    STATUS = 1
    ERROR = 2


class Button:
    LEFT = 1
    RIGHT = 2
    MIDDLE = 3
    BACK = 4
    FORWARD = 5


class Ease:
    JERK = 0
    IN_OUT = 1
    LINEAR = 2
    IN = 3
    OUT = 4
    CUBIC_BEZIER = 5


class Level:
    OFF = 0
    ERROR = 1
    WARN = 2
    INFO = 3
    DEBUG = 4
    TRACE = 5


# --- logging ---------------------------------------------------------------

_LOG_LEVEL = Level.OFF
_LOG_SINK = None
_LOG_LOCK = threading.Lock()


def set_log_level(level: int) -> None:
    """0 ``Off`` … 5 ``Trace``, the C ABI's ``gdb_set_log_level`` values."""
    level = int(level)
    if not Level.OFF <= level <= Level.TRACE:
        raise GdbError(ErrorCode.INVALID, "level must be 0 (off) to 5 (trace)")
    global _LOG_LEVEL
    with _LOG_LOCK:
        _LOG_LEVEL = level


def log_level() -> int:
    """The process-wide level in force."""
    with _LOG_LOCK:
        return _LOG_LEVEL


def set_log_sink(callback: Callable[[int, str], None] | None) -> None:
    """Process-wide sink, called as ``callback(level, message)``."""
    global _LOG_SINK
    with _LOG_LOCK:
        _LOG_SINK = callback


def _log(level: int, message: str) -> None:
    if level > _LOG_LEVEL:
        return
    sink = _LOG_SINK
    if sink is not None:
        try:
            sink(level, message)
        except Exception:  # a broken sink must not break the SDK
            pass


def _enabled(level: int) -> bool:
    """Same gate as ``gdb::log::enabled``: ask before formatting."""
    return level <= _LOG_LEVEL


def _trace(message: str) -> None:
    if _enabled(Level.TRACE):
        _log(Level.TRACE, message)


def _payload_debug(payload: bytes) -> str:
    """Rust's ``{:?}`` for a byte vector: ``[1, 2, 3]``, and ``[]`` when empty."""
    if not payload:
        return "[]"
    return "[" + ", ".join(str(byte) for byte in payload) + "]"


# --- wire format -----------------------------------------------------------


@dataclass(frozen=True)
class Packet:
    opcode: int
    session: int
    sequence: int
    payload: bytes = b""
    flags: int = 0


def encode_packet(
    opcode: int, session: int, sequence: int, payload: bytes = b"", report_id: int = OUT_ID
) -> bytes:
    """One 64-byte report: ``version, opcode, length, flags, session u32, sequence u32``."""
    payload = bytes(payload)
    if report_id not in (OUT_ID, IN_ID):
        raise GdbError(ErrorCode.INVALID, "report id must be 12 or 13")
    if len(payload) > MAX_PAYLOAD:
        raise GdbError(ErrorCode.INVALID, "payload is at most 51 bytes")
    report = bytearray(REPORT_SIZE)
    report[0] = report_id
    report[1] = PROTOCOL_VERSION
    report[2] = opcode & 0xFF
    report[3] = len(payload)
    report[4] = 0
    report[5:9] = struct.pack("<I", session & 0xFFFFFFFF)
    report[9:13] = struct.pack("<I", sequence & 0xFFFFFFFF)
    report[13 : 13 + len(payload)] = payload
    return bytes(report)


def decode_packet(report: bytes, report_id: int = IN_ID) -> Packet:
    if len(report) != REPORT_SIZE or report[0] != report_id:
        raise GdbError(ErrorCode.FRAMING, "invalid v3 report id or length")
    if report[1] != PROTOCOL_VERSION or report[4] != 0:
        raise GdbError(ErrorCode.FRAMING, "invalid v3 version or flags")
    length = report[3]
    if length > MAX_PAYLOAD:
        raise GdbError(ErrorCode.FRAMING, "invalid v3 payload length")
    if any(report[HEADER + length :]):
        raise GdbError(ErrorCode.FRAMING, "invalid v3 padding")
    return Packet(
        opcode=report[2],
        session=struct.unpack_from("<I", report, 5)[0],
        sequence=struct.unpack_from("<I", report, 9)[0],
        payload=bytes(report[13 : 13 + length]),
    )


def _u16(data: bytes, offset: int) -> int:
    return struct.unpack_from("<H", data, offset)[0]


def _u32(data: bytes, offset: int) -> int:
    return struct.unpack_from("<I", data, offset)[0]


# --- query payloads --------------------------------------------------------


class _JsonView:
    """Lets a typed reply answer both ``reply.lease_ms`` and ``reply["leaseMs"]``."""

    def to_dict(self) -> dict:  # pragma: no cover - overridden
        raise NotImplementedError

    def __getitem__(self, key):
        return self.to_dict()[key]

    def __contains__(self, key) -> bool:
        return key in self.to_dict()

    def items(self):
        return self.to_dict().items()

    def get(self, key, default=None):
        return self.to_dict().get(key, default)


@dataclass(frozen=True)
class Capabilities(_JsonView):
    board: int
    firmware: str
    lease_ms: int
    poll_ms: int
    flags: int
    keyboard: bytes
    modifiers: int
    consumer: int
    device_id: bytes
    build: int

    @classmethod
    def parse(cls, report: bytes) -> "Capabilities":
        if len(report) != REPORT_SIZE or report[0] != FEATURE_ID:
            raise GdbError(ErrorCode.FRAMING, "invalid Feature 14 reply")
        if report[1:5] != b"FMT3" or report[5] != PROTOCOL_VERSION:
            raise GdbError(ErrorCode.FRAMING, "Feature 14 is not protocol 3")
        if any(report[61:]):
            raise GdbError(ErrorCode.FRAMING, "Feature 14 reserved bytes are not zero")
        value = cls(
            board=report[6],
            firmware=f"{report[7]}.{report[8]}.{report[9]}",
            lease_ms=_u16(report, 10),
            poll_ms=report[14],
            flags=_u32(report, 15),
            keyboard=bytes(report[19:47]),
            modifiers=report[47],
            consumer=report[48],
            device_id=bytes(report[49:57]),
            build=_u32(report, 57),
        )
        if value.lease_ms < 1000 or value.poll_ms == 0:
            raise GdbError(ErrorCode.FRAMING, "invalid lease or poll interval")
        return value

    def supports(self, page: int, usage: int) -> bool:
        if (page, usage) not in KEY_CATALOG:
            return False
        if page == 7 and usage >= 0xE0:
            return bool(self.modifiers & (1 << (usage - 0xE0)))
        if page == 7:
            return bool(self.keyboard[usage // 8] & (1 << (usage % 8)))
        index = CONSUMERS.index(usage) if usage in CONSUMERS else -1
        return index >= 0 and bool(self.consumer & (1 << index))

    def to_dict(self) -> dict:
        return {
            "board": self.board,
            "firmware": self.firmware,
            "leaseMs": self.lease_ms,
            "pollMs": self.poll_ms,
            "capabilities": self.flags,
            "keyboard": list(self.keyboard),
            "modifiers": self.modifiers,
            "consumer": self.consumer,
            "deviceId": list(self.device_id),
            "build": self.build,
        }


@dataclass
class InputState(_JsonView):
    """31-byte input snapshot: modifiers, 28 key bytes, consumer, buttons."""

    modifiers: int = 0
    keys: bytes = b"\x00" * 28
    consumer: int = 0
    buttons: int = 0

    def copy(self) -> "InputState":
        return InputState(self.modifiers, bytes(self.keys), self.consumer, self.buttons)

    def set(self, page: int, usage: int, down: bool) -> None:
        if (page, usage) not in KEY_CATALOG:
            raise GdbError(ErrorCode.INVALID, "unsupported HID usage")
        if page == 7 and 0xE0 <= usage <= 0xE7:
            byte, bit = "modifiers", 1 << (usage - 0xE0)
        elif page == 7:
            byte, bit = "keys", 1 << (usage % 8)
        else:
            byte, bit = "consumer", 1 << CONSUMERS.index(usage)
        if byte == "keys":
            keys = bytearray(self.keys)
            offset = usage // 8
            keys[offset] = (keys[offset] | bit) if down else (keys[offset] & ~bit & 0xFF)
            self.keys = bytes(keys)
        else:
            current = getattr(self, byte)
            setattr(self, byte, (current | bit) if down else (current & ~bit & 0xFF))

    def contains(self, page: int, usage: int) -> bool:
        if (page, usage) not in KEY_CATALOG:
            return False
        if page == 7 and 0xE0 <= usage <= 0xE7:
            return bool(self.modifiers & (1 << (usage - 0xE0)))
        if page == 7:
            return bool(self.keys[usage // 8] & (1 << (usage % 8)))
        return bool(self.consumer & (1 << CONSUMERS.index(usage)))

    def pressed(self) -> list:
        return [key for key in sorted(KEY_CATALOG) if self.contains(*key)]

    def encode(self) -> bytes:
        return bytes([self.modifiers]) + bytes(self.keys) + bytes([self.consumer, self.buttons])

    @classmethod
    def decode(cls, raw: bytes) -> "InputState":
        if len(raw) != SNAPSHOT_BYTES:
            raise GdbError(ErrorCode.FRAMING, "snapshot must be 31 bytes")
        if raw[29] & 0x80 or raw[30] & ~0x1F & 0xFF:
            raise GdbError(ErrorCode.FRAMING, "invalid input snapshot")
        value = cls(modifiers=raw[0], keys=bytes(raw[1:29]), consumer=raw[29], buttons=raw[30])
        for usage in range(224):
            if value.keys[usage // 8] & (1 << (usage % 8)) and usage not in KEYBOARD_USAGES:
                raise GdbError(ErrorCode.FRAMING, "snapshot holds a reserved usage")
        return value

    def to_dict(self) -> dict:
        return {
            "modifiers": self.modifiers,
            "keys": list(self.keys),
            "consumer": self.consumer,
            "buttons": self.buttons,
        }


@dataclass(frozen=True)
class Status(_JsonView):
    received: int
    completed: int
    pending: int
    lease_ms: int
    rx: int
    tx: int
    hid: int
    failsafe: int
    state: InputState

    @classmethod
    def parse(cls, payload: bytes) -> "Status":
        if len(payload) != STATUS_BYTES or payload[0] != 0:
            raise GdbError(ErrorCode.FRAMING, "invalid v3 status")
        return cls(
            received=_u32(payload, 1),
            completed=_u32(payload, 5),
            pending=payload[9],
            lease_ms=_u16(payload, 10),
            rx=_u16(payload, 12),
            tx=_u16(payload, 14),
            hid=_u16(payload, 16),
            failsafe=_u16(payload, 18),
            state=InputState.decode(payload[20:51]),
        )

    def to_dict(self) -> dict:
        return {
            "received": self.received,
            "completed": self.completed,
            "pending": self.pending,
            "leaseMs": self.lease_ms,
            "rx": self.rx,
            "tx": self.tx,
            "hid": self.hid,
            "failsafe": self.failsafe,
            "state": self.state.to_dict(),
        }


@dataclass(frozen=True)
class StatusExt(_JsonView):
    uptime_ms: int
    late_max_us: int
    out_depth: int
    rx_depth: int
    tx_depth: int
    sessions: int
    last_fault: int
    last_fault_age_ms: int | None
    reset_reason: int
    boot_count: int
    build: int

    @classmethod
    def parse(cls, payload: bytes) -> "StatusExt":
        if len(payload) != STATUS_EXT_BYTES or payload[0] != 0:
            raise GdbError(ErrorCode.FRAMING, "invalid STATUS_EXT")
        age = _u32(payload, 12)
        return cls(
            uptime_ms=_u32(payload, 1),
            late_max_us=_u16(payload, 5),
            out_depth=payload[7],
            rx_depth=payload[8],
            tx_depth=payload[9],
            sessions=payload[10],
            last_fault=payload[11],
            last_fault_age_ms=None if age == 0xFFFFFFFF else age,
            reset_reason=payload[16],
            boot_count=_u16(payload, 17),
            build=_u32(payload, 19),
        )

    def to_dict(self) -> dict:
        return {
            "uptimeMs": self.uptime_ms,
            "lateMaxUs": self.late_max_us,
            "outDepth": self.out_depth,
            "rxDepth": self.rx_depth,
            "txDepth": self.tx_depth,
            "sessions": self.sessions,
            "lastFault": self.last_fault,
            "lastFaultAgeMs": self.last_fault_age_ms,
            "resetReason": self.reset_reason,
            "bootCount": self.boot_count,
            "build": self.build,
        }


@dataclass(frozen=True)
class SessionInfo(_JsonView):
    id: int
    lease_ms: int
    held: int
    flags: int

    def to_dict(self) -> dict:
        return {
            "id": self.id,
            "leaseMs": self.lease_ms,
            "held": self.held,
            "flags": self.flags,
        }


def parse_session_list(payload: bytes) -> list:
    """Eight fixed six-byte slots; there is no result byte (protocol-v3.md:99)."""
    if len(payload) != SESSION_LIST_BYTES:
        raise GdbError(ErrorCode.FRAMING, "invalid SESSION_LIST payload")
    sessions = []
    for index in range(SESSION_SLOTS):
        base = index * SESSION_ENTRY
        entry = payload[base : base + SESSION_ENTRY]
        if len(entry) < SESSION_ENTRY:
            break
        session_id = _u16(entry, 0)
        if session_id == 0:
            continue
        sessions.append(
            SessionInfo(
                id=session_id,
                lease_ms=_u16(entry, 2),
                held=entry[4],
                flags=entry[5],
            )
        )
    return sessions


# --- motion maths (sdk/motion/src/lib.rs) ---------------------------------


class SplitMix64:
    """Tiny, seedable RNG; identical on every platform."""

    def __init__(self, seed: int = 0):
        self._state = seed & 0xFFFFFFFFFFFFFFFF

    @classmethod
    def from_entropy(cls) -> "SplitMix64":
        nanos = time.time_ns() & 0xFFFFFFFFFFFFFFFF
        pid = os.getpid() & 0xFFFFFFFFFFFFFFFF
        low = ((pid << 32) | (pid >> 32)) & 0xFFFFFFFFFFFFFFFF  # rotate_left(32)
        return cls(nanos ^ low ^ 0x9E3779B97F4A7C15)

    def next_u64(self) -> int:
        mask = 0xFFFFFFFFFFFFFFFF
        self._state = (self._state + 0x9E3779B97F4A7C15) & mask
        z = self._state
        z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) & mask
        z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) & mask
        return (z ^ (z >> 31)) & mask

    def unit(self) -> float:
        # 53 significant bits, the value every language reproduces.
        return (self.next_u64() >> 11) / float(1 << 53)


@dataclass(frozen=True)
class Screen:
    width: int = 1920
    height: int = 1080


@dataclass(frozen=True)
class Node:
    at_ms: int
    x: int
    y: int


@dataclass(frozen=True)
class Profile:
    """Movement profile; the defaults mirror ``HumanProfile::default()``."""

    ease: int = Ease.JERK
    bezier: tuple = (0.42, 0.0, 0.58, 1.0)
    curve: float = 1.0
    jitter_px: float = 1.0
    overshoot_pct: float = 0.0
    settle_ms: int = 0
    min_step_px: int = 1

    @classmethod
    def linear(cls) -> "Profile":
        """Straight, constant speed, no jitter, no overshoot."""
        return cls(ease=Ease.LINEAR, curve=0.0, jitter_px=0.0)

    @classmethod
    def from_library(cls) -> "Profile":
        return cls()


def _clamp(value: float, low: float, high: float) -> float:
    return max(low, min(high, value))


def _round_half_away(value: float) -> int:
    """Rust's ``f64::round``: half away from zero, not banker's rounding."""
    return int(math.floor(value + 0.5)) if value >= 0 else int(math.ceil(value - 0.5))


def _bezier(a: float, b: float, c: float, d: float, t: float) -> float:
    u = 1.0 - t
    return u * u * u * a + 3.0 * u * u * t * b + 3.0 * u * t * t * c + t * t * t * d


def _cubic_bezier_ease(t: float, x1: float, y1: float, x2: float, y2: float) -> float:
    """Fixed-iteration Newton solve so the result never depends on luck."""
    x1 = _clamp(x1, 0.0, 1.0)
    x2 = _clamp(x2, 0.0, 1.0)

    def curve_x(u: float) -> float:
        return _bezier(0.0, x1, x2, 1.0, u)

    def curve_y(u: float) -> float:
        return _bezier(0.0, y1, y2, 1.0, u)

    u = t
    for _ in range(8):
        x = curve_x(u) - t
        if abs(x) < 1e-9:
            break
        slope = (curve_x(min(u + 1e-6, 1.0)) - curve_x(max(u - 1e-6, 0.0))) / 2e-6
        if abs(slope) < 1e-9:
            break
        u = _clamp(u - x / slope, 0.0, 1.0)
    return _clamp(curve_y(u), 0.0, 1.0)


def ease_apply(profile: Profile, t: float) -> float:
    t = _clamp(t, 0.0, 1.0)
    ease = profile.ease
    if ease == Ease.LINEAR:
        return t
    if ease == Ease.JERK:
        # Smooth at both ends, no overshoot.
        return t * t * t * (10.0 + t * (-15.0 + 6.0 * t))
    if ease == Ease.IN:
        return t * t * t
    if ease == Ease.OUT:
        u = 1.0 - t
        return 1.0 - u * u * u
    if ease == Ease.IN_OUT:
        return t * t * (3.0 - 2.0 * t)
    if ease == Ease.CUBIC_BEZIER:
        return _cubic_bezier_ease(t, *profile.bezier)
    return t


def split_delta(x: int, y: int) -> list:
    """Splits one relative move into HID-sized chunks (±1024 per axis)."""
    parts = []
    while x != 0 or y != 0:
        dx = max(-1024, min(1024, x))
        dy = max(-1024, min(1024, y))
        parts.append((dx, dy))
        x -= dx
        y -= dy
    return parts


def natural_duration(distance: float, rng: SplitMix64) -> int:
    value = 150.0 + distance * 0.42 + rng.unit() * 70.0 - 25.0
    return int(_clamp(value, 170.0, 900.0))


def trajectory_with(
    start: Point,
    target: Point,
    screen: Screen = Screen(),
    duration_ms: int = 0,
    profile: Profile = Profile(),
    rng: SplitMix64 | None = None,
) -> list:
    """Samples a human-ish path from ``start`` to ``target`` in host coordinates."""
    if rng is None:
        rng = SplitMix64.from_entropy()
    if screen.width <= 1 or screen.height <= 1:
        raise GdbError(ErrorCode.INVALID, "screen size is invalid")
    target = (
        max(0, min(screen.width - 1, target[0])),
        max(0, min(screen.height - 1, target[1])),
    )
    dx = float(target[0]) - float(start[0])
    dy = float(target[1]) - float(start[1])
    distance = math.hypot(dx, dy)
    duration = (
        natural_duration(distance, rng)
        if duration_ms == 0
        else max(80, min(2000, duration_ms))
    )
    if distance < 1.0:
        return [Node(at_ms=0, x=target[0], y=target[1])]
    nx = -dy / distance
    ny = dx / distance
    over = _clamp(distance * profile.overshoot_pct / 100.0, 0.0, 160.0)
    if over > 0.5:
        aim = (
            _clamp(float(target[0]) + dx / distance * over, 0.0, float(screen.width - 1)),
            _clamp(float(target[1]) + dy / distance * over, 0.0, float(screen.height - 1)),
        )
    else:
        aim = (float(target[0]), float(target[1]))
    limit = _clamp(distance * 0.13, 4.0, 110.0) * _clamp(profile.curve, 0.0, 2.0)
    bend = (rng.unit() * 2.0 - 1.0) * limit
    a1 = 0.20 + rng.unit() * 0.18
    a2 = 0.62 + rng.unit() * 0.22
    bend2 = bend * 0.55 + (rng.unit() * 2.0 - 1.0) * limit * 0.20
    c1 = (float(start[0]) + dx * a1 + nx * bend, float(start[1]) + dy * a1 + ny * bend)
    c2 = (float(start[0]) + dx * a2 + nx * bend2, float(start[1]) + dy * a2 + ny * bend2)
    phase = rng.unit() * math.tau
    frequency = 1.1 + rng.unit() * 1.2
    amplitude = _clamp(profile.jitter_px, 0.0, 24.0)
    steps = max(10, min(180, duration // 8))
    nodes = []
    last = start
    for step in range(1, steps + 1):
        p = float(step) / float(steps)
        t = ease_apply(profile, p)
        jitter = amplitude * math.sin(math.pi * p) * math.sin(phase + p * math.tau * frequency)
        if step == steps:
            point = (_round_half_away(aim[0]), _round_half_away(aim[1]))
        else:
            point = (
                _round_half_away(_bezier(float(start[0]), c1[0], c2[0], aim[0], t) + nx * jitter),
                _round_half_away(_bezier(float(start[1]), c1[1], c2[1], aim[1], t) + ny * jitter),
            )
        point = (
            max(0, min(screen.width - 1, point[0])),
            max(0, min(screen.height - 1, point[1])),
        )
        step_gap = abs(point[0] - last[0]) + abs(point[1] - last[1])
        if step_gap >= max(1, profile.min_step_px) or step == steps:
            nodes.append(Node(at_ms=duration * step // steps, x=point[0], y=point[1]))
            last = point
    if over > 0.5:
        settle_at = (nodes[-1].at_ms if nodes else 0) + 90
        nodes.append(Node(at_ms=settle_at, x=target[0], y=target[1]))
    if profile.settle_ms > 0:
        last_at = nodes[-1].at_ms if nodes else 0
        nodes.append(Node(at_ms=last_at + profile.settle_ms, x=target[0], y=target[1]))
    return nodes


STEP_ARRIVED = "arrived"
STEP_WAIT = "wait"
STEP_MOVE = "move"


@dataclass(frozen=True)
class Step:
    kind: str
    dx: int = 0
    dy: int = 0


class RelativeTracker:
    """Host-side feedback for relative HID motion; never emits absolute values."""

    TICK_MS = 8.0

    def __init__(self):
        self.gain = [1.0, 1.0]
        self.pending = None  # (sent_at_s, previous, delta)

    def step(self, position: Point, target: Point, now_s: float | None = None) -> Step:
        if now_s is None:
            now_s = time.monotonic()
        if self.pending is not None:
            sent_at, previous, delta = self.pending
            # Do not integrate the same stale feedback on every tick.
            if (now_s - sent_at) * 1000.0 < self.TICK_MS:
                return Step(STEP_WAIT)
            if position == previous and (now_s - sent_at) * 1000.0 < 32.0:
                return Step(STEP_WAIT)
            for axis in (0, 1):
                moved = position[axis] - previous[axis]
                sent = delta[axis]
                if sent != 0 and moved != 0 and (moved > 0) == (sent > 0):
                    measured = _clamp(float(moved) / float(sent), 0.25, 32.0)
                    self.gain[axis] = max(measured, self.gain[axis] * 0.75)
            self.pending = None
        error = [target[0] - position[0], target[1] - position[1]]
        if all(abs(value) <= 1 for value in error):
            return Step(STEP_ARRIVED)
        delta = [0, 0]
        for axis in (0, 1):
            if abs(error[axis]) > 1:
                damping = 1.0 if abs(error[axis]) <= 8 else 0.75
                step = _round_half_away(float(error[axis]) * damping / self.gain[axis])
                if step == 0:
                    delta[axis] = 1 if error[axis] > 0 else -1
                else:
                    delta[axis] = max(-512, min(512, step))
        self.pending = (now_s, position, (delta[0], delta[1]))
        return Step(STEP_MOVE, delta[0], delta[1])


# --- keyboard layout (sdk/rust/src/keyboard.rs) ---------------------------

_SHIFTED = {
    "!": 0x1E,
    "@": 0x1F,
    "#": 0x20,
    "$": 0x21,
    "%": 0x22,
    "^": 0x23,
    "&": 0x24,
    "*": 0x25,
    "(": 0x26,
    ")": 0x27,
}

_PLAIN = {
    "-": 0x2D,
    "=": 0x2E,
    "[": 0x2F,
    "]": 0x30,
    "\\": 0x31,
    ";": 0x33,
    "'": 0x34,
    "`": 0x35,
    ",": 0x36,
    ".": 0x37,
    "/": 0x38,
    "\n": 0x28,
    "\t": 0x2B,
    " ": 0x2C,
}

_SHIFTED_SYMBOLS = {
    "_": 0x2D,
    "+": 0x2E,
    "{": 0x2F,
    "}": 0x30,
    "|": 0x31,
    ":": 0x33,
    '"': 0x34,
    "~": 0x35,
    "<": 0x36,
    ">": 0x37,
    "?": 0x38,
}


def _usage_for(char: str) -> tuple | None:
    if "a" <= char <= "z":
        return (0x04 + ord(char) - ord("a"), False)
    if "A" <= char <= "Z":
        return (0x04 + ord(char) - ord("A"), True)
    if "1" <= char <= "9":
        return (0x1E + ord(char) - ord("1"), False)
    if char == "0":
        return (0x27, False)
    if char in _SHIFTED:
        return (_SHIFTED[char], True)
    if char in _PLAIN:
        return (_PLAIN[char], False)
    if char in _SHIFTED_SYMBOLS:
        return (_SHIFTED_SYMBOLS[char], True)
    return None


@dataclass(frozen=True)
class KeyStroke:
    page: int
    usage: int
    shift: bool


def strokes(text: str) -> list:
    """US-layout keystrokes; characters with no usage are dropped."""
    out = []
    for char in text:
        if char == "\r":
            continue
        mapped = _usage_for(char)
        if mapped is not None:
            out.append(KeyStroke(page=7, usage=mapped[0], shift=mapped[1]))
    return out


@dataclass(frozen=True)
class KeyStep:
    page: int = 7
    usage: int = 0
    hold_ms: int = 18
    gap_ms: int = 0


LEFT_SHIFT = 0xE1


# --- platform facts the protocol does not provide -------------------------


class _RECT(ctypes.Structure):
    _fields_ = [
        ("left", ctypes.c_long),
        ("top", ctypes.c_long),
        ("right", ctypes.c_long),
        ("bottom", ctypes.c_long),
    ]


class _POINT(ctypes.Structure):
    _fields_ = [("x", ctypes.c_long), ("y", ctypes.c_long)]


class _MONITORINFO(ctypes.Structure):
    _fields_ = [
        ("cbSize", ctypes.c_ulong),
        ("rcMonitor", _RECT),
        ("rcWork", _RECT),
        ("dwFlags", ctypes.c_ulong),
    ]


_USER32 = None
_dpi_done = False


def _user32():
    global _USER32
    if _USER32 is None:
        dll = ctypes.WinDLL("user32.dll", use_last_error=True)
        dll.GetPhysicalCursorPos.argtypes = [ctypes.POINTER(_POINT)]
        dll.GetPhysicalCursorPos.restype = ctypes.c_int
        dll.MonitorFromPoint.argtypes = [_POINT, ctypes.c_ulong]
        dll.MonitorFromPoint.restype = ctypes.c_void_p
        dll.GetMonitorInfoW.argtypes = [ctypes.c_void_p, ctypes.POINTER(_MONITORINFO)]
        dll.GetMonitorInfoW.restype = ctypes.c_int
        dll.SetProcessDpiAwarenessContext.argtypes = [ctypes.c_void_p]
        dll.SetProcessDpiAwarenessContext.restype = ctypes.c_int
        _USER32 = dll
    return _USER32


def enable_dpi_awareness() -> None:
    """Makes physical pixels what the sensor sees; callable more than once."""
    global _dpi_done
    if _dpi_done or sys.platform != "win32":
        return
    _dpi_done = True
    try:
        # DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 == (HANDLE)-4
        _user32().SetProcessDpiAwarenessContext(ctypes.c_void_p(-4 & 0xFFFFFFFFFFFFFFFF))
    except OSError:  # pragma: no cover - platform dependent
        pass


def screen_and_cursor() -> tuple:
    """Primary-monitor rect and the cursor relative to its top-left corner."""
    if sys.platform != "win32":
        raise GdbError(
            ErrorCode.UNSUPPORTED, "absolute mouse coordinates need Windows cursor APIs"
        )
    user32 = _user32()
    point = _POINT()
    if user32.GetPhysicalCursorPos(ctypes.byref(point)) == 0:
        raise GdbError(
            ErrorCode.UNSUPPORTED,
            f"GetPhysicalCursorPos failed: {ctypes.WinError(ctypes.get_last_error())}",
        )
    monitor = user32.MonitorFromPoint(_POINT(0, 0), 1)  # MONITOR_DEFAULTTOPRIMARY
    info = _MONITORINFO()
    info.cbSize = ctypes.sizeof(_MONITORINFO)
    if user32.GetMonitorInfoW(monitor, ctypes.byref(info)) == 0:
        raise GdbError(
            ErrorCode.UNSUPPORTED,
            f"GetMonitorInfoW failed: {ctypes.WinError(ctypes.get_last_error())}",
        )
    screen = Screen(
        width=info.rcMonitor.right - info.rcMonitor.left,
        height=info.rcMonitor.bottom - info.rcMonitor.top,
    )
    return screen, (point.x - info.rcMonitor.left, point.y - info.rcMonitor.top)


# --- HID.dll transport ----------------------------------------------------

_kernel32 = None
_hid = None

ERROR_IO_PENDING = 997
ERROR_INVALID_PARAMETER = 87
WAIT_OBJECT_0 = 0
WAIT_TIMEOUT = 0x102
GENERIC_READ = 0x80000000
GENERIC_WRITE = 0x40000000
FILE_SHARE_READ = 0x00000001
FILE_SHARE_WRITE = 0x00000002
OPEN_EXISTING = 3
FILE_FLAG_OVERLAPPED = 0x40000000
CREATE_NO_WINDOW = 0x08000000
INVALID_HANDLE = ctypes.c_void_p(-1).value


class _OVERLAPPED(ctypes.Structure):
    _fields_ = [
        ("Internal", ctypes.c_size_t),
        ("InternalHigh", ctypes.c_size_t),
        ("Offset", ctypes.c_ulong),
        ("OffsetHigh", ctypes.c_ulong),
        ("hEvent", ctypes.c_void_p),
    ]


class _HIDD_ATTRIBUTES(ctypes.Structure):
    _fields_ = [
        ("Size", ctypes.c_ulong),
        ("VendorID", ctypes.c_ushort),
        ("ProductID", ctypes.c_ushort),
        ("VersionNumber", ctypes.c_ushort),
    ]


def _kernel():
    global _kernel32
    if _kernel32 is None:
        dll = ctypes.WinDLL("kernel32.dll", use_last_error=True)
        dll.CreateFileW.argtypes = [
            ctypes.c_wchar_p,
            ctypes.c_ulong,
            ctypes.c_ulong,
            ctypes.c_void_p,
            ctypes.c_ulong,
            ctypes.c_ulong,
            ctypes.c_void_p,
        ]
        dll.CreateFileW.restype = ctypes.c_void_p
        for name in ("ReadFile", "WriteFile"):
            getattr(dll, name).argtypes = [
                ctypes.c_void_p,
                ctypes.c_void_p,
                ctypes.c_ulong,
                ctypes.c_void_p,
                ctypes.POINTER(_OVERLAPPED),
            ]
            getattr(dll, name).restype = ctypes.c_int
        dll.GetOverlappedResult.argtypes = [
            ctypes.c_void_p,
            ctypes.POINTER(_OVERLAPPED),
            ctypes.POINTER(ctypes.c_ulong),
            ctypes.c_int,
        ]
        dll.GetOverlappedResult.restype = ctypes.c_int
        dll.CancelIoEx.argtypes = [ctypes.c_void_p, ctypes.POINTER(_OVERLAPPED)]
        dll.CancelIoEx.restype = ctypes.c_int
        dll.CreateEventW.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_int, ctypes.c_wchar_p]
        dll.CreateEventW.restype = ctypes.c_void_p
        dll.ResetEvent.argtypes = [ctypes.c_void_p]
        dll.ResetEvent.restype = ctypes.c_int
        dll.WaitForSingleObject.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
        dll.WaitForSingleObject.restype = ctypes.c_ulong
        dll.CloseHandle.argtypes = [ctypes.c_void_p]
        dll.CloseHandle.restype = ctypes.c_int
        _kernel32 = dll
    return _kernel32


def _hidapi():
    global _hid
    if _hid is None:
        dll = ctypes.WinDLL("hid.dll", use_last_error=True)
        dll.HidD_GetAttributes.argtypes = [ctypes.c_void_p, ctypes.POINTER(_HIDD_ATTRIBUTES)]
        dll.HidD_GetAttributes.restype = ctypes.c_ubyte
        for name in ("HidD_GetSerialNumberString", "HidD_GetProductString", "HidD_GetManufacturerString"):
            getattr(dll, name).argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_ulong]
            getattr(dll, name).restype = ctypes.c_ubyte
        dll.HidD_GetFeature.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_ulong]
        dll.HidD_GetFeature.restype = ctypes.c_ubyte
        _hid = dll
    return _hid


def interface_path(instance_id: str) -> str:
    """``HID\\VID_03F0&PID_0024&MI_01\\7&..`` → ``\\\\?\\hid#vid_03f0&pid_0024&mi_01#7&..#<guid>``.

    SetupAPI enumeration through ctypes is unreliable on x64, so the device
    interface path is assembled from the PnP instance id instead.
    """
    parts = instance_id.strip().split("\\")
    if len(parts) != 3 or not parts[0].upper().startswith("HID"):
        raise GdbError(ErrorCode.INVALID, f"not a HID instance id: {instance_id!r}")
    return f"\\\\?\\hid#{parts[1].lower()}#{parts[2].lower()}#{HID_INTERFACE_GUID}"


def _instance_ids() -> list:
    """Present ``HID\\VID_03F0&PID_0024...`` instance ids, newest Windows way."""
    script = (
        "Get-PnpDevice -PresentOnly | Where-Object { $_.InstanceId -match "
        "'VID_03F0&PID_0024' } | Select-Object -ExpandProperty InstanceId"
    )
    try:
        out = subprocess.run(
            ["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", script],
            capture_output=True,
            text=True,
            timeout=30,
            creationflags=CREATE_NO_WINDOW,
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise GdbError(ErrorCode.IO, f"cannot enumerate HID devices: {error}") from None
    ids = []
    for line in out.stdout.splitlines():
        line = line.strip()
        if line.upper().startswith("HID\\VID_03F0&PID_0024".upper()):
            ids.append(line)
    return ids


class Transport:
    """Duck-type for the three transport methods; subclass or just match it."""

    def feature(self) -> bytes | None:
        raise NotImplementedError

    def write(self, report: bytes) -> None:
        raise NotImplementedError

    def read(self, timeout_ms: int) -> bytes | None:
        raise NotImplementedError

    def close(self) -> None:
        pass


class HidTransport(Transport):
    """Vendor-HID reports through ``hid.dll`` with per-call timeouts."""

    def __init__(self, path: str, write_timeout_ms: int = 2000):
        self.path = path
        self.write_timeout_ms = max(100, int(write_timeout_ms))
        self._handle = 0
        self._event = 0
        if sys.platform != "win32":
            raise GdbError(ErrorCode.UNSUPPORTED, "physical HID is only implemented on Windows")
        kernel = _kernel()
        handle = kernel.CreateFileW(
            path,
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_OVERLAPPED,
            None,
        )
        if not handle or handle == INVALID_HANDLE:
            raise GdbError(
                ErrorCode.DISCONNECTED,
                f"CreateFileW failed for {path}: {ctypes.WinError(ctypes.get_last_error())}",
            )
        event = kernel.CreateEventW(None, 1, 0, None)
        if not event:
            kernel.CloseHandle(handle)
            raise GdbError(ErrorCode.IO, "CreateEventW failed")
        self._handle = handle
        self._event = event
        self._overlapped = _OVERLAPPED()

    # -- descriptors -------------------------------------------------------

    def _string(self, name: str) -> str:
        buffer = ctypes.create_unicode_buffer(256)
        fn = getattr(_hidapi(), name)
        if fn(self._handle, ctypes.byref(buffer), ctypes.sizeof(buffer)) == 0:
            return ""
        return buffer.value

    def serial_number(self) -> str:
        return self._string("HidD_GetSerialNumberString")

    def product(self) -> str:
        return self._string("HidD_GetProductString")

    def manufacturer(self) -> str:
        return self._string("HidD_GetManufacturerString")

    def attributes(self) -> tuple:
        info = _HIDD_ATTRIBUTES()
        info.Size = ctypes.sizeof(_HIDD_ATTRIBUTES)
        if _hidapi().HidD_GetAttributes(self._handle, ctypes.byref(info)) == 0:
            return (0, 0)
        return (info.VendorID, info.ProductID)

    # -- reports -----------------------------------------------------------

    def feature(self) -> bytes | None:
        """GET_FEATURE(14); ``None`` when the collection does not answer.

        ``ReportBuffer[0]`` must carry the report id on the way in: a zeroed
        buffer makes ``HidD_GetFeature`` fail with ERROR_INVALID_PARAMETER.
        """
        buffer = ctypes.create_string_buffer(REPORT_SIZE + 1)
        buffer[0] = bytes([FEATURE_ID])
        ok = _hidapi().HidD_GetFeature(self._handle, buffer, REPORT_SIZE + 1)
        if not ok and ctypes.get_last_error() == ERROR_INVALID_PARAMETER:
            # Some drivers insist on exactly FeatureReportByteLength.
            ok = _hidapi().HidD_GetFeature(self._handle, buffer, REPORT_SIZE)
        if not ok:
            return None
        report = buffer.raw
        if report[0] != FEATURE_ID:
            return None
        return report[:REPORT_SIZE]

    def write(self, report: bytes) -> None:
        report = bytes(report)
        if len(report) != REPORT_SIZE or report[0] != OUT_ID:
            raise GdbError(ErrorCode.INVALID, "output report must be 64 bytes behind id 12")
        kernel = _kernel()
        kernel.ResetEvent(self._event)
        overlapped = _OVERLAPPED()
        overlapped.hEvent = self._event
        buffer = ctypes.create_string_buffer(report, REPORT_SIZE)
        ok = kernel.WriteFile(
            self._handle, buffer, REPORT_SIZE, None, ctypes.byref(overlapped)
        )
        if not ok:
            error = ctypes.get_last_error()
            if error != ERROR_IO_PENDING:
                raise GdbError(
                    ErrorCode.DISCONNECTED, f"WriteFile failed: {ctypes.WinError(error)}"
                )
            waited = kernel.WaitForSingleObject(self._event, self.write_timeout_ms)
            if waited != WAIT_OBJECT_0:
                kernel.CancelIoEx(self._handle, ctypes.byref(overlapped))
                raise GdbError(ErrorCode.TIMEOUT, "HID write timed out")
        written = ctypes.c_ulong(0)
        if kernel.GetOverlappedResult(self._handle, ctypes.byref(overlapped), ctypes.byref(written), 1) == 0:
            raise GdbError(
                ErrorCode.DISCONNECTED,
                f"GetOverlappedResult failed: {ctypes.WinError(ctypes.get_last_error())}",
            )
        if written.value != REPORT_SIZE:
            raise GdbError(
                ErrorCode.IO, f"short HID write: {written.value}/{REPORT_SIZE}"
            )

    def read(self, timeout_ms: int) -> bytes | None:
        """One input report, or ``None`` when the timeout expired."""
        kernel = _kernel()
        kernel.ResetEvent(self._event)
        overlapped = _OVERLAPPED()
        overlapped.hEvent = self._event
        buffer = ctypes.create_string_buffer(REPORT_SIZE + 1)
        ok = kernel.ReadFile(
            self._handle, buffer, REPORT_SIZE + 1, None, ctypes.byref(overlapped)
        )
        if not ok:
            error = ctypes.get_last_error()
            if error != ERROR_IO_PENDING:
                raise GdbError(
                    ErrorCode.DISCONNECTED, f"ReadFile failed: {ctypes.WinError(error)}"
                )
            waited = kernel.WaitForSingleObject(self._event, max(1, int(timeout_ms)))
            if waited == WAIT_TIMEOUT:
                kernel.CancelIoEx(self._handle, ctypes.byref(overlapped))
                kernel.WaitForSingleObject(self._event, 200)
                return None
            if waited != WAIT_OBJECT_0:
                raise GdbError(ErrorCode.IO, "WaitForSingleObject failed")
        read = ctypes.c_ulong(0)
        if kernel.GetOverlappedResult(self._handle, ctypes.byref(overlapped), ctypes.byref(read), 1) == 0:
            raise GdbError(
                ErrorCode.DISCONNECTED,
                f"GetOverlappedResult failed: {ctypes.WinError(ctypes.get_last_error())}",
            )
        if read.value == 0:
            return None
        data = buffer.raw[: read.value]
        if data[0] == IN_ID and len(data) >= REPORT_SIZE:
            return data[:REPORT_SIZE]
        if len(data) == REPORT_SIZE - 1 and data[0] == PROTOCOL_VERSION:
            # A stack that strips the report id: put it back where the SDK wants it.
            return bytes([IN_ID]) + data
        raise GdbError(ErrorCode.FRAMING, f"short HID read: {read.value}/{REPORT_SIZE}")

    def close(self) -> None:
        kernel = _kernel()
        if self._handle:
            try:
                kernel.CancelIoEx(self._handle, None)
            except OSError:  # pragma: no cover - platform dependent
                pass
            kernel.CloseHandle(self._handle)
            self._handle = 0
        if self._event:
            kernel.CloseHandle(self._event)
            self._event = 0

    def __del__(self):  # pragma: no cover - best effort
        try:
            self.close()
        except Exception:
            pass


# --- in-memory device -----------------------------------------------------


class MockTransport(Transport):
    """Speaks the wire protocol in memory, including the firmware's error paths.

    ``reports`` holds every 64-byte frame the SDK wrote, so a test can assert on
    bytes rather than on return values.
    """

    def __init__(self, build: int = 0x0DB00001, uptime_ms: int = 1000):
        self.reports = []  # raw 64-byte frames, in order
        self.writes = []  # decoded Packet objects, in order
        self.held = InputState()
        self.session = 0
        self.sessions = []
        self.closed = True
        self.silent = False
        self.disconnect_after = None
        self.feature_missing = False
        self.build = build
        self.uptime_ms = uptime_ms
        self._replies = deque()
        self._lock = threading.Lock()

    # -- inspection --------------------------------------------------------

    def packets(self, opcode: int | None = None) -> list:
        if opcode is None:
            return list(self.writes)
        return [packet for packet in self.writes if packet.opcode == opcode]

    def count(self, opcode: int) -> int:
        return sum(1 for packet in self.writes if packet.opcode == opcode)

    def set_silent(self, silent: bool) -> None:
        self.silent = bool(silent)

    def set_disconnect_after(self, writes: int | None) -> None:
        self.disconnect_after = writes

    def set_feature_missing(self, missing: bool) -> None:
        self.feature_missing = bool(missing)

    def set_build(self, build: int) -> None:
        self.build = build

    def is_closed(self) -> bool:
        return self.closed

    def push_frame(self, report: bytes) -> None:
        """Queues one raw input report, e.g. a foreign session's reply."""
        with self._lock:
            self._replies.append(bytes(report))

    def push_reply(self, opcode: int, session: int, sequence: int, payload: bytes) -> None:
        reply = opcode if opcode == EVENT else (opcode | 0x80)
        self.push_frame(encode_packet(reply, session, sequence, payload, IN_ID))

    # -- firmware emulation ------------------------------------------------

    def _feature_report(self) -> bytes:
        report = bytearray(REPORT_SIZE)
        report[0] = FEATURE_ID
        report[1:5] = b"FMT3"
        report[5] = PROTOCOL_VERSION
        report[6] = 2
        report[7], report[8], report[9] = 3, 2, 0
        report[10:12] = struct.pack("<H", LEASE_MS)
        report[14] = 1
        report[15:19] = struct.pack("<I", 15)
        for usage in KEYBOARD_USAGES:
            if usage < 224:
                report[19 + usage // 8] |= 1 << (usage % 8)
        report[47] = 0xFF
        report[48] = 0x7F
        report[49:57] = bytes([0x42, 0x50, 0x33, 0x30, 0x34, 0x42, 0x38, 0x33])
        report[57:61] = struct.pack("<I", self.build)
        return bytes(report)

    def feature(self) -> bytes | None:
        if self.feature_missing:
            return None
        return self._feature_report()

    def write(self, report: bytes) -> None:
        packet = decode_packet(report, OUT_ID)
        self.reports.append(bytes(report))
        self.writes.append(packet)
        if self.disconnect_after is not None and len(self.writes) > self.disconnect_after:
            raise GdbError(ErrorCode.DISCONNECTED, "mock transport disconnected")
        self._emulate(packet)

    def _emulate(self, packet: Packet) -> None:
        session, sequence, payload = packet.session, packet.sequence, packet.payload
        opcode = packet.opcode
        if opcode == OPEN:
            if session == 0 or sequence != 1 or session in self.sessions or len(self.sessions) >= SESSION_SLOTS:
                self._reply(opcode, session, sequence, b"\x02")
                return
            self.session = session
            self.sessions.append(session)
            self.closed = False
            self._reply(opcode, session, sequence, b"\x00")
            return
        if opcode == CLOSE:
            if session in self.sessions:
                self.sessions.remove(session)
            if self.session == session:
                self.session = 0
                self.closed = True
            self.held = InputState()
            self._reply(opcode, session, sequence, b"\x00")
            return
        if opcode == HEARTBEAT:
            return
        if session != self.session:
            # Unknown session: the firmware drops input silently.
            return
        if opcode == KEY:
            if len(payload) != 5:
                self._reply(EVENT, session, sequence, b"\x03")
                return
            page, usage = _u16(payload, 0), _u16(payload, 2)
            try:
                self.held.set(page, usage, payload[4] != 0)
            except GdbError:
                self._reply(EVENT, session, sequence, b"\x03")
            return
        if opcode == SNAPSHOT:
            try:
                self.held = InputState.decode(payload)
            except GdbError:
                self._reply(EVENT, session, sequence, b"\x03")
            return
        if opcode == MOUSE_MOVE or opcode == WHEEL:
            return
        if opcode == MOUSE_BUTTONS:
            self.held.buttons = payload[0] if payload else 0
            return
        if opcode == RELEASE_ALL:
            self.held = InputState()
            self._reply(opcode, session, sequence, b"\x00")
            return
        if opcode == BARRIER or opcode == BOOTLOADER:
            self._reply(opcode, session, sequence, b"\x00")
            return
        if opcode == STATUS:
            body = bytearray(STATUS_BYTES)
            body[1:5] = struct.pack("<I", sequence)
            body[5:9] = struct.pack("<I", sequence)
            body[10:12] = struct.pack("<H", LEASE_MS)
            body[20:51] = self.held.encode()
            self._reply(opcode, session, sequence, bytes(body))
            return
        if opcode == STATUS_EXT:
            body = bytearray(STATUS_EXT_BYTES)
            body[1:5] = struct.pack("<I", self.uptime_ms)
            body[10] = len(self.sessions)
            body[12:16] = struct.pack("<I", 0xFFFFFFFF)
            body[16] = 3
            body[17:19] = struct.pack("<H", 1)
            body[19:23] = struct.pack("<I", self.build)
            self._reply(opcode, session, sequence, bytes(body))
            return
        if opcode == SESSION_LIST:
            body = bytearray(SESSION_LIST_BYTES)
            for index, identifier in enumerate(self.sessions[:SESSION_SLOTS]):
                base = index * SESSION_ENTRY
                body[base : base + 2] = struct.pack("<H", identifier & 0xFFFF)
                body[base + 2 : base + 4] = struct.pack("<H", LEASE_MS)
                body[base + 4] = len(self.held.pressed())
            self._reply(opcode, session, sequence, bytes(body))
            return
        self._reply(EVENT, session, sequence, b"\x03")

    def _reply(self, opcode: int, session: int, sequence: int, payload: bytes) -> None:
        if self.silent:
            return
        self.push_reply(opcode, session, sequence, payload)

    def read(self, timeout_ms: int) -> bytes | None:
        with self._lock:
            return self._replies.popleft() if self._replies else None


# --- session --------------------------------------------------------------


_session_counter = itertools.count(1)
_session_lock = threading.Lock()


def new_session_id() -> int:
    """Non-zero, per-process-unique session ids."""
    now = time.time_ns()
    with _session_lock:
        bump = next(_session_counter) * 0x9E3779B9
    mixed = (now & 0xFFFFFFFF) ^ ((now >> 32) & 0xFFFFFFFF) ^ os.getpid() ^ bump
    return max(1, mixed & 0xFFFFFFFF)


class Session:
    """One owner slot: Feature 14 handshake, OPEN, sequenced commands, reply matching."""

    def __init__(self, transport: Transport, capabilities: Capabilities, session_id: int, timeout_ms: int):
        self.transport = transport
        self.capabilities = capabilities
        self.id = session_id
        self.sequence = 0
        self.timeout_ms = max(100, int(timeout_ms))
        self.last_renew = time.monotonic()

    @classmethod
    def open(
        cls, transport: Transport, timeout_ms: int = 1000, session_id: int | None = None
    ) -> "Session":
        report = transport.feature()
        if not report:
            raise GdbError(ErrorCode.DISCONNECTED, "no Feature 14 reply")
        capabilities = Capabilities.parse(report)
        session = cls(
            transport,
            capabilities,
            max(1, int(session_id if session_id is not None else new_session_id())),
            timeout_ms,
        )
        session.command(OPEN)
        return session

    # -- state -------------------------------------------------------------

    def renew_age_ms(self) -> float:
        return (time.monotonic() - self.last_renew) * 1000.0

    # -- commands ----------------------------------------------------------

    def command(
        self, opcode: int, payload: bytes = b"", expects_reply: bool | None = None
    ) -> Packet | None:
        if self.sequence >= 0xFFFFFFFF:
            raise GdbError(ErrorCode.INVALID, "sequence exhausted; reopen")
        self.sequence += 1
        sequence = self.sequence
        report = encode_packet(opcode, self.id, sequence, payload)
        if opcode not in NON_RENEWING:
            self.last_renew = time.monotonic()
        _trace(
            f"out opcode={opcode} session={self.id} sequence={sequence} "
            f"payload={_payload_debug(payload)}"
        )
        self.transport.write(report)
        if expects_reply is None:
            expects_reply = opcode in REPLY_OPCODES
        if not expects_reply:
            return None
        deadline = time.monotonic() + self.timeout_ms / 1000.0
        while True:
            remaining_ms = (deadline - time.monotonic()) * 1000.0
            if remaining_ms <= 0:
                raise GdbError(
                    ErrorCode.TIMEOUT,
                    f"no reply to opcode {opcode} within {self.timeout_ms} ms",
                )
            frame = self.transport.read(max(1, int(remaining_ms)))
            if frame is None:
                continue
            packet = decode_packet(frame, IN_ID)
            if packet.session != self.id:
                _trace(
                    f"skip opcode={packet.opcode} session={packet.session} "
                    f"sequence={packet.sequence} (not ours)"
                )
                continue
            _trace(
                f"in opcode={packet.opcode} session={packet.session} "
                f"sequence={packet.sequence} payload={_payload_debug(packet.payload)}"
            )
            if packet.opcode == EVENT:
                raise error_from_result(packet.payload[0] if packet.payload else 0)
            if packet.opcode != (opcode | 0x80) or packet.sequence != sequence:
                continue
            if not packet.payload:
                raise GdbError(ErrorCode.FRAMING, "empty control reply")
            if opcode != SESSION_LIST and packet.payload[0] != 0:
                raise error_from_result(packet.payload[0])
            return packet

    def close(self) -> None:
        self.command(CLOSE)

    def heartbeat(self) -> None:
        self.command(HEARTBEAT)

    def barrier(self) -> None:
        self.command(BARRIER)

    def release_all(self) -> None:
        self.command(RELEASE_ALL)

    def bootloader(self) -> None:
        self.command(BOOTLOADER)

    # -- input helpers (mirror clients/python/hid_v3.py) -------------------

    def key(self, page: int, usage: int, down: bool) -> None:
        self.command(KEY, struct.pack("<HHB", page, usage, 1 if down else 0), False)

    def snapshot(self, state: bytes) -> None:
        if len(state) != SNAPSHOT_BYTES:
            raise GdbError(ErrorCode.INVALID, "snapshot is 31 bytes")
        self.command(SNAPSHOT, bytes(state), False)

    def mouse_move(self, dx: int, dy: int) -> None:
        self.command(MOUSE_MOVE, struct.pack("<hh", dx, dy), False)

    def buttons(self, mask: int) -> None:
        self.command(MOUSE_BUTTONS, bytes([mask & 0xFF]), False)

    def wheel(self, vertical: int, horizontal: int = 0) -> None:
        self.command(WHEEL, struct.pack("<hh", vertical, horizontal), False)

    def __del__(self):  # pragma: no cover - best effort
        try:
            if self.transport is not None:
                # Best effort: the lease is the backstop when this fails.
                self.transport.write(
                    encode_packet(CLOSE, self.id, min(self.sequence + 1, 0xFFFFFFFF))
                )
        except Exception:
            pass


# --- movement -------------------------------------------------------------


class _MovementShared:
    def __init__(self):
        self._cancel = False
        self._target = None
        self._lock = threading.Lock()

    def stop(self) -> None:
        self._cancel = True

    @property
    def cancelled(self) -> bool:
        return self._cancel

    def retarget(self, x: int, y: int) -> None:
        with self._lock:
            self._target = (x, y)

    def take_target(self):
        with self._lock:
            target, self._target = self._target, None
            return target


class Movement:
    """Handle for a running move; use as a context manager or call ``wait``."""

    def __init__(self, shared: _MovementShared, thread: threading.Thread):
        self._shared = shared
        self._thread = thread
        self._error = None

    def _run(self, work) -> None:
        try:
            work()
        except GdbError as error:
            self._error = error
        except Exception as error:  # pragma: no cover - defensive
            self._error = GdbError(ErrorCode.IO, str(error))

    def cancel(self) -> None:
        self._shared.stop()

    def retarget(self, x: int, y: int) -> None:
        self._shared.retarget(x, y)

    @property
    def finished(self) -> bool:
        return not self._thread.is_alive()

    def wait(self, timeout_ms: int = 10_000) -> None:
        self._thread.join(max(1, timeout_ms) / 1000.0)
        if self._thread.is_alive():
            raise GdbError(ErrorCode.TIMEOUT, "movement did not finish in time")
        if self._error is not None:
            error, self._error = self._error, None
            raise error

    def close(self) -> None:
        self._shared.stop()
        self._thread.join(1.0)

    def __enter__(self) -> "Movement":
        return self

    def __exit__(self, *exc_info) -> None:
        self.close()

    def __del__(self):  # pragma: no cover - best effort
        try:
            self._shared.stop()
        except Exception:
            pass


# --- device ---------------------------------------------------------------


def _clamp_target(x: int, y: int, screen: Screen) -> Point:
    return (
        max(0, min(screen.width - 1, int(x))),
        max(0, min(screen.height - 1, int(y))),
    )


class Device:
    """One controller session.

    ``info`` is a property (like the ctypes binding); the queries are methods
    (``status()``, ``status_ext()``, ``sessions()``) that return typed objects.
    Those objects also answer the binding's JSON key names, so
    ``device.status()["leaseMs"]`` keeps working.
    """

    HEARTBEAT_MS = HEARTBEAT_MS
    LEASE_MS = LEASE_MS

    def __init__(
        self,
        transport: Transport,
        *,
        target=None,
        serial: str = "",
        product: str = "",
        timeout_ms: int = 1000,
        auto_reconnect: bool = True,
        heartbeat: bool = True,
        heartbeat_ms: int = HEARTBEAT_MS,
    ):
        self._lock = threading.RLock()
        self._target = target or ("mock", "")
        self._options = {
            "timeout_ms": max(100, int(timeout_ms)),
            "auto_reconnect": bool(auto_reconnect),
            "heartbeat": bool(heartbeat),
        }
        self._auto_reconnect = bool(auto_reconnect)
        self._heartbeat_ms = max(5, int(heartbeat_ms))
        self._closed = False
        self._session: Session | None = None
        self._buttons = 0
        self._cursor = [960, 540]
        self._handler = None
        self._screen_source = None
        self._stop = threading.Event()
        self._heartbeat_thread = None

        session = Session.open(transport, self._options["timeout_ms"])
        capabilities = session.capabilities
        self._session = session
        self._info = {
            "serial": serial,
            "product": product,
            "board": capabilities.board,
            "firmware": capabilities.firmware,
            "leaseMs": capabilities.lease_ms,
            "pollMs": capabilities.poll_ms,
            "capabilities": capabilities.flags,
            "deviceId": list(capabilities.device_id),
            "build": capabilities.build,
        }
        self.mouse = Mouse(self)
        self.keyboard = Keyboard(self)
        if self._options["heartbeat"]:
            self._start_heartbeat()
        _log(
            Level.INFO,
            f"connected serial={serial} firmware={capabilities.firmware} board={capabilities.board}",
        )

    # -- construction ------------------------------------------------------

    @classmethod
    def open(
        cls,
        selector: str | None = None,
        timeout_ms: int = 1000,
        auto_reconnect: bool = True,
        heartbeat: bool = True,
    ) -> "Device":
        """Opens by USB serial or HID path; ``None`` picks the first device."""
        if selector and (selector.startswith("\\\\?\\") or selector.lower().startswith("hid#")):
            transport = HidTransport(selector, write_timeout_ms=timeout_ms)
            serial = transport.serial_number()
            product = transport.product() or "Vendor HID"
            return cls(
                transport,
                target=("path", selector),
                serial=serial,
                product=product,
                timeout_ms=timeout_ms,
                auto_reconnect=auto_reconnect,
                heartbeat=heartbeat,
            )
        entries = list_devices()
        if selector:
            entry = next(
                (item for item in entries if item["serial"] == selector or item["path"] == selector),
                None,
            )
            if entry is None:
                entry = next(
                    (item for item in entries if selector in item["serial"]), None
                )
            if entry is None:
                raise GdbError(
                    ErrorCode.DISCONNECTED, f"no GDBOX device matches {selector!r}"
                )
        else:
            if not entries:
                raise GdbError(ErrorCode.DISCONNECTED, "no GDBOX device found")
            entry = entries[0]
        transport = HidTransport(entry["path"], write_timeout_ms=timeout_ms)
        return cls(
            transport,
            target=("serial", entry["serial"]),
            serial=entry["serial"],
            product=entry["product"],
            timeout_ms=timeout_ms,
            auto_reconnect=auto_reconnect,
            heartbeat=heartbeat,
        )

    @classmethod
    def open_mock(
        cls,
        timeout_ms: int = 1000,
        auto_reconnect: bool = True,
        heartbeat: bool = True,
        transport: MockTransport | None = None,
        heartbeat_ms: int = HEARTBEAT_MS,
    ) -> "Device":
        """A device backed by ``MockTransport``; no hardware involved."""
        return cls(
            transport if transport is not None else MockTransport(),
            target=("mock", ""),
            serial="MOCK",
            product="GDBOX mock",
            timeout_ms=timeout_ms,
            auto_reconnect=auto_reconnect,
            heartbeat=heartbeat,
            heartbeat_ms=heartbeat_ms,
        )

    @staticmethod
    def list_devices() -> list:
        return list_devices()

    # -- properties --------------------------------------------------------

    @property
    def info(self) -> dict:
        if self._closed:
            raise GdbError(ErrorCode.INVALID, "device is closed")
        return dict(self._info)

    @property
    def state(self) -> int:
        if self._closed:
            return ConnectionState.DISCONNECTED
        return ConnectionState.CONNECTED if self._session else ConnectionState.DISCONNECTED

    @property
    def cursor(self) -> Point:
        """Simulated pointer of a mock device (real cursor otherwise)."""
        return (self._cursor[0], self._cursor[1])

    def set_screen_source(self, source: Callable[[], tuple] | None) -> None:
        """Overrides where screen size and cursor come from (``() -> (Screen, (x, y))``)."""
        self._screen_source = source

    def set_event_handler(self, callback: Callable[[int, int, str], None] | None) -> None:
        """``callback(kind, code, message)`` — see ``EventKind``.

        ``EventKind.STATE`` events carry a ``ConnectionState`` and no message;
        ``EventKind.ERROR`` events carry an ``ErrorCode`` and its message. The
        callback runs on whichever SDK thread noticed the change, so it must not
        block.
        """
        self._handler = callback

    def set_auto_reconnect(self, enabled: bool) -> None:
        self._auto_reconnect = bool(enabled)

    def scheduler(self) -> "Scheduler":
        return Scheduler(self)

    # -- queries -----------------------------------------------------------

    def status(self) -> Status:
        def op(session: Session) -> Status:
            reply = session.command(STATUS)
            if reply is None:
                raise GdbError(ErrorCode.FRAMING, "STATUS had no reply")
            return Status.parse(reply.payload)

        return self._with_session(op)

    def status_ext(self) -> StatusExt:
        def op(session: Session) -> StatusExt:
            reply = session.command(STATUS_EXT)
            if reply is None:
                raise GdbError(ErrorCode.FRAMING, "STATUS_EXT had no reply")
            return StatusExt.parse(reply.payload)

        return self._with_session(op)

    def sessions(self) -> list:
        def op(session: Session) -> list:
            reply = session.command(SESSION_LIST)
            if reply is None:
                raise GdbError(ErrorCode.FRAMING, "SESSION_LIST had no reply")
            return parse_session_list(reply.payload)

        return self._with_session(op)

    # -- control -----------------------------------------------------------

    def barrier(self) -> None:
        self._with_session(lambda session: session.command(BARRIER))

    def release_all(self) -> None:
        self._buttons = 0
        self._with_session(lambda session: session.command(RELEASE_ALL))

    def bootloader(self) -> None:
        self._with_session(lambda session: session.command(BOOTLOADER))

    def reconnect(self) -> None:
        with self._lock:
            self._reopen()

    def command(self, opcode: int, payload: bytes = b"", expects_reply: bool | None = None):
        """Raw escape hatch: skips the SDK bookkeeping, keeps sequencing."""
        return self._with_session(
            lambda session: session.command(opcode, payload, expects_reply)
        )

    def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        self._stop.set()
        thread = self._heartbeat_thread
        if thread is not None and thread is not threading.current_thread():
            thread.join(2.0)
        self._buttons = 0
        _log(Level.INFO, f"closed serial={self._info['serial']}")
        with self._lock:
            session, self._session = self._session, None
        self._notify(0, ConnectionState.DISCONNECTED, "")
        if session is not None:
            try:
                session.close()
            except GdbError as error:
                # The lease is the backstop when CLOSE cannot be delivered.
                _log(Level.WARN, f"close failed ({error.code}): {error.message}")

    # -- internals ---------------------------------------------------------

    def _notify(self, kind: int, code: int, message: str) -> None:
        handler = self._handler
        if handler is None:
            return
        try:
            handler(kind, code, message)
        except Exception:  # a broken callback must not break the SDK
            pass

    def _notify_error(self, error: GdbError) -> None:
        self._notify(EventKind.ERROR, error.code, error.message)

    def _with_session(self, operation: Callable[[Session], object]):
        if self._closed:
            raise GdbError(ErrorCode.DISCONNECTED, "device is closed")
        with self._lock:
            session = self._session
            if session is not None:
                try:
                    return operation(session)
                except GdbError as error:
                    _log(Level.WARN, f"command failed ({error.code}): {error.message}")
                    if not recoverable(error.code) or not self._auto_reconnect:
                        self._notify_error(error)
                        raise
            try:
                self._reopen()
            except GdbError as error:
                self._closed = True
                self._notify_error(error)
                raise
            current = self._session
            if current is None:
                raise GdbError(ErrorCode.DISCONNECTED, "no session after reconnect")
            try:
                return operation(current)
            except GdbError as error:
                self._notify_error(error)
                raise

    def _reopen(self) -> None:
        self._session = None
        transport = _open_transport(self._target, self._options["timeout_ms"])
        session = Session.open(transport, self._options["timeout_ms"])
        self._session = session
        _log(Level.INFO, f"reconnected serial={self._info['serial']}")
        self._notify(0, ConnectionState.CONNECTED, "")

    def _start_heartbeat(self) -> None:
        tick = max(0.005, min(0.1, self._heartbeat_ms / 1000.0 / 4.0))

        def loop() -> None:
            while not self._stop.wait(tick):
                if self._closed:
                    return
                if not self._lock.acquire(blocking=False):
                    continue
                failed = None
                try:
                    if self._closed:
                        return
                    session = self._session
                    if session is None:
                        continue
                    # Ask the session, not the caller: STATUS and BARRIER do not
                    # renew the device-side lease, so a host polling STATUS in a
                    # loop must still heartbeat.
                    if session.renew_age_ms() < self._heartbeat_ms:
                        continue
                    try:
                        session.heartbeat()
                    except GdbError as error:
                        failed = error
                finally:
                    self._lock.release()
                if failed is not None:
                    self._notify_error(failed)
                    if recoverable(failed.code) and self._auto_reconnect:
                        try:
                            self.reconnect()
                        except GdbError as error:
                            self._notify_error(error)

        thread = threading.Thread(target=loop, name="gdb-heartbeat", daemon=True)
        self._heartbeat_thread = thread
        thread.start()

    def _is_mock(self) -> bool:
        return self._target[0] == "mock"

    def _cursor_state(self) -> tuple:
        if self._screen_source is not None:
            return self._screen_source()
        if self._is_mock():
            return Screen(), (int(self._cursor[0]), int(self._cursor[1]))
        return screen_and_cursor()

    def _send_mouse_move(self, dx: int, dy: int) -> None:
        self._with_session(lambda session: session.mouse_move(dx, dy))
        if self._is_mock():
            self._cursor[0] = max(0, min(1919, self._cursor[0] + int(dx)))
            self._cursor[1] = max(0, min(1079, self._cursor[1] + int(dy)))

    def _buttons_mask(self) -> int:
        return self._buttons

    def _set_buttons(self, mask: int) -> None:
        if mask & ~0x1F & 0xFF:
            raise GdbError(ErrorCode.INVALID, "buttons are bits 0-4")
        self._buttons = mask

    def __enter__(self) -> "Device":
        return self

    def __exit__(self, *exc_info) -> None:
        self.close()

    def __del__(self):  # pragma: no cover - best effort
        try:
            self._stop.set()
        except Exception:
            pass


def recoverable(code: int) -> bool:
    return code in (ErrorCode.DISCONNECTED, ErrorCode.IO, ErrorCode.LEASE)


def _open_transport(target, timeout_ms: int) -> Transport:
    kind, value = target
    if kind == "mock":
        return MockTransport()
    if kind == "path":
        return HidTransport(value, write_timeout_ms=timeout_ms)
    entries = list_devices()
    entry = next((item for item in entries if item["serial"] == value), None)
    if entry is None:
        raise GdbError(ErrorCode.DISCONNECTED, f"device {value} is not present")
    return HidTransport(entry["path"], write_timeout_ms=timeout_ms)


# --- input ----------------------------------------------------------------


class Keyboard:
    def __init__(self, device: Device):
        self._device = device

    def key_down(self, page: int, usage: int) -> None:
        self._emit(page, usage, True)

    def key_up(self, page: int, usage: int) -> None:
        self._emit(page, usage, False)

    def press(self, page: int, usage: int) -> None:
        self.long_press(page, usage, 18)

    def long_press(self, page: int, usage: int, hold_ms: int) -> None:
        self.key_down(page, usage)
        time.sleep(max(1, int(hold_ms)) / 1000.0)
        self.key_up(page, usage)

    def chord(self, usages: Sequence[int], hold_ms: int = 60) -> None:
        """Presses every usage, holds, then releases in reverse order."""
        if not usages:
            raise GdbError(ErrorCode.INVALID, "a chord needs at least one usage")
        for usage in usages:
            self.key_down(7, usage)
        time.sleep(max(1, int(hold_ms)) / 1000.0)
        for usage in reversed(list(usages)):
            self.key_up(7, usage)

    def sequence(self, steps: Sequence[KeyStep]) -> None:
        for step in steps:
            self.long_press(step.page, step.usage, step.hold_ms)
            if step.gap_ms > 0:
                time.sleep(int(step.gap_ms) / 1000.0)

    def type_text(
        self, text: str, interval_ms: int = 45, hold_ms: int = 18, jitter_ms: int = 30
    ) -> None:
        rng = SplitMix64.from_entropy()
        for stroke in strokes(text):
            if stroke.shift:
                self.key_down(7, LEFT_SHIFT)
            self.key_down(stroke.page, stroke.usage)
            time.sleep(max(1, int(hold_ms)) / 1000.0)
            self.key_up(stroke.page, stroke.usage)
            if stroke.shift:
                self.key_up(7, LEFT_SHIFT)
            jitter = 0 if jitter_ms == 0 else int(rng.unit() * float(jitter_ms))
            time.sleep((int(interval_ms) + jitter) / 1000.0)

    def release_all(self) -> None:
        self._device.release_all()

    def _emit(self, page: int, usage: int, down: bool) -> None:
        if (page, usage) not in KEY_CATALOG:
            raise GdbError(
                ErrorCode.INVALID,
                f"usage {page:#06x}:{usage:#06x} is not in the key catalog",
            )
        self._device._with_session(lambda session: session.key(page, usage, down))


class Mouse:
    def __init__(self, device: Device):
        self._device = device

    def move_by(
        self, dx: int, dy: int, duration_ms: int = 0, profile: Profile | None = None
    ) -> None:
        self._run_move_by(dx, dy, duration_ms, profile or Profile(), _MovementShared())

    def move_raw(self, dx: int, dy: int) -> None:
        """One relative move, split only at the ±1024 HID limit, no barrier."""
        for chunk_x, chunk_y in split_delta(dx, dy):
            self._device._with_session(lambda session: session.mouse_move(chunk_x, chunk_y))

    def move_to(
        self, x: int, y: int, duration_ms: int = 0, profile: Profile | None = None
    ) -> None:
        self._run_move_to(x, y, duration_ms, profile or Profile(), _MovementShared())

    def move_path(
        self, points: Sequence[Point], duration_ms: int = 0, profile: Profile | None = None
    ) -> None:
        if not points:
            raise GdbError(ErrorCode.INVALID, "path needs at least one point")
        shared = _MovementShared()
        for x, y in points:
            self._run_move_to(x, y, duration_ms, profile or Profile(), shared)

    def start_move_to(
        self, x: int, y: int, duration_ms: int = 0, profile: Profile | None = None
    ) -> Movement:
        return self._spawn(
            lambda shared: self._run_move_to(x, y, duration_ms, profile or Profile(), shared)
        )

    def start_move_by(
        self, dx: int, dy: int, duration_ms: int = 0, profile: Profile | None = None
    ) -> Movement:
        return self._spawn(
            lambda shared: self._run_move_by(dx, dy, duration_ms, profile or Profile(), shared)
        )

    def start_move_path(
        self, points: Sequence[Point], duration_ms: int = 0, profile: Profile | None = None
    ) -> Movement:
        if not points:
            raise GdbError(ErrorCode.INVALID, "path needs at least one point")
        snapshot = list(points)

        def work(shared: _MovementShared) -> None:
            for x, y in snapshot:
                self._run_move_to(x, y, duration_ms, profile or Profile(), shared)

        return self._spawn(work)

    def button(self, button: int, down: bool) -> None:
        if button not in (Button.LEFT, Button.RIGHT, Button.MIDDLE, Button.BACK, Button.FORWARD):
            raise GdbError(ErrorCode.INVALID, "button must be 1..=5")
        # Each session owns its own button mask; the firmware unions the owners.
        bit = 1 << (button - 1)
        current = self._device._buttons_mask()
        nxt = (current | bit) if down else (current & ~bit & 0xFF)
        self._device._set_buttons(nxt)
        self._device._with_session(lambda session: session.buttons(nxt))

    def click(self, button: int = Button.LEFT, count: int = 1) -> None:
        if count < 1:
            raise GdbError(ErrorCode.INVALID, "click count must be at least 1")
        for _ in range(int(count)):
            self.button(button, True)
            time.sleep(0.025)
            self.button(button, False)
            time.sleep(0.035)

    def double_click(self, button: int = Button.LEFT) -> None:
        self.click(button, 2)

    def wheel(self, vertical: int, horizontal: int = 0) -> None:
        if not (-1024 <= vertical <= 1024 and -1024 <= horizontal <= 1024):
            raise GdbError(ErrorCode.INVALID, "wheel is limited to +/-1024 per axis")
        self._device._with_session(
            lambda session: session.wheel(int(vertical), int(horizontal))
        )

    def drag(
        self,
        button: int,
        x: int,
        y: int,
        duration_ms: int = 0,
        profile: Profile | None = None,
    ) -> None:
        self.button(button, True)
        try:
            self.move_to(x, y, duration_ms, profile)
        finally:
            try:
                self._device.release_all()
            except GdbError:
                pass

    # -- workers -----------------------------------------------------------

    def _spawn(self, work) -> Movement:
        shared = _MovementShared()
        placeholder = threading.Thread(name="gdb-movement", daemon=True)
        movement = Movement(shared, placeholder)

        def target() -> None:
            movement._run(lambda: work(shared))

        movement._thread = threading.Thread(target=target, name="gdb-movement", daemon=True)
        movement._thread.start()
        return movement

    def _run_move_by(
        self, dx: int, dy: int, duration_ms: int, profile: Profile, shared: _MovementShared
    ) -> None:
        chunks = split_delta(dx, dy)
        if not chunks:
            return
        pause = 0.0 if duration_ms == 0 else (int(duration_ms) / len(chunks)) / 1000.0
        for index, (chunk_x, chunk_y) in enumerate(chunks):
            if shared.cancelled:
                raise GdbError(ErrorCode.CANCELLED, ErrorCode.MESSAGES[9])
            self._device._send_mouse_move(chunk_x, chunk_y)
            self._device.barrier()
            if index + 1 != len(chunks) and pause > 0:
                time.sleep(pause)

    def _run_move_to(
        self, x: int, y: int, duration_ms: int, profile: Profile, shared: _MovementShared
    ) -> None:
        device = self._device
        enable_dpi_awareness()
        screen, position = device._cursor_state()
        if (
            position[0] < 0
            or position[1] < 0
            or position[0] >= screen.width
            or position[1] >= screen.height
        ):
            raise GdbError(
                ErrorCode.UNSUPPORTED,
                "absolute coordinates are relative to the primary monitor "
                f"({screen.width}x{screen.height} at 0,0) and the cursor is at "
                f"({position[0]}, {position[1]}), so the move would start off that screen",
            )
        target = _clamp_target(x, y, screen)
        rng = SplitMix64.from_entropy()
        nodes = trajectory_with(position, target, screen, duration_ms, profile, rng)
        started = time.monotonic()
        index = 0
        tracker = RelativeTracker()
        deadline = started + (nodes[-1].at_ms if nodes else 0) / 1000.0 + 0.5
        while True:
            if shared.cancelled:
                raise GdbError(ErrorCode.CANCELLED, ErrorCode.MESSAGES[9])
            retarget = shared.take_target()
            if retarget is not None:
                target = _clamp_target(retarget[0], retarget[1], screen)
                current_screen, current = device._cursor_state()
                if (current_screen.width, current_screen.height) != (screen.width, screen.height):
                    raise GdbError(
                        ErrorCode.UNSUPPORTED,
                        "monitor layout changed during the movement "
                        f"({screen.width}x{screen.height} -> "
                        f"{current_screen.width}x{current_screen.height})",
                    )
                nodes = trajectory_with(current, target, screen, duration_ms, profile, rng)
                started = time.monotonic()
                index = 0
                tracker = RelativeTracker()
                deadline = started + (nodes[-1].at_ms if nodes else 0) / 1000.0 + 0.5
            if index >= len(nodes):
                return
            node = nodes[index]
            elapsed_ms = (time.monotonic() - started) * 1000.0
            if elapsed_ms < node.at_ms:
                time.sleep(min((node.at_ms - elapsed_ms) / 1000.0, 0.002))
                continue
            _, position = device._cursor_state()
            step = tracker.step(position, (node.x, node.y))
            if step.kind == STEP_WAIT:
                time.sleep(0.001)
            elif step.kind == STEP_MOVE:
                device._send_mouse_move(step.dx, step.dy)
                device.barrier()
            else:
                if index + 1 == len(nodes):
                    device.barrier()
                    return
                index += 1
            if time.monotonic() > deadline:
                raise GdbError(ErrorCode.TIMEOUT, "pointer did not reach the target")


# --- scheduler ------------------------------------------------------------


class Scheduler:
    """Task queue with priorities, delays, repeat, pause/resume and cancel.

    Jobs run on one worker thread in ``(due, priority desc, submission order)``.
    """

    MAX_ERRORS = 32

    def __init__(self, device: Device):
        self._device = device
        self._cv = threading.Condition()
        self._heap = []
        self._ids = itertools.count(1)
        self._seq = itertools.count(1)
        self._cancelled = set()
        self._errors = []
        self._waiters = []
        self._paused = False
        self._stop = False
        self._started = time.monotonic()
        self._thread = threading.Thread(target=self._run, name="gdb-scheduler", daemon=True)
        self._thread.start()

    # -- api ---------------------------------------------------------------

    def submit(self, delay_ms: int, priority: int, callback: Callable[[], int]) -> int:
        return self._push(max(0, int(delay_ms)), int(priority), None, callback)

    def every(self, interval_ms: int, priority: int, callback: Callable[[], int]) -> int:
        if int(interval_ms) <= 0:
            raise GdbError(ErrorCode.INVALID, "interval must be positive")
        return self._push(int(interval_ms), int(priority), int(interval_ms), callback)

    def cancel(self, task_id: int) -> None:
        with self._cv:
            # Inserting here (not only in the worker) means a cancel that races a
            # due job still wins as long as the callback has not started.
            self._cancelled.add(int(task_id))
            self._cv.notify_all()

    def pause(self) -> None:
        with self._cv:
            self._paused = True
            self._cv.notify_all()

    def resume(self) -> None:
        with self._cv:
            self._paused = False
            self._cv.notify_all()

    def wait_idle(self, timeout_ms: int = 5000) -> None:
        with self._cv:
            if not self._heap:
                return
            event = threading.Event()
            self._waiters.append(event)
            self._cv.notify_all()
        if not event.wait(max(1, int(timeout_ms)) / 1000.0):
            with self._cv:
                if event in self._waiters:
                    self._waiters.remove(event)
            raise GdbError(ErrorCode.TIMEOUT, "scheduler did not become idle")

    def take_errors(self) -> list:
        with self._cv:
            errors, self._errors = self._errors, []
        return errors

    def shutdown(self) -> None:
        with self._cv:
            if self._stop:
                return
            self._stop = True
            self._cv.notify_all()
            waiters, self._waiters = self._waiters, []
            for waiter in waiters:
                waiter.set()
        if self._thread is not threading.current_thread():
            self._thread.join(5.0)

    def close(self) -> None:
        self.shutdown()

    def __enter__(self) -> "Scheduler":
        return self

    def __exit__(self, *exc_info) -> None:
        self.close()

    def __del__(self):  # pragma: no cover - best effort
        try:
            self.shutdown()
        except Exception:
            pass

    # -- worker ------------------------------------------------------------

    def _now_ms(self) -> float:
        return (time.monotonic() - self._started) * 1000.0

    def _push(self, delay_ms: int, priority: int, interval: int | None, callback) -> int:
        task_id = next(self._ids)
        seq = next(self._seq)
        due = int(self._now_ms()) + delay_ms
        with self._cv:
            heapq.heappush(self._heap, (due, -priority, seq, task_id, interval, callback))
            self._cv.notify_all()
        return task_id

    def _release_waiters(self) -> None:
        for waiter in self._waiters:
            waiter.set()
        self._waiters.clear()

    def _record_error(self, task_id: int, code: int, message: str) -> None:
        if len(self._errors) >= self.MAX_ERRORS:
            self._errors.pop(0)
        self._errors.append(
            {"taskId": task_id, "code": int(code), "message": message}
        )

    def _run(self) -> None:
        while True:
            with self._cv:
                if self._stop:
                    self._release_waiters()
                    return
                job = None
                if not self._paused:
                    now = self._now_ms()
                    while self._heap and self._heap[0][0] <= now:
                        candidate = heapq.heappop(self._heap)
                        if candidate[3] in self._cancelled:
                            self._cancelled.discard(candidate[3])
                            continue
                        job = candidate
                        break
                if job is None:
                    if not self._heap:
                        self._release_waiters()
                    self._cv.wait(0.02)
                    continue
            due, negative_priority, seq, task_id, interval, callback = job
            code, message = 0, ""
            try:
                result = callback()
                if isinstance(result, int) and result != 0:
                    code = result
                    message = ErrorCode.MESSAGES.get(result, f"task failed ({result})")
            except GdbError as error:
                code, message = error.code, error.message
            except Exception as error:  # pragma: no cover - defensive
                code, message = ErrorCode.IO, str(error)
            with self._cv:
                if code:
                    self._record_error(task_id, code, message)
                if interval:
                    if task_id in self._cancelled:
                        self._cancelled.discard(task_id)
                    else:
                        nxt = due + interval
                        now = self._now_ms()
                        while nxt <= now:
                            nxt += interval
                        heapq.heappush(
                            self._heap,
                            (nxt, negative_priority, seq, task_id, interval, callback),
                        )
                if not self._heap:
                    self._release_waiters()


# --- enumeration ----------------------------------------------------------


def _probe_entry(path: str) -> dict:
    transport = HidTransport(path)
    try:
        report = transport.feature()
        if not report:
            raise GdbError(ErrorCode.DISCONNECTED, "collection does not answer Feature 14")
        Capabilities.parse(report)
        vid, pid = transport.attributes()
        return {
            "path": path,
            "serial": transport.serial_number(),
            "product": transport.product() or "Vendor HID",
            "manufacturer": transport.manufacturer(),
            "vid": vid or VENDOR_ID,
            "pid": pid or PRODUCT_ID,
        }
    finally:
        transport.close()


def list_devices() -> list:
    """Connected GDBOX devices: ``[{path, serial, product, manufacturer, vid, pid}]``."""
    if sys.platform != "win32":
        raise GdbError(ErrorCode.UNSUPPORTED, "physical HID is only implemented on Windows")
    candidates = [interface_path(instance) for instance in _instance_ids()]
    entries = []
    for path in candidates:
        try:
            entries.append(_probe_entry(path))
        except GdbError:
            # Not the vendor collection (or held by another process); the MI_01
            # heuristic below still yields a usable path.
            continue
    if entries:
        return entries
    fallback = [path for path in candidates if "&mi_01" in path]
    if not fallback:
        return []
    return [
        {
            "path": path,
            "serial": "",
            "product": "Vendor HID",
            "manufacturer": "",
            "vid": VENDOR_ID,
            "pid": PRODUCT_ID,
        }
        for path in fallback
    ]


# --- firmware build and flashing (sdk/rust/src/flash.rs) -------------------
#
# Three details learned on hardware decide the shape of this section:
#
# * the board refuses BOOTLOADER while a host still polls it, so the runtime
#   session is closed *before* the drive is waited for;
# * the UF2 drive only appears after that reboot, so "was it mounted before this
#   command" — not its letter — is what identifies the right one;
# * verification is a second enumeration plus a Feature 14 read, not the reply of
#   the copy: the old handle is gone by the time the board is back.
#
# Nothing here knows about a window. cargo comes in through ``runner``, progress
# through ``progress`` and cancellation through ``cancel``, so the same sequence
# runs from any host program; the messages are the ones a person reads while
# flashing, hence Chinese.


APP_CRATE = "xiao-firmware"
FLASH_TARGET = "thumbv6m-none-eabi"
FLASH_FEATURES = "hardware"
BOARD_ID = 2

#: Family id RP2040 writes into every UF2 block.
FAMILY_RP2040 = 0xE48BFF56
XIP_BASE = 0x10000000
XIP_LIMIT = 0x11000000
UF2_BLOCK = 512
UF2_PAYLOAD = 256
UF2_MAGIC_START = (0x0A324655, 0x9E5D5157)
UF2_MAGIC_END = 0x0AB16F30
UF2_FLAG_FAMILY = 0x00002000

#: How long the UF2 drive may take to appear after the reboot.
DRIVE_WAIT_S = 30.0
#: How long the runtime HID may take to come back and answer Feature 14.
HANDSHAKE_WAIT_S = 15.0
#: Copy chunk: a cancelled transfer stops between chunks, not mid-block.
COPY_CHUNK = 16 * 1024
#: Poll interval for "is it there yet" loops.
POLL_S = 0.25

#: What a host shows the user: stage 1 toolchain … stage 7 verify.
STAGES = {
    1: "檢查 Rust 工具鏈",
    2: "安裝 Rust 目標",
    3: "編譯 Rust 韌體",
    4: "打包 UF2",
    5: "進入並選擇 bootloader",
    6: "傳輸韌體",
    7: "等待 runtime HID 重新列舉與 hello 驗證",
}


class Stream:
    """Which pipe a child-program line came from."""

    STDOUT = "Stdout"
    STDERR = "Stderr"


def _elf_u16(data: bytes, at: int) -> int:
    if at + 2 > len(data):
        raise GdbError(ErrorCode.INVALID, "ELF 太短")
    return struct.unpack_from("<H", data, at)[0]


def _elf_u32(data: bytes, at: int) -> int:
    if at + 4 > len(data):
        raise GdbError(ErrorCode.INVALID, "ELF 太短")
    return struct.unpack_from("<I", data, at)[0]


def pack_elf(elf: bytes) -> bytes:
    """Packs the flash image of an RP2040 ELF into UF2 bytes.

    Two details matter and both were learned the hard way: every block must be
    256-byte aligned (the BOOTSEL bootloader silently ignores a file that is
    not), and the flash segments have to be flattened into one image because a
    segment's file order is not its address order — ``.boot2`` regularly comes
    last while it has to be at ``0x10000000``.
    """
    elf = bytes(elf)
    if len(elf) <= 52 or elf[:4] != b"\x7fELF":
        raise GdbError(ErrorCode.INVALID, "不是 ELF 檔")
    if elf[4] != 1 or elf[5] != 1:
        raise GdbError(ErrorCode.INVALID, "只支援 32-bit little-endian ELF")
    phoff = _elf_u32(elf, 28)
    phentsize = _elf_u16(elf, 42)
    phnum = _elf_u16(elf, 44)

    segments = []
    for index in range(phnum):
        base = phoff + index * phentsize
        if base + 32 > len(elf):
            raise GdbError(ErrorCode.INVALID, "program header 超出檔案")
        kind = _elf_u32(elf, base)
        offset = _elf_u32(elf, base + 4)
        paddr = _elf_u32(elf, base + 12)
        filesz = _elf_u32(elf, base + 16)
        if kind != 1 or filesz == 0 or not XIP_BASE <= paddr < XIP_LIMIT:
            continue
        end = offset + filesz
        if end > len(elf):
            raise GdbError(ErrorCode.INVALID, "segment 超出檔案")
        segments.append((paddr, elf[offset:end]))
    if not segments:
        raise GdbError(ErrorCode.INVALID, "ELF 沒有任何 flash segment；這不是 RP2040 建置？")
    segments.sort(key=lambda segment: segment[0])

    # Flatten to one 256-byte aligned image; gaps stay erased (0xFF).
    start = segments[0][0] & ~(UF2_PAYLOAD - 1)
    end = max(address + len(data) for address, data in segments)
    end = (end + UF2_PAYLOAD - 1) & ~(UF2_PAYLOAD - 1)
    image = bytearray(b"\xff" * (end - start))
    for address, data in segments:
        at = address - start
        image[at : at + len(data)] = data

    total = len(image) // UF2_PAYLOAD
    blocks = bytearray()
    for number in range(total):
        payload = image[number * UF2_PAYLOAD : (number + 1) * UF2_PAYLOAD]
        block = bytearray()
        for value in (
            UF2_MAGIC_START[0],
            UF2_MAGIC_START[1],
            UF2_FLAG_FAMILY,
            start + number * UF2_PAYLOAD,
            UF2_PAYLOAD,
            number,
            total,
            FAMILY_RP2040,
        ):
            block += struct.pack("<I", value)
        block += payload
        block += b"\x00" * (UF2_BLOCK - 4 - len(block))
        block += struct.pack("<I", UF2_MAGIC_END)
        blocks += block
    return bytes(blocks)


def uf2_blocks(uf2: bytes) -> int:
    """Block count of a packed image; the caller logs it, the tests assert on it."""
    return len(uf2) // UF2_BLOCK


@dataclass(frozen=True)
class Image:
    """A packed image: the bytes the bootloader drive expects and their blocks."""

    bytes: bytes = b""
    blocks: int = 0

    @classmethod
    def from_uf2(cls, packed: bytes) -> "Image":
        return cls(bytes=bytes(packed), blocks=uf2_blocks(packed))

    @classmethod
    def from_elf(cls, elf: bytes) -> "Image":
        return cls.from_uf2(pack_elf(elf))

    def __len__(self) -> int:
        return len(self.bytes)


@dataclass
class ProcessOutcome:
    """What a finished child program reported."""

    stdout: str = ""
    stderr: str = ""
    exit: int | None = None
    timed_out: bool = False
    cancelled: bool = False

    def check(self) -> str:
        """Returns stdout when the program succeeded, a usable message when not."""
        if self.cancelled:
            raise GdbError(ErrorCode.CANCELLED, "操作已取消")
        if self.timed_out:
            raise GdbError(ErrorCode.TIMEOUT, "子程序逾時")
        if self.exit != 0:
            raise GdbError(
                ErrorCode.IO,
                f"子程序失敗，exit={self.exit}\n{self.stderr}\n{self.stdout}",
            )
        return self.stdout


@dataclass
class Outcome:
    """Result of ``push``: the copy finished, and whether the board answered.

    ``verified`` is what the caller reports to the user; ``error`` is the reason
    when it is false. A transfer that never happened raises ``GdbError``
    instead — that distinction is the one a person needs.
    """

    verified: bool = False
    board: int = 0
    firmware: str = ""
    lease_ms: int = 0
    message: str = ""
    error: GdbError | None = None

    @property
    def ok(self) -> bool:
        return self.verified

    def __str__(self) -> str:
        return self.message


@dataclass
class Config:
    """Where the firmware lives and how the board is put into BOOTSEL.

    ``manifest_dir`` is the firmware root — the folder that contains ``app/`` —
    and may also point straight at the crate (a folder holding ``Cargo.toml``).
    Empty means the ``firmware/`` folder of this checkout. ``boot_target`` is an
    explicit UF2 drive; empty waits for the one this command created.
    """

    cargo_path: str = ""
    manifest_dir: str = ""
    target: str = FLASH_TARGET
    features: str = FLASH_FEATURES
    boot_target: str = ""
    board_id: int = BOARD_ID
    skip_dependencies: bool = False
    auto_bootloader: bool = True
    drive_wait_s: float = DRIVE_WAIT_S
    handshake_wait_s: float = HANDSHAKE_WAIT_S

    @property
    def firmware(self) -> str:
        text = self.manifest_dir.strip()
        if text:
            return os.path.normpath(os.path.abspath(text))
        here = os.path.dirname(os.path.abspath(__file__))
        return os.path.normpath(os.path.join(here, "..", "..", "..", "firmware"))

    @property
    def app(self) -> str:
        root = self.firmware
        if os.path.isfile(os.path.join(root, "Cargo.toml")):
            return root
        return os.path.join(root, "app")

    @property
    def elf(self) -> str:
        return os.path.join(self.app, "target", self.target, "release", APP_CRATE)


class Reporter:
    """Progress, in the order the user sees it: stages, log lines, drive picker."""

    def log(self, line: str) -> None:
        pass

    def stage(self, stage: int, message: str) -> None:
        pass

    def choose_drive(self, candidates: Sequence[str]) -> str:
        """Several UF2 drives are mounted; the answer must be one of them.

        Refusing by default is deliberate: guessing here flashes the wrong board.
        """
        raise GdbError(
            ErrorCode.INVALID,
            "同時偵測到多個 UF2 磁碟（%s），請指定 boot 磁碟" % "、".join(candidates),
        )


class _Progress(Reporter):
    """Normalises ``progress``: a Reporter, a plain ``(stage, message)`` callable, or None."""

    def __init__(self, progress=None):
        self._target = progress

    def log(self, line: str) -> None:
        target = self._target
        if target is None:
            return
        handler = getattr(target, "log", None)
        if callable(handler):
            handler(line)
        elif callable(target):
            # A log line has no stage; the callable sees None.
            target(None, line)

    def stage(self, stage: int, message: str) -> None:
        target = self._target
        if target is None:
            return
        handler = getattr(target, "stage", None)
        if callable(handler):
            handler(stage, message)
        elif callable(target):
            target(stage, message)

    def choose_drive(self, candidates: Sequence[str]) -> str:
        target = self._target
        handler = getattr(target, "choose_drive", None)
        if callable(handler):
            return handler(list(candidates))
        return super().choose_drive(candidates)


class SystemRunner:
    """The plain runner: spawn, stream both pipes, kill on cancel or timeout."""

    def run(self, program, args, timeout_s, cancel=None, log=None) -> ProcessOutcome:
        outcome = ProcessOutcome()
        argv = [str(program)] + [str(arg) for arg in args]
        flags = CREATE_NO_WINDOW if sys.platform == "win32" else 0
        try:
            child = subprocess.Popen(
                argv,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                creationflags=flags,
            )
        except OSError as error:
            raise GdbError(ErrorCode.IO, f"無法啟動 {program}：{error}") from None
        lines: queue.Queue = queue.Queue()
        readers = [
            threading.Thread(target=_pump, args=(child.stdout, Stream.STDOUT, lines), daemon=True),
            threading.Thread(target=_pump, args=(child.stderr, Stream.STDERR, lines), daemon=True),
        ]
        for reader in readers:
            reader.start()
        deadline = time.monotonic() + max(0.0, float(timeout_s))
        while True:
            _drain_child(lines, 0.02, outcome, log)
            if _cancelled(cancel) or time.monotonic() >= deadline:
                outcome.cancelled = _cancelled(cancel)
                outcome.timed_out = not outcome.cancelled
                child.kill()
                outcome.exit = child.wait()
                break
            code = child.poll()
            if code is not None:
                outcome.exit = code
                break
        # A grandchild can keep the pipes open after the child is gone, so the
        # readers are given a moment and then left to finish on their own.
        end = time.monotonic() + 1.0
        while time.monotonic() < end:
            if not _drain_child(lines, 0.02, outcome, log):
                break
        for reader in readers:
            reader.join(0.5)
        return outcome


def _pump(pipe, stream: str, lines: "queue.Queue") -> None:
    try:
        for raw in iter(pipe.readline, b""):
            lines.put((stream, raw.decode("utf-8", "replace").rstrip("\r\n")))
    finally:
        try:
            pipe.close()
        except Exception:  # pragma: no cover - best effort
            pass
        lines.put(None)


def _drain_child(lines, wait_s: float, outcome: ProcessOutcome, log) -> bool:
    """Moves whatever the readers produced into ``outcome``; ``False`` once they are gone."""
    limit = 10 * 1024 * 1024
    while True:
        try:
            item = lines.get(timeout=wait_s)
        except queue.Empty:
            return True
        if item is None:
            return False
        stream, text = item
        sink = "stdout" if stream == Stream.STDOUT else "stderr"
        if len(getattr(outcome, sink)) + len(text) <= limit:
            setattr(outcome, sink, getattr(outcome, sink) + text + "\n")
        if log is not None:
            log(stream, text)


def _cancelled(cancel) -> bool:
    """Reads the host's cancellation token: ``cancelled()``, ``is_set()``, callable, bool."""
    if cancel is None:
        return False
    for name in ("cancelled", "is_set"):
        reader = getattr(cancel, name, None)
        if callable(reader):
            return bool(reader())
    if callable(cancel):
        return bool(cancel())
    return bool(cancel)


def _check(cancel) -> None:
    if _cancelled(cancel):
        raise GdbError(ErrorCode.CANCELLED, "操作已取消")


def _pause(cancel, seconds: float = POLL_S) -> None:
    """Sleeps in slices so cancellation is honoured inside a wait."""
    end = time.monotonic() + max(0.0, seconds)
    while True:
        _check(cancel)
        remaining = end - time.monotonic()
        if remaining <= 0:
            return
        time.sleep(min(remaining, 0.01))


def _invoke_runner(runner, program, args, timeout_s, cancel, log):
    handler = getattr(runner, "run", None)
    if callable(handler):
        return handler(program, args, timeout_s, cancel, log)
    if callable(runner):
        return runner(program, args, timeout_s, cancel, log)
    raise GdbError(
        ErrorCode.INVALID, "runner must expose run(program, args, timeout, cancel, log)"
    )


def _as_process_outcome(value) -> ProcessOutcome:
    if isinstance(value, ProcessOutcome):
        return value
    if value is None:
        return ProcessOutcome(exit=0)
    return ProcessOutcome(
        stdout=getattr(value, "stdout", "") or "",
        stderr=getattr(value, "stderr", "") or "",
        exit=getattr(value, "exit", 0),
        timed_out=bool(getattr(value, "timed_out", False)),
        cancelled=bool(getattr(value, "cancelled", False)),
    )


def _cargo_path(config: Config) -> str:
    """``cargo``, preferring an explicit setting."""
    text = config.cargo_path.strip()
    if text:
        if not os.path.isfile(text):
            raise GdbError(ErrorCode.INVALID, f"指定的 cargo 不存在：{text}")
        return text
    found = shutil.which("cargo.exe") or shutil.which("cargo")
    if not found:
        raise GdbError(
            ErrorCode.DISCONNECTED,
            "找不到 cargo；請安裝 Rust 工具鏈，或在設定指定 cargo 路徑",
        )
    return found


def _rustup_beside(cargo: str) -> str | None:
    """rustup normally sits beside cargo; without it the target cannot be added."""
    name = "rustup.exe" if sys.platform == "win32" else "rustup"
    candidate = os.path.join(os.path.dirname(cargo), name)
    return candidate if os.path.isfile(candidate) else None


def _run(runner, program, args, timeout_s, cancel, report: Reporter) -> str:
    _check(cancel)
    runner = SystemRunner() if runner is None else runner
    report.log(f"{program} {' '.join(str(arg) for arg in args)}")
    outcome = _as_process_outcome(
        _invoke_runner(
            runner,
            program,
            list(args),
            float(timeout_s),
            cancel,
            lambda stream, text: report.log(f"{stream}: {text}"),
        )
    )
    _check(cancel)
    return outcome.check()


def build(config: Config | None = None, runner=None, cancel=None, progress=None) -> Image:
    """Builds ``app/`` for the RP2040 and packs the result into a UF2 image."""
    config = config or Config()
    report = _Progress(progress)
    _check(cancel)
    manifest = os.path.join(config.app, "Cargo.toml")
    if not os.path.isfile(manifest):
        raise GdbError(
            ErrorCode.INVALID,
            f"韌體資料夾不完整（缺 app/Cargo.toml）：{config.firmware}",
        )
    report.stage(1, STAGES[1])
    cargo = _cargo_path(config)
    report.stage(2, STAGES[2])
    if config.skip_dependencies:
        report.log(f"略過 target 安裝（假設 {config.target} 已安裝）")
    else:
        rustup = _rustup_beside(cargo)
        if rustup is None:
            report.log(f"找不到 rustup，跳過 target 安裝（假設 {config.target} 已安裝）")
        else:
            _run(runner, rustup, ["target", "add", config.target], 600.0, cancel, report)
    report.stage(3, STAGES[3])
    _run(
        runner,
        cargo,
        [
            "build",
            "--release",
            "--target",
            config.target,
            "--manifest-path",
            manifest,
            "--features",
            config.features,
        ],
        900.0,
        cancel,
        report,
    )
    elf = config.elf
    if not os.path.isfile(elf):
        raise GdbError(ErrorCode.INVALID, f"編譯成功但找不到 ELF：{elf}")
    try:
        with open(elf, "rb") as handle:
            data = handle.read()
    except OSError as error:
        raise GdbError(ErrorCode.IO, f"讀取 ELF 失敗：{elf}：{error}") from None
    report.stage(4, STAGES[4])
    packed = pack_elf(data)
    blocks = uf2_blocks(packed)
    report.log(f"UF2：{blocks} 個區塊")
    return Image(bytes=packed, blocks=blocks)


def drives() -> list:
    """Every mounted RP2040 bootloader drive, as ``"E:\\"`` roots."""
    if sys.platform != "win32":
        raise GdbError(ErrorCode.UNSUPPORTED, "UF2 磁碟偵測僅支援 Windows")
    kernel = ctypes.WinDLL("kernel32.dll")
    kernel.GetLogicalDrives.restype = ctypes.c_uint32
    mask = kernel.GetLogicalDrives()
    found = []
    for bit in range(26):
        if mask & (1 << bit):
            root = f"{chr(ord('A') + bit)}:\\"
            try:
                with open(os.path.join(root, "INFO_UF2.TXT"), encoding="utf-8") as handle:
                    info = handle.read()
            except OSError:
                continue
            if "RP2040" in info or "RPI-RP2" in info:
                found.append(root)
    return found


def _wait_drive(cancel, report: Reporter, previous: set, timeout_s: float, discover=None) -> str:
    scan = discover if discover is not None else drives
    deadline = time.monotonic() + max(0.0, float(timeout_s))
    while time.monotonic() < deadline:
        _check(cancel)
        fresh = [
            path for path in scan() if os.path.normcase(str(path)) not in previous
        ]
        if len(fresh) == 1:
            return fresh[0]
        if len(fresh) > 1:
            return report.choose_drive([str(path) for path in fresh])
        _pause(cancel)
    raise GdbError(
        ErrorCode.TIMEOUT,
        f"{float(timeout_s):.0f} 秒內未偵測到 RP2040 UF2 磁碟；請 BOOTSEL 上電後重試",
    )


def write_image(target: str, data: bytes, cancel=None) -> None:
    """Writes the image to ``target`` in chunks so a cancelled copy stops early."""
    _check(cancel)
    try:
        output = open(target, "wb")
    except OSError as error:
        raise GdbError(
            ErrorCode.IO, f"建立 UF2 目的檔失敗：{target}：{error}"
        ) from None
    with output:
        for at in range(0, len(data), COPY_CHUNK):
            _check(cancel)
            try:
                output.write(data[at : at + COPY_CHUNK])
            except OSError as error:
                raise GdbError(
                    ErrorCode.IO, f"UF2 複製失敗（保留 OS 錯誤碼）：{target}：{error}"
                ) from None
        try:
            output.flush()
        except OSError as error:
            raise GdbError(ErrorCode.IO, f"UF2 重新整理失敗：{target}：{error}") from None


def _check_board(reported: int, expected: int) -> None:
    if int(reported) == int(expected):
        return
    raise GdbError(
        ErrorCode.INVALID, f"重新列舉的板型不符：{reported}（預期 {expected}）"
    )


def _open_and_check(entry: dict, config: Config):
    """Opens one enumerated board and reads Feature 14: ``(board, firmware, lease_ms)``."""
    device = Device.open(
        entry.get("path") or entry.get("serial") or None,
        timeout_ms=1000,
        auto_reconnect=False,
        heartbeat=False,
    )
    try:
        info = device.info
        board = int(info["board"])
        firmware = str(info["firmware"])
        lease_ms = int(info["leaseMs"])
    finally:
        try:
            device.close()
        except GdbError:
            pass
    _check_board(board, config.board_id)
    return board, firmware, lease_ms


def _runtime_candidates(entries: list, existing: list) -> list:
    """Devices that appeared after the copy: an already-running board cannot stand in."""
    known = {str(path) for path in existing}
    return [entry for entry in entries if str(entry.get("path")) not in known]


def _wait_for_board(cancel, timeout_s: float, serial, enumerate_devices, open_entry):
    deadline = time.monotonic() + max(0.0, float(timeout_s))
    last = "尚未列舉"
    while time.monotonic() < deadline:
        _check(cancel)
        matched = [
            entry
            for entry in enumerate_devices()
            if not serial or str(entry.get("serial") or "") == serial
        ]
        # Duplicate firmware serials cannot prove identity: do not pick the first.
        if len(matched) == 1:
            try:
                return open_entry(matched[0])
            except GdbError as error:
                last = error.message
        elif len(matched) > 1:
            last = "多個符合裝置，無法確認燒錄目標；請僅連接目標板後驗證"
        _pause(cancel)
    raise GdbError(ErrorCode.TIMEOUT, f"runtime hello 驗證逾時：{last}")


def push(
    device=None,
    image=None,
    progress=None,
    cancel=None,
    config: Config | None = None,
    discover=None,
    enumerate_devices=None,
    open_device=None,
) -> Outcome:
    """Sends BOOTLOADER, copies the image onto the UF2 drive, waits for the board.

    ``device`` may be ``None`` (the board is then expected to be in BOOTSEL
    already). It is closed before the drive is waited for: the board refuses
    BOOTLOADER while a host keeps polling it, and the caller's handle is spent
    afterwards. A ``GdbError`` means the transfer itself failed;
    ``Outcome(verified=False)`` means the copy finished but the board never
    answered — the distinction the caller reports to the user.

    ``discover``/``enumerate_devices``/``open_device`` are the seams the Rust SDK
    names ``drives``/``Manager::list``/``Manager::open``; tests replace them.
    """
    config = config or Config()
    report = _Progress(progress)
    _check(cancel)
    if image is None:
        raise GdbError(ErrorCode.INVALID, "沒有可傳輸的映像；先呼叫 build()")
    image = image if isinstance(image, Image) else Image.from_uf2(bytes(image))
    if not image.bytes:
        raise GdbError(ErrorCode.INVALID, "映像為空")
    scan = discover if discover is not None else drives
    enumerate_fn = enumerate_devices if enumerate_devices is not None else list_devices
    open_entry = (
        open_device
        if open_device is not None
        else (lambda entry: _open_and_check(entry, config))
    )
    # Read the serial while the runtime handle still exists; the board is gone by
    # the time the copy starts.
    serial = ""
    if device is not None:
        try:
            serial = str((device.info or {}).get("serial") or "")
        except GdbError:
            serial = ""

    report.stage(5, STAGES[5])
    explicit = config.boot_target.strip()
    # Never auto-select a drive that was already mounted before this command: it
    # belongs to some other board.
    previous = set()
    if config.auto_bootloader and not explicit and device is not None:
        previous = {os.path.normcase(str(path)) for path in scan()}
    if config.auto_bootloader and not explicit:
        if device is not None:
            try:
                device.bootloader()
                report.log("已確認 enter_bootloader；等待裝置重啟")
            except GdbError as error:
                _check(cancel)
                report.log(
                    "bootloader 命令未收到完整確認，繼續檢查列舉結果：" + error.message
                )
            # The board refuses BOOTLOADER while a host keeps polling it, and the
            # runtime session has to be gone before the drive shows up.
            try:
                device.close()
            except GdbError:
                pass
        else:
            report.log("未選取 runtime HID，請使用實體 BOOTSEL 進入下載模式")
    if explicit:
        drive = explicit
    else:
        drive = _wait_drive(cancel, report, previous, config.drive_wait_s, discover)
    _check(cancel)
    # Boards already in runtime here are unrelated to this transfer: excluding
    # them stops an existing board's hello from verifying another board.
    existing = [str(entry.get("path")) for entry in enumerate_fn()]
    report.stage(6, STAGES[6])
    if not os.path.isfile(os.path.join(drive, "INFO_UF2.TXT")):
        raise GdbError(ErrorCode.INVALID, "指定位置不是 UF2 bootloader 磁碟")
    write_image(os.path.join(drive, "Firmware.uf2"), image.bytes, cancel)
    report.log("UF2 複製完成，等待 runtime 驗證")
    report.stage(7, STAGES[7])
    try:
        board, firmware, lease_ms = _wait_for_board(
            cancel,
            config.handshake_wait_s,
            serial or None,
            lambda: _runtime_candidates(enumerate_fn(), existing),
            open_entry,
        )
    except GdbError as error:
        return Outcome(verified=False, message=error.message, error=error)
    return Outcome(
        verified=True,
        board=board,
        firmware=firmware,
        lease_ms=lease_ms,
        message=f"board={board}, fw={firmware}, lease={lease_ms} ms",
    )


class _Uf2:
    """``gdb.flash.uf2`` — the RP2040 ELF → UF2 packer."""

    FAMILY_RP2040 = FAMILY_RP2040
    XIP_BASE = XIP_BASE
    XIP_LIMIT = XIP_LIMIT
    BLOCK = UF2_BLOCK
    PAYLOAD = UF2_PAYLOAD
    MAGIC_START = UF2_MAGIC_START
    MAGIC_END = UF2_MAGIC_END
    FLAG_FAMILY = UF2_FLAG_FAMILY

    pack = staticmethod(pack_elf)
    blocks = staticmethod(uf2_blocks)
    Image = Image


class _Flash:
    """``gdb.flash`` — build, pack, push; the same module, a separate namespace."""

    Config = Config
    Image = Image
    Outcome = Outcome
    ProcessOutcome = ProcessOutcome
    Reporter = Reporter
    Stream = Stream
    SystemRunner = SystemRunner

    APP_CRATE = APP_CRATE
    TARGET = FLASH_TARGET
    FEATURES = FLASH_FEATURES
    BOARD_ID = BOARD_ID
    STAGES = STAGES
    DRIVE_WAIT_S = DRIVE_WAIT_S
    HANDSHAKE_WAIT_S = HANDSHAKE_WAIT_S

    uf2 = _Uf2()

    build = staticmethod(build)
    push = staticmethod(push)
    pack = staticmethod(pack_elf)
    blocks = staticmethod(uf2_blocks)
    write_image = staticmethod(write_image)
    drives = staticmethod(drives)


flash = _Flash()

