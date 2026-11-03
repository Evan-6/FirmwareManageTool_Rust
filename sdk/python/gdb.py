"""GDBOX host SDK for Python.

ctypes binding over ``gdb.dll`` (the Rust SDK compiled to a C ABI), so the
behaviour is the same as the Rust and C++ SDKs. No hardware is needed for
``Device.open_mock()``.

    import gdb
    with gdb.Device.open_mock() as device:
        print(device.info)
        device.keyboard.type_text("hello")
        device.mouse.move_to(200, 300, duration_ms=250)
"""

from __future__ import annotations

import ctypes
import json
import os
import sys
from ctypes import POINTER, byref, c_bool, c_char_p, c_double, c_int16, c_int32, c_size_t, c_uint8, c_uint16, c_uint32, c_uint64, c_void_p
from dataclasses import dataclass
from typing import Callable, Iterable, Sequence

__all__ = [
    "GdbError",
    "ErrorCode",
    "ConnectionState",
    "Button",
    "Ease",
    "Profile",
    "KeyStep",
    "Point",
    "Packet",
    "Movement",
    "Mouse",
    "Keyboard",
    "Scheduler",
    "Device",
    "Remote",
    "Observer",
    "Watch",
    "watch",
    "probe",
    "probe_all",
    "monitors",
    "window_metrics",
    "prepare_platform",
    "LIBRARY",
]

ABI_VERSION = 1
ABI_VERSION_MINOR = 9


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
    INVALID_SCREEN = 11
    STALE_WINDOW = 12

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
        11: "INVALID_SCREEN",
        12: "STALE_WINDOW",
    }


class ConnectionState:
    DISCONNECTED = 0
    CONNECTING = 1
    CONNECTED = 2
    ERROR = 3


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


class GdbError(Exception):
    """Raised for every non-zero ErrorCode coming out of the SDK."""

    def __init__(self, code: int, message: str):
        self.code = code
        self.name = ErrorCode.NAMES.get(code, str(code))
        self.message = message
        super().__init__(f"{self.name}: {message}" if message else self.name)


class _Point(ctypes.Structure):
    _fields_ = [("x", c_int32), ("y", c_int32)]


class _KeyStep(ctypes.Structure):
    _fields_ = [
        ("page", c_uint16),
        ("usage", c_uint16),
        ("hold_ms", c_uint32),
        ("gap_ms", c_uint32),
    ]


class _Packet(ctypes.Structure):
    _fields_ = [
        ("opcode", c_uint8),
        ("flags", c_uint8),
        ("session", c_uint32),
        ("sequence", c_uint32),
        ("payload", POINTER(c_uint8)),
        ("len", c_size_t),
    ]


class _Profile(ctypes.Structure):
    _fields_ = [
        ("ease", c_int32),
        ("bezier", c_double * 4),
        ("curve", c_double),
        ("jitter_px", c_double),
        ("overshoot_pct", c_double),
        ("settle_ms", c_uint32),
        ("min_step_px", c_int32),
    ]


Point = tuple[int, int]
EventCallback = Callable[[int, int, str], None]
TaskCallback = Callable[[], int]
LogCallback = Callable[[int, str], None]
WatchCallback = Callable[[int, dict], None]


def _library_candidates() -> Iterable[str]:
    override = os.environ.get("GDBOX_LIBRARY")
    if override:
        yield override
    here = os.path.dirname(os.path.abspath(__file__))
    root = os.path.abspath(os.path.join(here, "..", ".."))
    names = ["gdb.dll"] if sys.platform == "win32" else ["libgdb.so", "libgdb.dylib", "gdb.so"]
    for name in names:
        yield os.path.join(here, name)
        for profile in ("debug", "release"):
            yield os.path.join(root, "target", profile, name)


def _load() -> ctypes.CDLL:
    errors = []
    for path in _library_candidates():
        if not os.path.exists(path):
            continue
        try:
            return ctypes.WinDLL(path) if sys.platform == "win32" else ctypes.CDLL(path)
        except OSError as error:  # pragma: no cover - platform dependent
            errors.append(f"{path}: {error}")
    raise GdbError(
        ErrorCode.IO,
        "cannot load the GDBOX library; build it with `cargo build -p gdb-ffi` "
        "or set GDBOX_LIBRARY. Tried: " + ", ".join(errors or ["no candidate paths"]),
    )


LIBRARY = _load()

_D = LIBRARY
_D.gdb_abi_version.restype = c_uint32
_D.gdb_version.restype = c_char_p
_D.gdb_last_error.argtypes = [c_char_p, c_size_t]
_D.gdb_last_error.restype = c_int32
_D.gdb_device_last_error.argtypes = [c_void_p, c_char_p, c_size_t]
_D.gdb_device_last_error.restype = c_int32
_D.gdb_string_free.argtypes = [c_void_p]
_D.gdb_set_log_level.argtypes = [c_uint8]
_D.gdb_set_log_level.restype = c_int32
_D.gdb_device_list.restype = c_void_p
_D.gdb_open.restype = c_void_p
_D.gdb_open.argtypes = [c_char_p, c_uint32, c_bool, c_bool]
_D.gdb_open_mock.restype = c_void_p
_D.gdb_open_mock.argtypes = [c_uint32, c_bool, c_bool]
_D.gdb_open_shared.restype = c_void_p
_D.gdb_open_shared.argtypes = [c_char_p, c_uint32, c_bool, c_bool]
_D.gdb_open_mock_shared.restype = c_void_p
_D.gdb_open_mock_shared.argtypes = [c_uint32, c_bool, c_bool]
_D.gdb_stolen_frames.argtypes = [c_void_p]
_D.gdb_stolen_frames.restype = c_uint64
_D.gdb_device_free.argtypes = [c_void_p]
_D.gdb_close.argtypes = [c_void_p]
_D.gdb_close.restype = c_int32
_D.gdb_reconnect.argtypes = [c_void_p]
_D.gdb_reconnect.restype = c_int32
_D.gdb_set_auto_reconnect.argtypes = [c_void_p, c_bool]
_D.gdb_set_auto_reconnect.restype = c_int32
_D.gdb_device_state.argtypes = [c_void_p]
_D.gdb_device_state.restype = c_int32
for name in ("gdb_device_info", "gdb_status", "gdb_status_ext", "gdb_sessions"):
    getattr(_D, name).argtypes = [c_void_p]
    getattr(_D, name).restype = c_void_p
for name in ("gdb_release_all", "gdb_barrier", "gdb_bootloader"):
    getattr(_D, name).argtypes = [c_void_p]
    getattr(_D, name).restype = c_int32
_D.gdb_set_event_handler.argtypes = [c_void_p, c_void_p, c_void_p]
_D.gdb_set_event_handler.restype = c_int32
_D.gdb_command.argtypes = [c_void_p, c_uint8, POINTER(c_uint8), c_size_t, POINTER(_Packet)]
_D.gdb_command.restype = c_int32
_D.gdb_packet_free.argtypes = [POINTER(_Packet)]
_D.gdb_packet_free.restype = None
_D.gdb_profile_default.restype = _Profile

_D.gdb_mouse_move_by.argtypes = [c_void_p, c_int32, c_int32, c_uint32, POINTER(_Profile)]
_D.gdb_mouse_move_by.restype = c_int32
_D.gdb_move_raw.argtypes = [c_void_p, c_int32, c_int32]
_D.gdb_move_raw.restype = c_int32
_D.gdb_mouse_move_to.argtypes = [c_void_p, c_int32, c_int32, c_uint32, POINTER(_Profile)]


class _Screen(ctypes.Structure):
    _fields_ = [("kind", c_int32), ("arg", ctypes.c_int64)]


class Metrics(ctypes.Structure):
    """The session's counters, as gdb_metrics_get fills them."""
    _fields_ = [
        ("frames_sent", c_uint64), ("frames_received", c_uint64),
        ("retries", c_uint64), ("stolen_frames", c_uint64),
        ("reconnects", c_uint64), ("lease_faults", c_uint64), ("timeouts", c_uint64),
        ("latency_count", c_uint64), ("latency_min_us", c_uint64),
        ("latency_p50_us", c_uint64), ("latency_p95_us", c_uint64), ("latency_max_us", c_uint64),
    ]

    def as_dict(self) -> dict:
        return {name: getattr(self, name) for name, _ in self._fields_}


class Clamps(ctypes.Structure):
    """Clamped absolute moves, as gdb_metrics_clamps fills them.

    A move whose destination was outside the anchor rectangle is pulled back
    into it: it still converges and the click still lands inside the window, and
    only these numbers say the requested pixel was never reached. The four
    coordinates are meaningful when ``clamped_total`` is not zero.
    """
    _fields_ = [
        ("clamped_total", c_uint64),
        ("requested_x", c_int32), ("requested_y", c_int32),
        ("applied_x", c_int32), ("applied_y", c_int32),
    ]

    def as_dict(self) -> dict:
        return {name: getattr(self, name) for name, _ in self._fields_}


class Screen:
    """Which rectangle an absolute move is clamped into. Coordinates stay
    virtual-desktop physical pixels; the target is not a new origin."""
    primary = None  # set below

    def __init__(self, kind: int, arg: int = 0):
        self.kind, self.arg = kind, arg

    @classmethod
    def monitor(cls, index: int) -> "Screen":
        return cls(1, index)

    @classmethod
    def window(cls, hwnd: int) -> "Screen":
        return cls(2, hwnd)

    @classmethod
    def window_client(cls, hwnd: int) -> "Screen":
        """A window's client area: the part inside the frame, which is where a
        game draws and where a screenshot's coordinates start. ``window`` is the
        whole frame, title bar and borders included."""
        return cls(4, hwnd)

    @classmethod
    def containing(cls, x: int, y: int) -> "Screen":
        """The monitor holding the point, so a caller that starts a move from a
        coordinate does not have to know which display that is.

        The point travels in the same 64-bit argument every target uses: x in the
        high 32 bits, y in the low 32, both as uint32. A point on no monitor
        resolves to the union of every screen, and the move then fails with
        ``UNSUPPORTED`` naming it.
        """
        return cls(3, ((x & 0xFFFFFFFF) << 32) | (y & 0xFFFFFFFF))

    def _raw(self) -> _Screen:
        return _Screen(self.kind, self.arg)


Screen.primary = Screen(0)

_D.gdb_metrics_get.argtypes = [c_void_p, POINTER(Metrics)]
_D.gdb_metrics_get.restype = c_int32
_D.gdb_metrics_reset.argtypes = [c_void_p]
_D.gdb_metrics_reset.restype = c_int32
_D.gdb_metrics_clamps.argtypes = [c_void_p, POINTER(Clamps)]
_D.gdb_metrics_clamps.restype = c_int32
_D.gdb_monitors_json.argtypes = []
_D.gdb_monitors_json.restype = c_void_p
_D.gdb_window_metrics.argtypes = [ctypes.c_int64]
_D.gdb_window_metrics.restype = c_void_p
_D.gdb_platform_prepare.argtypes = []
_D.gdb_platform_prepare.restype = c_void_p
_D.gdb_mouse_move_to_on.argtypes = [c_void_p, c_int32, c_int32, _Screen, c_uint32, POINTER(_Profile)]
_D.gdb_mouse_move_to_on.restype = c_int32
_D.gdb_mouse_move_path_on.argtypes = [c_void_p, POINTER(_Point), c_size_t, _Screen, c_uint32, POINTER(_Profile)]
_D.gdb_mouse_move_path_on.restype = c_int32
_D.gdb_mouse_start_move_to_on.argtypes = [c_void_p, c_int32, c_int32, _Screen, c_uint32, POINTER(_Profile)]
_D.gdb_mouse_start_move_to_on.restype = c_void_p
_D.gdb_mouse_move_to.restype = c_int32
_D.gdb_mouse_move_path.argtypes = [c_void_p, POINTER(_Point), c_size_t, c_uint32, POINTER(_Profile)]
_D.gdb_mouse_move_path.restype = c_int32
_D.gdb_mouse_button.argtypes = [c_void_p, c_uint8, c_bool]
_D.gdb_mouse_button.restype = c_int32
_D.gdb_mouse_click.argtypes = [c_void_p, c_uint8, c_uint8]
_D.gdb_mouse_click.restype = c_int32
_D.gdb_mouse_click_timed.argtypes = [c_void_p, c_uint8, c_uint8, c_uint32, c_uint32, c_uint32]
_D.gdb_mouse_click_timed.restype = c_int32
_D.gdb_mouse_click_at_on.argtypes = [c_void_p, c_int32, c_int32, _Screen, c_uint8, c_uint8, c_uint32, c_uint32, c_uint32]
_D.gdb_mouse_click_at_on.restype = c_int32
_D.gdb_mouse_double_click.argtypes = [c_void_p, c_uint8]
_D.gdb_mouse_double_click.restype = c_int32
_D.gdb_mouse_wheel.argtypes = [c_void_p, c_int32, c_int32]
_D.gdb_mouse_wheel.restype = c_int32
_D.gdb_mouse_drag.argtypes = [c_void_p, c_uint8, c_int32, c_int32, c_uint32, POINTER(_Profile)]
_D.gdb_mouse_drag.restype = c_int32
for name in ("gdb_mouse_start_move_to", "gdb_mouse_start_move_by"):
    getattr(_D, name).argtypes = [c_void_p, c_int32, c_int32, c_uint32, POINTER(_Profile)]
    getattr(_D, name).restype = c_void_p
_D.gdb_mouse_start_move_path.argtypes = [c_void_p, POINTER(_Point), c_size_t, c_uint32, POINTER(_Profile)]
_D.gdb_mouse_start_move_path.restype = c_void_p
for name in ("gdb_movement_cancel", "gdb_movement_retarget"):
    getattr(_D, name).restype = c_int32
_D.gdb_movement_cancel.argtypes = [c_void_p]
_D.gdb_movement_retarget.argtypes = [c_void_p, c_int32, c_int32]
_D.gdb_movement_is_finished.argtypes = [c_void_p]
_D.gdb_movement_is_finished.restype = c_bool
_D.gdb_movement_wait.argtypes = [c_void_p, c_uint32]
_D.gdb_movement_wait.restype = c_int32
_D.gdb_movement_free.argtypes = [c_void_p]

for name in ("gdb_key_down", "gdb_key_up", "gdb_key_press"):
    getattr(_D, name).argtypes = [c_void_p, c_uint16, c_uint16]
    getattr(_D, name).restype = c_int32
_D.gdb_key_long_press.argtypes = [c_void_p, c_uint16, c_uint16, c_uint32]
_D.gdb_key_long_press.restype = c_int32
_D.gdb_keys_chord.argtypes = [c_void_p, POINTER(c_uint16), c_size_t, c_uint32]
_D.gdb_keys_chord.restype = c_int32
_D.gdb_keys_sequence.argtypes = [c_void_p, POINTER(_KeyStep), c_size_t]
_D.gdb_keys_sequence.restype = c_int32
_D.gdb_type_text.argtypes = [c_void_p, c_char_p, c_uint32, c_uint32, c_uint32]
_D.gdb_type_text.restype = c_int32

_D.gdb_scheduler_new.argtypes = [c_void_p]
_D.gdb_scheduler_new.restype = c_void_p
_D.gdb_scheduler_free.argtypes = [c_void_p]
_D.gdb_scheduler_submit.argtypes = [c_void_p, c_uint32, c_int32, c_void_p, c_void_p]
_D.gdb_scheduler_submit.restype = c_uint64
_D.gdb_scheduler_submit_at.argtypes = [c_void_p, c_uint64, c_int32, c_void_p, c_void_p]
_D.gdb_scheduler_submit_at.restype = c_uint64
_D.gdb_scheduler_elapsed_ms.argtypes = [c_void_p]
_D.gdb_scheduler_elapsed_ms.restype = c_uint64
_D.gdb_scheduler_every.argtypes = [c_void_p, c_uint32, c_int32, c_void_p, c_void_p]
_D.gdb_scheduler_every.restype = c_uint64
_D.gdb_scheduler_cancel.argtypes = [c_void_p, c_uint64]
_D.gdb_scheduler_cancel.restype = c_int32
for name in ("gdb_scheduler_pause", "gdb_scheduler_resume"):
    getattr(_D, name).argtypes = [c_void_p]
    getattr(_D, name).restype = c_int32
_D.gdb_scheduler_wait_idle.argtypes = [c_void_p, c_uint32]
_D.gdb_scheduler_wait_idle.restype = c_int32
_D.gdb_scheduler_take_errors.argtypes = [c_void_p]
_D.gdb_scheduler_take_errors.restype = c_void_p

# The attached surface: separate symbols, same header.
_D.gdb_attach.argtypes = [c_char_p, POINTER(c_void_p)]
_D.gdb_attach.restype = c_int32
_D.gdb_remote_spawn.argtypes = [c_char_p]
_D.gdb_remote_spawn.restype = c_int32
_D.gdb_remote_free.argtypes = [c_void_p]
_D.gdb_remote_free.restype = None
_D.gdb_remote_close.argtypes = [c_void_p]
_D.gdb_remote_close.restype = c_int32
for name in ("gdb_remote_info", "gdb_remote_status", "gdb_remote_status_ext", "gdb_remote_sessions"):
    getattr(_D, name).argtypes = [c_void_p]
    getattr(_D, name).restype = c_void_p
_D.gdb_remote_key.argtypes = [c_void_p, c_uint16, c_uint16, c_bool]
_D.gdb_remote_key.restype = c_int32
_D.gdb_remote_mouse_move.argtypes = [c_void_p, c_int16, c_int16]
_D.gdb_remote_mouse_move.restype = c_int32
_D.gdb_remote_move_by.argtypes = [c_void_p, c_int32, c_int32, c_uint32, POINTER(_Profile)]
_D.gdb_remote_move_by.restype = c_int32
_D.gdb_remote_move_raw.argtypes = [c_void_p, c_int32, c_int32]
_D.gdb_remote_move_raw.restype = c_int32
_D.gdb_remote_mouse_buttons.argtypes = [c_void_p, c_uint8]
_D.gdb_remote_mouse_buttons.restype = c_int32
_D.gdb_remote_wheel.argtypes = [c_void_p, c_int32, c_int32]
_D.gdb_remote_wheel.restype = c_int32
_D.gdb_remote_barrier.argtypes = [c_void_p]
_D.gdb_remote_barrier.restype = c_int32
_D.gdb_remote_release_all.argtypes = [c_void_p]
_D.gdb_remote_release_all.restype = c_int32
_D.gdb_remote_command.argtypes = [c_void_p, c_uint8, POINTER(c_uint8), c_size_t, POINTER(_Packet)]
_D.gdb_remote_command.restype = c_int32

# The presence surface: Feature 14 only, no session, no packet.
_D.gdb_probe.argtypes = [c_char_p]
_D.gdb_probe.restype = c_void_p
_D.gdb_probe_all.argtypes = []
_D.gdb_probe_all.restype = c_void_p
_D.gdb_observer_start.argtypes = [c_char_p, c_uint32]
_D.gdb_observer_start.restype = c_void_p
_D.gdb_observer_stop.argtypes = [c_void_p]
_D.gdb_observer_stop.restype = None
_D.gdb_observer_free.argtypes = [c_void_p]
_D.gdb_observer_free.restype = None
_D.gdb_observer_info.argtypes = [c_void_p]
_D.gdb_observer_info.restype = c_void_p
_D.gdb_observer_state.argtypes = [c_void_p]
_D.gdb_observer_state.restype = c_int32
for name in ("gdb_observer_status", "gdb_observer_barrier", "gdb_observer_release_all"):
    getattr(_D, name).argtypes = [c_void_p]
    getattr(_D, name).restype = c_int32
_D.gdb_observer_command.argtypes = [c_void_p, c_uint8, POINTER(c_uint8), c_size_t, POINTER(_Packet)]
_D.gdb_observer_command.restype = c_int32


def _last_error() -> tuple[int, str]:
    buffer = ctypes.create_string_buffer(512)
    code = _D.gdb_last_error(buffer, len(buffer))
    return code, buffer.value.decode("utf-8", "replace")


def _check(code: int) -> None:
    if code != 0:
        _, message = _last_error()
        raise GdbError(code, message)


def _take(pointer: int | None):
    if not pointer:
        code, message = _last_error()
        raise GdbError(code or ErrorCode.IO, message or "no reply")
    try:
        return json.loads((ctypes.cast(pointer, c_char_p).value or b"").decode("utf-8"))
    finally:
        _D.gdb_string_free(pointer)


@dataclass(frozen=True)
class Profile:
    """Motion profile; the defaults mirror `HumanProfile::default()`."""

    ease: int = Ease.JERK
    bezier: tuple[float, float, float, float] = (0.42, 0.0, 0.58, 1.0)
    curve: float = 1.0
    jitter_px: float = 1.0
    overshoot_pct: float = 0.0
    settle_ms: int = 0
    min_step_px: int = 1

    @classmethod
    def linear(cls) -> "Profile":
        return cls(ease=Ease.LINEAR, curve=0.0, jitter_px=0.0)

    @classmethod
    def from_library(cls) -> "Profile":
        default = _D.gdb_profile_default()
        return cls(
            ease=default.ease,
            bezier=tuple(default.bezier),
            curve=default.curve,
            jitter_px=default.jitter_px,
            overshoot_pct=default.overshoot_pct,
            settle_ms=default.settle_ms,
            min_step_px=default.min_step_px,
        )

    def _ctype(self) -> _Profile:
        value = _Profile()
        value.ease = self.ease
        for index, number in enumerate(self.bezier):
            value.bezier[index] = number
        value.curve = self.curve
        value.jitter_px = self.jitter_px
        value.overshoot_pct = self.overshoot_pct
        value.settle_ms = self.settle_ms
        value.min_step_px = self.min_step_px
        return value


@dataclass(frozen=True)
class KeyStep:
    page: int = 7
    usage: int = 0
    hold_ms: int = 18
    gap_ms: int = 0


@dataclass(frozen=True)
class Packet:
    """One reply packet; ``Device.command`` returns None instead of an empty one."""

    opcode: int
    flags: int
    session: int
    sequence: int
    payload: bytes


def _profile_pointer(profile: Profile | None):
    if profile is None:
        return POINTER(_Profile)()
    return ctypes.pointer(profile._ctype())


class Movement:
    """Handle for a running move; use as a context manager or call wait()."""

    def __init__(self, pointer: int):
        self._pointer = pointer

    def __bool__(self) -> bool:
        return bool(self._pointer)

    def cancel(self) -> None:
        _check(_D.gdb_movement_cancel(self._pointer))

    def retarget(self, x: int, y: int) -> None:
        _check(_D.gdb_movement_retarget(self._pointer, x, y))

    @property
    def finished(self) -> bool:
        return bool(_D.gdb_movement_is_finished(self._pointer))

    def wait(self, timeout_ms: int = 10_000) -> None:
        _check(_D.gdb_movement_wait(self._pointer, timeout_ms))

    def close(self) -> None:
        if self._pointer:
            _D.gdb_movement_free(self._pointer)
            self._pointer = 0

    def __enter__(self) -> "Movement":
        return self

    def __exit__(self, *exc_info) -> None:
        self.close()

    def __del__(self) -> None:  # pragma: no cover - best effort
        self.close()


class Mouse:
    def __init__(self, device: "Device"):
        self._device = device

    def _handle(self) -> int:
        return self._device._handle()

    def move_by(self, dx: int, dy: int, duration_ms: int = 0, profile: Profile | None = None) -> None:
        _check(_D.gdb_mouse_move_by(self._handle(), dx, dy, duration_ms, _profile_pointer(profile)))

    def move_raw(self, dx: int, dy: int) -> None:
        """One relative move of any size: split at the ±1024 report limit, no barrier.

        The caller flushes with ``device.barrier()`` where a completion guarantee
        matters, which is what makes this the cheap path for remote control.
        """
        _check(_D.gdb_move_raw(self._handle(), dx, dy))

    def move_to(self, x: int, y: int, duration_ms: int = 0, profile: Profile | None = None,
                screen: Screen | None = None) -> None:
        """An absolute move. Without `screen` it clamps to the primary monitor;
        `screen=Screen.monitor(1)` targets another one and a cursor sitting there
        is not an error."""
        if screen is None:
            _check(_D.gdb_mouse_move_to(self._handle(), x, y, duration_ms, _profile_pointer(profile)))
            return
        _check(_D.gdb_mouse_move_to_on(self._handle(), x, y, screen._raw(), duration_ms, _profile_pointer(profile)))

    def move_path(self, points: Sequence[Point], duration_ms: int = 0, profile: Profile | None = None,
                  screen: Screen | None = None) -> None:
        array, count = _points(points)
        if screen is None:
            _check(_D.gdb_mouse_move_path(self._handle(), array, count, duration_ms, _profile_pointer(profile)))
            return
        _check(_D.gdb_mouse_move_path_on(self._handle(), array, count, screen._raw(), duration_ms, _profile_pointer(profile)))

    def start_move_to(self, x: int, y: int, duration_ms: int = 0, profile: Profile | None = None) -> Movement:
        pointer = _D.gdb_mouse_start_move_to(self._handle(), x, y, duration_ms, _profile_pointer(profile))
        return _movement(pointer)

    def start_move_by(self, dx: int, dy: int, duration_ms: int = 0, profile: Profile | None = None) -> Movement:
        pointer = _D.gdb_mouse_start_move_by(self._handle(), dx, dy, duration_ms, _profile_pointer(profile))
        return _movement(pointer)

    def start_move_path(self, points: Sequence[Point], duration_ms: int = 0, profile: Profile | None = None) -> Movement:
        array, count = _points(points)
        pointer = _D.gdb_mouse_start_move_path(self._handle(), array, count, duration_ms, _profile_pointer(profile))
        return _movement(pointer)

    def button(self, button: int, down: bool) -> None:
        _check(_D.gdb_mouse_button(self._handle(), button, down))

    def click(self, button: int = Button.LEFT, count: int = 1) -> None:
        _check(_D.gdb_mouse_click(self._handle(), button, count))

    def click_timed(self, button: int = Button.LEFT, count: int = 1, hold_ms: int = 0,
                    gap_ms: int = 0, jitter_ms: int = 0) -> None:
        """Clicks with the press length the caller asked for.

        ``hold_ms`` is how long the button stays down and ``gap_ms`` how long to
        wait after the release; 0 for either draws that half the way ``click``
        does. ``jitter_ms`` spreads each half by up to that much in either
        direction, so 0 is an exact press.
        """
        _check(_D.gdb_mouse_click_timed(self._handle(), button, count, hold_ms, gap_ms, jitter_ms))

    def click_at(self, x: int, y: int, screen: Screen, button: int = Button.LEFT, count: int = 1,
                 hold_ms: int = 0, gap_ms: int = 0, jitter_ms: int = 0) -> None:
        """Moves to ``(x, y)`` on ``screen`` and clicks there, in one call."""
        _check(_D.gdb_mouse_click_at_on(
            self._handle(), x, y, screen._raw(), button, count, hold_ms, gap_ms, jitter_ms
        ))

    def double_click(self, button: int = Button.LEFT) -> None:
        _check(_D.gdb_mouse_double_click(self._handle(), button))

    def wheel(self, vertical: int, horizontal: int = 0) -> None:
        _check(_D.gdb_mouse_wheel(self._handle(), vertical, horizontal))

    def drag(self, button: int, x: int, y: int, duration_ms: int = 0, profile: Profile | None = None) -> None:
        _check(_D.gdb_mouse_drag(self._handle(), button, x, y, duration_ms, _profile_pointer(profile)))


def _points(points: Sequence[Point]):
    if not points:
        raise GdbError(ErrorCode.INVALID, "path needs at least one point")
    array = (_Point * len(points))()
    for index, (x, y) in enumerate(points):
        array[index] = _Point(x, y)
    return array, len(points)


def _movement(pointer: int | None) -> Movement:
    if not pointer:
        code, message = _last_error()
        raise GdbError(code or ErrorCode.IO, message or "movement did not start")
    return Movement(pointer)


class Keyboard:
    def __init__(self, device: "Device"):
        self._device = device

    def _handle(self) -> int:
        return self._device._handle()

    def key_down(self, page: int, usage: int) -> None:
        _check(_D.gdb_key_down(self._handle(), page, usage))

    def key_up(self, page: int, usage: int) -> None:
        _check(_D.gdb_key_up(self._handle(), page, usage))

    def press(self, page: int, usage: int) -> None:
        _check(_D.gdb_key_press(self._handle(), page, usage))

    def long_press(self, page: int, usage: int, hold_ms: int) -> None:
        _check(_D.gdb_key_long_press(self._handle(), page, usage, hold_ms))

    def chord(self, usages: Sequence[int], hold_ms: int = 60) -> None:
        values = (c_uint16 * len(usages))(*usages)
        _check(_D.gdb_keys_chord(self._handle(), values, len(usages), hold_ms))

    def sequence(self, steps: Sequence[KeyStep]) -> None:
        array = (_KeyStep * len(steps))(
            *[_KeyStep(step.page, step.usage, step.hold_ms, step.gap_ms) for step in steps]
        )
        _check(_D.gdb_keys_sequence(self._handle(), array, len(steps)))

    def type_text(self, text: str, interval_ms: int = 45, hold_ms: int = 18, jitter_ms: int = 30) -> None:
        _check(_D.gdb_type_text(self._handle(), text.encode("utf-8"), interval_ms, hold_ms, jitter_ms))

    def release_all(self) -> None:
        self._device.release_all()


# Task callbacks must stay referenced for as long as the scheduler lives.
_TASK_CALLBACK = ctypes.CFUNCTYPE(c_int32, c_void_p)
_EVENT_CALLBACK = ctypes.CFUNCTYPE(None, c_void_p, c_int32, c_int32, c_char_p)
_LOG_CALLBACK = ctypes.CFUNCTYPE(None, c_void_p, c_int32, c_char_p)
# A watch callback runs on the watch thread; ctypes acquires the GIL for it.
_WATCH_CALLBACK = ctypes.CFUNCTYPE(None, c_void_p, c_int32, c_char_p)

_D.gdb_watch_start.argtypes = [c_uint32, c_void_p, c_void_p]
_D.gdb_watch_start.restype = c_void_p
_D.gdb_watch_stop.argtypes = [c_void_p]
_D.gdb_watch_stop.restype = None
_D.gdb_watch_free.argtypes = [c_void_p]
_D.gdb_watch_free.restype = None
_D.gdb_watch_listing.argtypes = [c_void_p]
_D.gdb_watch_listing.restype = c_void_p


class Scheduler:
    """Task queue with priorities, delays, repeat, pause/resume and cancel."""

    def __init__(self, device: "Device"):
        self._device = device
        pointer = _D.gdb_scheduler_new(device._handle())
        if not pointer:
            code, message = _last_error()
            raise GdbError(code or ErrorCode.IO, message)
        self._pointer = pointer
        self._callbacks: dict[int, object] = {}

    def submit(self, delay_ms: int, priority: int, callback: TaskCallback) -> int:
        wrapped = _TASK_CALLBACK(lambda _user: int(callback()))
        task_id = _D.gdb_scheduler_submit(
            self._pointer, delay_ms, priority, ctypes.cast(wrapped, c_void_p), None
        )
        if task_id == 0:
            code, message = _last_error()
            raise GdbError(code or ErrorCode.IO, message or "submit failed")
        self._callbacks[task_id] = wrapped
        return task_id

    def elapsed_ms(self) -> int:
        """Milliseconds since the scheduler started; the unit `submit_at` takes."""
        return int(_D.gdb_scheduler_elapsed_ms(self._pointer))

    def submit_at(self, due_ms: int, priority: int, callback: TaskCallback) -> int:
        """Queues at an absolute due. Jobs sharing a due are ordered by priority,
        which `submit` cannot guarantee: it stamps the due from the clock, so two
        calls a millisecond apart never tie."""
        wrapped = _TASK_CALLBACK(lambda _user: int(callback()))
        task_id = _D.gdb_scheduler_submit_at(
            self._pointer, due_ms, priority, ctypes.cast(wrapped, c_void_p), None
        )
        if task_id == 0:
            code, message = _last_error()
            raise GdbError(code or ErrorCode.IO, message or "submit failed")
        self._callbacks[task_id] = wrapped
        return task_id

    def every(self, interval_ms: int, priority: int, callback: TaskCallback) -> int:
        wrapped = _TASK_CALLBACK(lambda _user: int(callback()))
        task_id = _D.gdb_scheduler_every(
            self._pointer, interval_ms, priority, ctypes.cast(wrapped, c_void_p), None
        )
        if task_id == 0:
            code, message = _last_error()
            raise GdbError(code or ErrorCode.IO, message or "schedule failed")
        self._callbacks[task_id] = wrapped
        return task_id

    def cancel(self, task_id: int) -> None:
        _check(_D.gdb_scheduler_cancel(self._pointer, task_id))

    def pause(self) -> None:
        _check(_D.gdb_scheduler_pause(self._pointer))

    def resume(self) -> None:
        _check(_D.gdb_scheduler_resume(self._pointer))

    def wait_idle(self, timeout_ms: int = 5_000) -> None:
        _check(_D.gdb_scheduler_wait_idle(self._pointer, timeout_ms))

    def take_errors(self) -> list[dict]:
        pointer = _D.gdb_scheduler_take_errors(self._pointer)
        return _take(pointer)

    def close(self) -> None:
        if getattr(self, "_pointer", 0):
            _D.gdb_scheduler_free(self._pointer)
            self._pointer = 0
            self._callbacks.clear()

    def __enter__(self) -> "Scheduler":
        return self

    def __exit__(self, *exc_info) -> None:
        self.close()

    def __del__(self) -> None:  # pragma: no cover - best effort
        self.close()


class Device:
    """One controller session."""

    def __init__(self, pointer: int):
        self._pointer = pointer
        self._events: list[object] = []
        self.mouse = Mouse(self)
        self.keyboard = Keyboard(self)

    @classmethod
    def open(
        cls,
        selector: str | None = None,
        timeout_ms: int = 1000,
        auto_reconnect: bool = True,
        heartbeat: bool = True,
        shared: bool = False,
    ) -> "Device":
        """Connect to a board.

        ``shared=True`` opens it for use alongside other programs: a reply that
        never arrives is one ``ErrorCode.BUSY`` naming the command instead of a
        timeout, the session is never reopened behind your back, and
        ``stolen_frames`` counts the replies that were not ours.
        """
        name = selector.encode("utf-8") if selector else None
        if shared:
            pointer = _D.gdb_open_shared(name, timeout_ms, auto_reconnect, heartbeat)
        else:
            pointer = _D.gdb_open(name, timeout_ms, auto_reconnect, heartbeat)
        if not pointer:
            code, message = _last_error()
            raise GdbError(code or ErrorCode.IO, message or "open failed")
        return cls(pointer)

    @classmethod
    def open_mock(
        cls,
        timeout_ms: int = 1000,
        auto_reconnect: bool = True,
        heartbeat: bool = True,
        shared: bool = False,
    ) -> "Device":
        if shared:
            pointer = _D.gdb_open_mock_shared(timeout_ms, auto_reconnect, heartbeat)
        else:
            pointer = _D.gdb_open_mock(timeout_ms, auto_reconnect, heartbeat)
        if not pointer:
            code, message = _last_error()
            raise GdbError(code or ErrorCode.IO, message or "mock open failed")
        return cls(pointer)

    @staticmethod
    def list_devices() -> list[dict]:
        return _take(_D.gdb_device_list())

    @property
    def info(self) -> dict:
        return _take(_D.gdb_device_info(self._handle()))

    @property
    def state(self) -> int:
        value = _D.gdb_device_state(self._handle())
        if value < 0:
            raise GdbError(ErrorCode.INVALID, "device handle is NULL")
        return value

    def metrics(self) -> Metrics:
        """The session's counters and latency percentiles."""
        out = Metrics()
        _check(_D.gdb_metrics_get(self._handle(), ctypes.byref(out)))
        return out

    def reset_metrics(self) -> None:
        _check(_D.gdb_metrics_reset(self._handle()))

    def clamps(self) -> Clamps:
        """The absolute moves the anchor rectangle pulled back, and the last
        one's coordinates: the numbers that say a requested pixel was never
        reached. Meaningful when ``clamped_total`` is not zero."""
        out = Clamps()
        _check(_D.gdb_metrics_clamps(self._handle(), ctypes.byref(out)))
        return out

    @property
    def status(self) -> dict:
        return _take(_D.gdb_status(self._handle()))

    @property
    def status_ext(self) -> dict:
        return _take(_D.gdb_status_ext(self._handle()))

    @property
    def sessions(self) -> list[dict]:
        return _take(_D.gdb_sessions(self._handle()))

    @property
    def stolen_frames(self) -> int:
        """Replies the reader looked past because they answered another session
        or sequence: the counter that tells a collision from a slow board."""
        return _D.gdb_stolen_frames(self._handle())

    def last_error(self) -> tuple[int, str]:
        """The latest failure the device recorded, whoever called it.

        Returns ``(code, message)``: the message ``gdb_last_error`` would have
        carried on the thread that failed, plus the failing opcode, the session
        and the last STATUS the device answered. Unlike the thread-local error,
        a success does not clear it, so a command that failed on a scheduler
        worker is still readable here. ``(0, "")`` means no failure yet.
        """
        buffer = ctypes.create_string_buffer(512)
        code = _D.gdb_device_last_error(self._handle(), buffer, len(buffer))
        return code, buffer.value.decode("utf-8", "replace")

    def release_all(self) -> None:
        _check(_D.gdb_release_all(self._handle()))

    def barrier(self) -> None:
        _check(_D.gdb_barrier(self._handle()))

    def bootloader(self) -> None:
        _check(_D.gdb_bootloader(self._handle()))

    def command(self, opcode: int, payload: bytes = b"") -> Packet | None:
        """Sends one protocol command by opcode and payload; None when it has no reply.

        The raw escape hatch for a host that already speaks the wire protocol:
        same session, sequence numbers and error codes as the typed methods, but
        the SDK's own bookkeeping (held buttons, the mock cursor) is not updated.
        """
        buffer = (c_uint8 * len(payload)).from_buffer_copy(payload) if payload else None
        out = _Packet()
        _check(_D.gdb_command(self._handle(), opcode, buffer, len(payload), byref(out)))
        try:
            if out.opcode == 0:
                return None
            data = ctypes.string_at(out.payload, out.len) if out.len else b""
            return Packet(out.opcode, out.flags, out.session, out.sequence, data)
        finally:
            _D.gdb_packet_free(byref(out))

    def reconnect(self) -> None:
        _check(_D.gdb_reconnect(self._handle()))

    def set_auto_reconnect(self, enabled: bool) -> None:
        _check(_D.gdb_set_auto_reconnect(self._handle(), enabled))

    def set_event_handler(self, callback: EventCallback | None) -> None:
        if callback is None:
            _check(_D.gdb_set_event_handler(self._handle(), None, None))
            self._events.clear()
            return
        wrapped = _EVENT_CALLBACK(
            lambda _user, kind, code, message: callback(
                kind, code, message.decode("utf-8", "replace") if message else ""
            )
        )
        self._events.append(wrapped)
        _check(
            _D.gdb_set_event_handler(
                self._handle(), ctypes.cast(wrapped, c_void_p), None
            )
        )

    def scheduler(self) -> Scheduler:
        return Scheduler(self)

    def close(self) -> None:
        if self._pointer:
            try:
                _D.gdb_close(self._pointer)
            finally:
                _D.gdb_device_free(self._pointer)
                self._pointer = 0

    def _handle(self) -> int:
        if not self._pointer:
            raise GdbError(ErrorCode.INVALID, "device is closed")
        return self._pointer

    def __enter__(self) -> "Device":
        return self

    def __exit__(self, *exc_info) -> None:
        self.close()

    def __del__(self) -> None:  # pragma: no cover - best effort
        if getattr(self, "_pointer", 0):
            _D.gdb_device_free(self._pointer)
            self._pointer = 0


class Remote:
    """A client of a broker process.

    Only one program may read the vendor collection, so one program (a broker,
    ``gdb::broker::Server`` in the Rust SDK) owns the device and everyone else
    attaches over ``\\\\.\\pipe\\gdbox-<name>``:

        with gdb.Remote() as remote:          # the "default" pipe
            print(remote.status["leaseMs"])
            remote.move_raw(400, -120)
            remote.barrier()

    The JSON accessors return exactly what :class:`Device` returns, so one parser
    serves both. What an attached handle cannot do is what needs the collection
    itself: absolute moves, drags, typing, chords, flashing and the bootloader.
    """

    def __init__(self, name: str | None = None):
        handle = c_void_p()
        _check(_D.gdb_attach(name.encode("utf-8") if name else None, byref(handle)))
        if not handle.value:
            raise GdbError(ErrorCode.IO, "attach returned OK without a handle")
        self._pointer = handle.value
        self.name = name or "default"

    @classmethod
    def attach(cls, name: str | None = None) -> "Remote":
        return cls(name)

    @staticmethod
    def spawn(name: str | None = None) -> None:
        """Starts a broker when none is listening; attach afterwards.

        The executable comes from ``GDBOX_BROKER_EXE``, a host that serves
        ``--gdbox-broker <name>``. Without it there is nothing to start, so this
        raises ``UNSUPPORTED``; a broker that is already listening is a no-op.
        """
        _check(_D.gdb_remote_spawn(name.encode("utf-8") if name else None))

    @property
    def info(self) -> dict:
        return _take(_D.gdb_remote_info(self._handle()))

    @property
    def status(self) -> dict:
        return _take(_D.gdb_remote_status(self._handle()))

    @property
    def status_ext(self) -> dict:
        return _take(_D.gdb_remote_status_ext(self._handle()))

    @property
    def sessions(self) -> list[dict]:
        return _take(_D.gdb_remote_sessions(self._handle()))

    def key(self, page: int, usage: int, down: bool = True) -> None:
        _check(_D.gdb_remote_key(self._handle(), page, usage, down))

    def mouse_move(self, dx: int, dy: int) -> None:
        """One MOUSE_MOVE report; the deltas must fit the ±1024 of the protocol."""
        _check(_D.gdb_remote_mouse_move(self._handle(), dx, dy))

    def move_by(self, dx: int, dy: int, duration_ms: int = 0, profile: Profile | None = None) -> None:
        _check(_D.gdb_remote_move_by(self._handle(), dx, dy, duration_ms, _profile_pointer(profile)))

    def move_raw(self, dx: int, dy: int) -> None:
        _check(_D.gdb_remote_move_raw(self._handle(), dx, dy))

    def mouse_buttons(self, bits: int) -> None:
        """Raw HID mask: left=1, right=2, middle=4, back=8, forward=16."""
        _check(_D.gdb_remote_mouse_buttons(self._handle(), bits))

    def wheel(self, vertical: int, horizontal: int = 0) -> None:
        _check(_D.gdb_remote_wheel(self._handle(), vertical, horizontal))

    def barrier(self) -> None:
        _check(_D.gdb_remote_barrier(self._handle()))

    def release_all(self) -> None:
        _check(_D.gdb_remote_release_all(self._handle()))

    def command(self, opcode: int, payload: bytes = b"") -> Packet | None:
        """``Device.command`` over the pipe: same opcode, same reply shape."""
        buffer = (c_uint8 * len(payload)).from_buffer_copy(payload) if payload else None
        out = _Packet()
        _check(_D.gdb_remote_command(self._handle(), opcode, buffer, len(payload), byref(out)))
        try:
            if out.opcode == 0:
                return None
            data = ctypes.string_at(out.payload, out.len) if out.len else b""
            return Packet(out.opcode, out.flags, out.session, out.sequence, data)
        finally:
            _D.gdb_packet_free(byref(out))

    def close(self) -> None:
        if self._pointer:
            try:
                _D.gdb_remote_close(self._pointer)
            finally:
                _D.gdb_remote_free(self._pointer)
                self._pointer = 0

    def _handle(self) -> int:
        if not self._pointer:
            raise GdbError(ErrorCode.INVALID, "remote is closed")
        return self._pointer

    def __enter__(self) -> "Remote":
        return self

    def __exit__(self, *exc_info) -> None:
        self.close()

    def __del__(self) -> None:  # pragma: no cover - best effort
        if getattr(self, "_pointer", 0):
            _D.gdb_remote_free(self._pointer)
            self._pointer = 0


def probe(selector: str | None = None) -> dict:
    """The board's identity from Feature 14 alone: no session, no packet.

    Uses the same selector rules as ``Device.open``. Because a feature report is a
    control transfer, this is safe while another program holds the session - which
    is the whole point of it. The dict has the same keys as ``Device.info``.
    """
    return _take(_D.gdb_probe(selector.encode("utf-8") if selector else None))


def probe_all() -> list[dict]:
    """Every attached board, one Feature 14 read each. An empty bus is ``[]``."""
    return _take(_D.gdb_probe_all())


def monitors() -> list[dict]:
    """Every monitor, in exactly the order ``Screen.monitor`` indexes them.

    Each entry is ``{"left", "top", "right", "bottom", "primary", "dpi"}`` in
    virtual-desktop physical pixels. ``primary`` marks the entry ``Screen.primary``
    resolves to and ``dpi`` is the effective DPI (96 = 100%, 0 when the system
    will not say). No device handle is needed and no packet is sent, so this is
    safe beside a session; read it instead of guessing which index a display is.
    """
    return _take(_D.gdb_monitors_json())


def window_metrics(hwnd: int) -> dict:
    """One window as the system reports it.

    The dict is ``{"rect", "clientOrigin", "clientSize", "dpi", "visible",
    "minimized", "foreground"}``: ``rect`` is the frame ``Screen.window`` clamps
    into, and ``clientOrigin``/``clientSize`` describe the area inside it, which
    is what ``Screen.window_client`` uses and what a capture shows. Raises
    ``UNSUPPORTED`` where the platform has no window APIs and ``STALE_WINDOW``
    for a handle that does not resolve. No device handle is needed.
    """
    return _take(_D.gdb_window_metrics(hwnd))


def prepare_platform() -> dict:
    """Raises the process to per-monitor DPI awareness and reports the mode.

    Returns ``{"supported", "dpiAwareness", "value", "perMonitor"}``. A host that
    turns a screenshot coordinate into a screen coordinate has to agree with the
    SDK about the scale factor, and this is what decides it; safe more than once
    and safe beside an open device. Every ``Device.open*`` already calls it.
    """
    return _take(_D.gdb_platform_prepare())


class Observer:
    """Presence without a session: Feature 14 on a timer, and no way to command.

        with gdb.Observer(interval_ms=500) as observer:
            print(observer.info)          # same keys as Device.info

    Use this instead of ``Device`` when another program owns the session. Every
    command raises ``GdbError`` with ``ErrorCode.UNSUPPORTED``: the handle has no
    session and must not put a frame on the IN queue the owner is reading.
    """

    def __init__(self, selector: str | None = None, interval_ms: int = 500):
        pointer = _D.gdb_observer_start(
            selector.encode("utf-8") if selector else None, interval_ms
        )
        if not pointer:
            code, message = _last_error()
            raise GdbError(code or ErrorCode.IO, message or "no board to observe")
        self._pointer = pointer

    @property
    def info(self) -> dict:
        return _take(_D.gdb_observer_info(self._handle()))

    @property
    def state(self) -> int:
        value = _D.gdb_observer_state(self._handle())
        if value < 0:
            raise GdbError(ErrorCode.INVALID, "observer handle is NULL")
        return value

    def stop(self) -> None:
        if self._pointer:
            _D.gdb_observer_stop(self._pointer)

    def close(self) -> None:
        if self._pointer:
            try:
                _D.gdb_observer_stop(self._pointer)
            finally:
                _D.gdb_observer_free(self._pointer)
                self._pointer = 0

    def status(self) -> None:
        _check(_D.gdb_observer_status(self._handle()))

    def barrier(self) -> None:
        _check(_D.gdb_observer_barrier(self._handle()))

    def release_all(self) -> None:
        _check(_D.gdb_observer_release_all(self._handle()))

    def command(self, opcode: int, payload: bytes = b"") -> None:
        buffer = (c_uint8 * len(payload)).from_buffer_copy(payload) if payload else None
        out = _Packet()
        try:
            _check(
                _D.gdb_observer_command(
                    self._handle(), opcode, buffer, len(payload), byref(out)
                )
            )
        finally:
            _D.gdb_packet_free(byref(out))

    def _handle(self) -> int:
        if not self._pointer:
            raise GdbError(ErrorCode.INVALID, "observer is closed")
        return self._pointer

    def __enter__(self) -> "Observer":
        return self

    def __exit__(self, *exc_info) -> None:
        self.close()

    def __del__(self) -> None:  # pragma: no cover - best effort
        if getattr(self, "_pointer", 0):
            _D.gdb_observer_free(self._pointer)
            self._pointer = 0


class Watch:
    """Device arrivals and removals, without polling the bus yourself.

        def seen(kind, event):
            print(kind, event)          # gdb.Watch.ARRIVED, {"serial": ...}

        with gdb.watch(seen, interval_ms=250) as watch:
            print(watch.listing)        # {"devices": [...], "volumes": [...]}

    ``kind`` is ``Watch.ARRIVED``, ``Watch.REMOVED`` or ``Watch.BOOTLOADER``;
    ``event`` is the object the C ABI sent - a device entry
    (``path``/``serial``/``product``/``manufacturer``/``vid``/``pid``) or
    ``{"path": "E:\\\\"}`` for an RP2040 bootloader drive. The first poll reports
    what is already attached, so a program that starts late still sees the bus.

    The callback runs on the watch thread: it must return quickly (it is the only
    thread polling) and must not call ``stop``, which waits for that same thread.
    The dict belongs to the call, so copy anything that outlives it. A raise is
    reported by ctypes and swallowed, and the watch keeps running.
    """

    ARRIVED = 0
    REMOVED = 1
    BOOTLOADER = 2

    def __init__(self, callback: WatchCallback | None, interval_ms: int = 500):
        # ctypes keeps no reference to a callback, and the watch thread calls
        # straight through that address: this object must outlive the thread.
        self._callback = (
            _WATCH_CALLBACK(
                lambda _user, kind, text: _deliver_watch_event(callback, kind, text)
            )
            if callback is not None
            else None
        )
        pointer = _D.gdb_watch_start(
            interval_ms,
            ctypes.cast(self._callback, c_void_p) if self._callback else None,
            None,
        )
        if not pointer:
            code, message = _last_error()
            raise GdbError(code or ErrorCode.IO, message or "watch did not start")
        self._pointer = pointer

    @property
    def listing(self) -> dict:
        """The bus right now: ``{"devices": [...], "volumes": [...]}``."""
        return _take(_D.gdb_watch_listing(self._handle()))

    def stop(self) -> None:
        """Stops and joins the thread; the handle stays valid."""
        if self._pointer:
            _D.gdb_watch_stop(self._pointer)

    def close(self) -> None:
        if self._pointer:
            try:
                _D.gdb_watch_stop(self._pointer)
            finally:
                _D.gdb_watch_free(self._pointer)
                self._pointer = 0

    def _handle(self) -> int:
        if not self._pointer:
            raise GdbError(ErrorCode.INVALID, "watch is closed")
        return self._pointer

    def __enter__(self) -> "Watch":
        return self

    def __exit__(self, *exc_info) -> None:
        self.close()

    def __del__(self) -> None:  # pragma: no cover - best effort
        if getattr(self, "_pointer", 0):
            _D.gdb_watch_free(self._pointer)
            self._pointer = 0


def _deliver_watch_event(callback: WatchCallback, kind: int, text: bytes | None) -> None:
    try:
        event = json.loads((text or b"{}").decode("utf-8"))
    except ValueError:
        event = {}
    callback(kind, event)


def watch(callback: WatchCallback, interval_ms: int = 500) -> Watch:
    """Watches the bus for arrivals and removals: ``gdb.watch(seen, 250)``."""
    return Watch(callback, interval_ms)


def set_log_level(level: int) -> None:
    _D.gdb_set_log_level(level)


def set_log_sink(callback: LogCallback | None) -> None:
    global _LOG_SINK
    if callback is None:
        _LOG_SINK = None
        _D.gdb_set_log_sink(None, None)
        return

    def forward(_user, level, message):
        callback(level, message.decode("utf-8", "replace") if message else "")

    _LOG_SINK = _LOG_CALLBACK(forward)
    _D.gdb_set_log_sink(ctypes.cast(_LOG_SINK, c_void_p), None)


_LOG_SINK = None
_D.gdb_set_log_sink.argtypes = [c_void_p, c_void_p]
_D.gdb_set_log_sink.restype = c_int32
