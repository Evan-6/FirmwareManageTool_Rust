"""Behaviour tests for the Python binding; no hardware required.

    python sdk/python/test_gdb.py
"""

from __future__ import annotations

import json
import os
import sys
import threading
import time
import unittest

import gdb


class DeviceTests(unittest.TestCase):
    def test_abi_and_version(self):
        self.assertEqual(gdb.LIBRARY.gdb_abi_version(), gdb.ABI_VERSION)
        self.assertEqual(gdb.LIBRARY.gdb_version(), b"0.1.0")
        self.assertEqual(gdb.abi_versions(), (gdb.ABI_VERSION, gdb.ABI_VERSION_MINOR))

    def test_the_capability_list_describes_this_build(self):
        listed = gdb.capabilities()
        self.assertEqual(
            set(listed),
            {"click_timed", "click_at_on", "window_metrics", "window_client_screen",
             "pursuit", "move_report", "event_json", "scheduler", "broker", "replay",
             "flash"},
        )
        for name, value in listed.items():
            self.assertIsInstance(value, bool, name)
        for name in ("click_timed", "pursuit", "scheduler", "broker"):
            self.assertTrue(listed[name], name)
        self.assertFalse(listed["replay"], "replay has no C symbol")
        self.assertFalse(listed["flash"], "flash has no C symbol")

    def test_shared_open_reports_the_stolen_frames(self):
        with gdb.Device.open_mock(shared=True) as device:
            self.assertEqual(device.stolen_frames, 0)
            self.assertEqual(device.status["pending"], 0)
            self.assertEqual(device.state, gdb.ConnectionState.CONNECTED)
            # A reply of our own is not a stolen frame.
            self.assertEqual(device.stolen_frames, 0)

    def test_mock_reports_the_firmware_contract(self):
        with gdb.Device.open_mock() as device:
            info = device.info
            self.assertEqual(info["board"], 2)
            self.assertEqual(info["firmware"], "3.2.0")
            self.assertEqual(info["leaseMs"], 2000)
            self.assertEqual(info["pollMs"], 1)
            self.assertEqual(info["capabilities"], 15)
            self.assertEqual(device.state, gdb.ConnectionState.CONNECTED)

            status = device.status
            self.assertEqual(status["leaseMs"], 2000)
            self.assertEqual(status["pending"], 0)
            self.assertIn("state", status)

            ext = device.status_ext
            self.assertGreater(ext["uptimeMs"], 0)
            self.assertEqual(ext["bootCount"], 1)

            sessions = device.sessions
            self.assertEqual(len(sessions), 1)
            self.assertIn("held", sessions[0])

    def test_mock_device_is_listed_by_identity_only(self):
        devices = gdb.Device.list_devices()
        self.assertIsInstance(devices, list)
        for entry in devices:
            self.assertIn("serial", entry)

    def test_keyboard_and_text(self):
        with gdb.Device.open_mock() as device:
            device.keyboard.key_down(7, 0x04)
            device.keyboard.key_up(7, 0x04)
            device.keyboard.press(7, 0x05)
            device.keyboard.long_press(7, 0x06, 5)
            device.keyboard.chord([0xE0, 0x06], 5)
            device.keyboard.sequence(
                [gdb.KeyStep(page=7, usage=0x04, hold_ms=2, gap_ms=1)]
            )
            device.keyboard.type_text("hello", interval_ms=1, hold_ms=1, jitter_ms=0)
            device.release_all()

    def test_timed_and_positioned_clicks(self):
        with gdb.Device.open_mock() as device:
            started = time.monotonic()
            device.mouse.click_timed(gdb.Button.LEFT, 1, hold_ms=40, gap_ms=0)
            self.assertGreaterEqual(time.monotonic() - started, 0.04)

            device.mouse.click_at(300, 200, gdb.Screen.primary, gdb.Button.RIGHT, 1,
                                  hold_ms=1, gap_ms=0)
            with self.assertRaises(gdb.GdbError) as caught:
                device.mouse.click_timed(gdb.Button.LEFT, 0, hold_ms=1)
            self.assertEqual(caught.exception.code, gdb.ErrorCode.INVALID)

    def test_invalid_usage_is_a_typed_error(self):
        with gdb.Device.open_mock() as device:
            with self.assertRaises(gdb.GdbError) as caught:
                device.keyboard.key_down(7, 0x01)
            self.assertEqual(caught.exception.code, gdb.ErrorCode.INVALID)

    def test_mouse_surface(self):
        with gdb.Device.open_mock() as device:
            device.mouse.move_by(120, -80, duration_ms=20)
            device.mouse.move_path([(10, 10), (20, 20)], duration_ms=10)
            device.mouse.move_to_on_pursuit(1400, 300, gdb.Screen.primary)
            report = device.mouse.move_to_report(420, 260, gdb.Screen.primary, duration_ms=20)
            self.assertEqual((report.requested_x, report.requested_y), (420, 260))
            self.assertFalse(report.clamped)
            self.assertLessEqual(report.error_px, 1)
            self.assertGreater(report.elapsed_us, 0)
            device.mouse.button(gdb.Button.LEFT, True)
            device.mouse.button(gdb.Button.LEFT, False)
            device.mouse.click(gdb.Button.RIGHT, 2)
            device.mouse.double_click(gdb.Button.MIDDLE)
            device.mouse.wheel(120, -120)
            with self.assertRaises(gdb.GdbError) as caught:
                device.mouse.wheel(4000, 0)
            self.assertEqual(caught.exception.code, gdb.ErrorCode.INVALID)
            with self.assertRaises(gdb.GdbError):
                device.mouse.button(9, True)

    def test_movement_can_be_cancelled_and_retargeted(self):
        with gdb.Device.open_mock() as device:
            movement = device.mouse.start_move_by(400, 400, duration_ms=400)
            movement.cancel()
            with self.assertRaises(gdb.GdbError) as caught:
                movement.wait(2000)
            self.assertEqual(caught.exception.code, gdb.ErrorCode.CANCELLED)
            movement.close()

            movement = device.mouse.start_move_by(100, 0, duration_ms=50)
            movement.wait(5000)
            self.assertTrue(movement.finished)
            movement.close()

    def test_scheduler_orders_by_priority_and_cancels(self):
        # Priority orders jobs that share a due, and `pause` acknowledges: it
        # returns only once the worker has read the flag, so both submits below
        # are in the heap before `resume`. The due is explicit for the same
        # reason — `submit` stamps it from the clock, so two calls a millisecond
        # apart would not tie and the low job would legitimately run first.
        with gdb.Device.open_mock() as device:
            scheduler = device.scheduler()
            order: list[str] = []
            scheduler.pause()
            due = scheduler.elapsed_ms() + 50
            scheduler.submit_at(due, 0, lambda: order.append("low") or 0)
            scheduler.submit_at(due, 10, lambda: order.append("high") or 0)
            scheduler.resume()
            scheduler.wait_idle(2000)
            self.assertEqual(order, ["high", "low"])

            cancelled: list[int] = []
            scheduler.pause()
            task = scheduler.submit(0, 0, lambda: cancelled.append(1) or 0)
            scheduler.cancel(task)
            scheduler.resume()
            scheduler.wait_idle(2000)
            self.assertEqual(cancelled, [])
            scheduler.close()

    def test_scheduler_repeats_and_reports_errors(self):
        with gdb.Device.open_mock() as device:
            scheduler = device.scheduler()
            ticks = []
            task = scheduler.every(10, 0, lambda: ticks.append(1) or 0)
            # The worker is a thread of its own and its first tick is not
            # synchronised with this one: a freshly spawned thread was measured
            # more than a second late on Windows. Wait for the tick, then time the
            # repeats from a tick that is known to have happened — a task that
            # never repeats still fails, and the 80 ms window still has to hold
            # one more 10 ms tick on top of the first.
            deadline = time.monotonic() + 5
            while not ticks and time.monotonic() < deadline:
                time.sleep(0.005)
            self.assertTrue(ticks, "the repeating task never ran")
            time.sleep(0.08)
            scheduler.cancel(task)
            self.assertGreaterEqual(len(ticks), 2)

            scheduler.submit(0, 0, lambda: gdb.ErrorCode.INVALID)
            scheduler.wait_idle(2000)
            errors = scheduler.take_errors()
            self.assertEqual(len(errors), 1)
            self.assertEqual(errors[0]["code"], gdb.ErrorCode.INVALID)
            scheduler.close()

    def test_events_fire_on_close(self):
        device = gdb.Device.open_mock()
        seen: list[tuple[int, int, str]] = []
        device.set_event_handler(lambda kind, code, message: seen.append((kind, code, message)))
        device.close()
        deadline = time.time() + 2
        while not seen and time.time() < deadline:
            time.sleep(0.01)
        self.assertTrue(any(kind == 0 and code == gdb.ConnectionState.DISCONNECTED for kind, code, _ in seen))

    def test_json_events_carry_the_state_and_the_status(self):
        device = gdb.Device.open_mock(timeout_ms=200)
        seen: list[tuple[int, dict]] = []
        device.set_event_handler_json(lambda kind, body: seen.append((kind, body)))
        _ = device.status  # a summary for the payload to carry
        device.close()
        deadline = time.monotonic() + 2
        while not seen and time.monotonic() < deadline:
            time.sleep(0.01)
        self.assertTrue(seen, "closing the device produced no JSON event")
        kind, body = seen[0]
        self.assertEqual(kind, 0)
        self.assertEqual(body["state"], gdb.ConnectionState.DISCONNECTED)
        self.assertIsInstance(body["status"], dict)
        self.assertIn("received", body["status"])

    def test_missing_selector_is_disconnected(self):
        with self.assertRaises(gdb.GdbError) as caught:
            gdb.Device.open("no-such-serial")
        self.assertEqual(caught.exception.code, gdb.ErrorCode.DISCONNECTED)

    def test_closed_device_rejects_calls(self):
        device = gdb.Device.open_mock()
        device.close()
        with self.assertRaises(gdb.GdbError):
            _ = device.info

    def test_command_sends_a_frame_and_returns_the_reply(self):
        with gdb.Device.open_mock() as device:
            reply = device.command(0x06)  # STATUS
            self.assertIsInstance(reply, gdb.Packet)
            self.assertEqual(reply.opcode, 0x86)
            self.assertEqual(len(reply.payload), 51)
            self.assertEqual(reply.payload[0], 0)
            self.assertEqual(reply.flags, 0)

            # An input command is fire and forget: no reply at all.
            self.assertIsNone(device.command(0x12, b"\x2c\x01\xd8\xff"))

            with self.assertRaises(gdb.GdbError) as caught:
                device.command(0x7F)
            self.assertEqual(caught.exception.code, gdb.ErrorCode.INVALID)

    def test_move_raw_chunks_large_deltas(self):
        with gdb.Device.open_mock() as device:
            device.mouse.move_raw(2400, -1200)
            device.mouse.move_raw(-30_000, 30_000)
            device.barrier()
            self.assertEqual(device.state, gdb.ConnectionState.CONNECTED)

    def test_log_sink_receives_a_line(self):
        lines: list[str] = []
        gdb.set_log_level(5)
        gdb.set_log_sink(lambda _level, message: lines.append(message))
        try:
            with gdb.Device.open_mock():
                pass
        finally:
            gdb.set_log_sink(None)
            gdb.set_log_level(0)
        self.assertTrue(lines)
        self.assertTrue(any("opcode=" in line for line in lines))


class RemoteTests(unittest.TestCase):
    """The attached surface. No broker runs here, so these cover the contract a
    caller can rely on without one: the symbols, the header, and the typed errors
    that stand in for a broker that is not listening."""

    SYMBOLS = (
        "gdb_attach",
        "gdb_remote_spawn",
        "gdb_remote_free",
        "gdb_remote_info",
        "gdb_remote_status",
        "gdb_remote_status_ext",
        "gdb_remote_sessions",
        "gdb_remote_key",
        "gdb_remote_mouse_move",
        "gdb_remote_move_by",
        "gdb_remote_move_raw",
        "gdb_remote_mouse_buttons",
        "gdb_remote_wheel",
        "gdb_remote_barrier",
        "gdb_remote_release_all",
        "gdb_remote_command",
        "gdb_remote_close",
    )

    def test_the_library_exports_every_attached_symbol(self):
        self.assertEqual(gdb.ABI_VERSION_MINOR, 9)
        for name in self.SYMBOLS:
            self.assertTrue(hasattr(gdb.LIBRARY, name), f"gdb.dll has no {name}")

    def test_the_header_declares_the_attached_surface(self):
        here = os.path.dirname(os.path.abspath(__file__))
        root = os.path.abspath(os.path.join(here, "..", ".."))
        path = os.path.join(root, "sdk", "ffi", "include", "gdb.h")
        with open(path, encoding="utf-8") as handle:
            header = handle.read()
        self.assertIn("typedef struct gdb_remote gdb_remote;", header)
        self.assertIn("GDB_ABI_VERSION_MINOR", header)
        for name in self.SYMBOLS:
            self.assertIn(f"{name}(", header, f"gdb.h does not declare {name}")

    def test_attach_without_a_broker_is_disconnected(self):
        with self.assertRaises(gdb.GdbError) as caught:
            gdb.Remote(f"python-test-no-broker-{os.getpid()}")
        self.assertEqual(caught.exception.code, gdb.ErrorCode.DISCONNECTED)

    def test_attach_rejects_a_bad_name(self):
        with self.assertRaises(gdb.GdbError) as caught:
            gdb.Remote("nested/name")
        self.assertEqual(caught.exception.code, gdb.ErrorCode.INVALID)

    def test_spawn_without_an_executable_is_unsupported(self):
        saved = os.environ.pop("GDBOX_BROKER_EXE", None)
        try:
            with self.assertRaises(gdb.GdbError) as caught:
                gdb.Remote.spawn(f"python-test-no-broker-{os.getpid()}")
            self.assertEqual(caught.exception.code, gdb.ErrorCode.UNSUPPORTED)
        finally:
            if saved is not None:
                os.environ["GDBOX_BROKER_EXE"] = saved


class ProbeTests(unittest.TestCase):
    """Presence: Feature 14 only, which is what lets a second program see the
    board while another one owns the session. The symbol, header and typed-error
    checks need no board; the live one skips when the bus is empty."""

    SYMBOLS = (
        "gdb_probe",
        "gdb_probe_all",
        "gdb_observer_start",
        "gdb_observer_stop",
        "gdb_observer_free",
        "gdb_observer_info",
        "gdb_observer_state",
        "gdb_observer_status",
        "gdb_observer_barrier",
        "gdb_observer_release_all",
        "gdb_observer_command",
    )

    def test_the_library_exports_every_presence_symbol(self):
        self.assertEqual(gdb.ABI_VERSION_MINOR, 9)
        for name in self.SYMBOLS:
            self.assertTrue(hasattr(gdb.LIBRARY, name), f"gdb.dll has no {name}")

    def test_the_header_declares_the_presence_surface(self):
        here = os.path.dirname(os.path.abspath(__file__))
        root = os.path.abspath(os.path.join(here, "..", ".."))
        path = os.path.join(root, "sdk", "ffi", "include", "gdb.h")
        with open(path, encoding="utf-8") as handle:
            header = handle.read()
        self.assertIn("typedef struct gdb_observer gdb_observer;", header)
        self.assertIn("#define GDB_ABI_VERSION_MINOR 9u", header)
        for name in self.SYMBOLS:
            self.assertIn(f"{name}(", header, f"gdb.h does not declare {name}")

    def test_the_library_exports_every_shared_symbol(self):
        symbols = ("gdb_open_shared", "gdb_open_mock_shared", "gdb_stolen_frames")
        self.assertEqual(gdb.ABI_VERSION_MINOR, 9)
        for name in symbols:
            self.assertTrue(hasattr(gdb.LIBRARY, name), f"gdb.dll has no {name}")
        here = os.path.dirname(os.path.abspath(__file__))
        root = os.path.abspath(os.path.join(here, "..", ".."))
        path = os.path.join(root, "sdk", "ffi", "include", "gdb.h")
        with open(path, encoding="utf-8") as handle:
            header = handle.read()
        for name in symbols:
            self.assertIn(f"{name}(", header, f"gdb.h does not declare {name}")

    def test_a_selector_that_matches_nothing_is_disconnected(self):
        with self.assertRaises(gdb.GdbError) as caught:
            gdb.probe("no-such-serial")
        self.assertEqual(caught.exception.code, gdb.ErrorCode.DISCONNECTED)
        self.assertTrue(caught.exception.message)

    def test_probe_all_is_a_list_of_the_same_shape_as_info(self):
        probes = gdb.probe_all()
        self.assertIsInstance(probes, list)
        with gdb.Device.open_mock() as device:
            keys = set(device.info)
        for probe in probes:
            self.assertEqual(set(probe), keys, "a probe is the info object")
            self.assertEqual(probe["board"], 2)

    def test_an_observer_cannot_start_without_a_board(self):
        with self.assertRaises(gdb.GdbError) as caught:
            gdb.Observer("no-such-serial", interval_ms=100)
        self.assertEqual(caught.exception.code, gdb.ErrorCode.DISCONNECTED)

    def test_an_observer_refuses_every_command(self):
        if not gdb.probe_all():
            self.skipTest("no board attached, so there is nothing to observe")
        with gdb.Observer(interval_ms=100) as observer:
            self.assertEqual(observer.info["board"], 2)
            self.assertEqual(observer.state, gdb.ConnectionState.CONNECTED)
            calls = (
                observer.status,
                observer.barrier,
                observer.release_all,
                lambda: observer.command(0x06),
            )
            for call in calls:
                with self.assertRaises(gdb.GdbError) as caught:
                    call()
                self.assertEqual(caught.exception.code, gdb.ErrorCode.UNSUPPORTED)
                self.assertIn("Feature 14", caught.exception.message)


class WatchTests(unittest.TestCase):
    """The arrival watch. It needs no board: with nothing attached the callback
    simply never fires, which is also what "no change" must look like."""

    SYMBOLS = (
        "gdb_watch_start",
        "gdb_watch_stop",
        "gdb_watch_free",
        "gdb_watch_listing",
    )

    def test_the_library_exports_every_watch_symbol(self):
        self.assertEqual(gdb.ABI_VERSION_MINOR, 9)
        for name in self.SYMBOLS:
            self.assertTrue(hasattr(gdb.LIBRARY, name), f"gdb.dll has no {name}")

    def test_the_header_declares_the_watch_surface(self):
        here = os.path.dirname(os.path.abspath(__file__))
        root = os.path.abspath(os.path.join(here, "..", ".."))
        path = os.path.join(root, "sdk", "ffi", "include", "gdb.h")
        with open(path, encoding="utf-8") as handle:
            header = handle.read()
        self.assertIn("typedef struct gdb_watch gdb_watch;", header)
        self.assertIn("typedef void (*gdb_watch_fn)", header)
        for name in self.SYMBOLS:
            self.assertIn(f"{name}(", header, f"gdb.h does not declare {name}")

    def test_a_null_callback_is_invalid(self):
        with self.assertRaises(gdb.GdbError) as caught:
            gdb.Watch(None, interval_ms=100)
        self.assertEqual(caught.exception.code, gdb.ErrorCode.INVALID)

    def test_the_listing_is_the_bus_right_now(self):
        with gdb.watch(lambda kind, event: None, interval_ms=100) as watched:
            listing = watched.listing
        self.assertIsInstance(listing["devices"], list)
        self.assertIsInstance(listing["volumes"], list)
        for entry in listing["devices"]:
            self.assertIn("serial", entry)
        # A closed handle is a typed refusal, not a crash.
        with self.assertRaises(gdb.GdbError) as caught:
            _ = watched.listing
        self.assertEqual(caught.exception.code, gdb.ErrorCode.INVALID)

    def test_an_arrival_is_reported_once_and_stop_is_final(self):
        events: list[tuple[int, dict]] = []
        # A board may or may not be attached: with one on the bus the first poll
        # reports it as an arrival, and with none there is nothing to report.
        watched = gdb.watch(lambda kind, event: events.append((kind, event)), interval_ms=100)
        try:
            deadline = time.monotonic() + 5
            while not events and time.monotonic() < deadline:
                time.sleep(0.01)
            if events:
                kind, event = events[0]
                self.assertEqual(kind, gdb.Watch.ARRIVED)
                self.assertIn("serial", event)
            else:
                self.assertIsInstance(watched.listing, dict)
        finally:
            watched.stop()
        seen = len(events)
        time.sleep(0.3)
        self.assertEqual(len(events), seen, "a stopped watch must not call back")
        watched.close()


class ScreenTargetTests(unittest.TestCase):
    """The monitor listing, the containing anchor and the clamp counters: what a
    multi-monitor mistake looks like from the host side. No board needed."""

    SYMBOLS = ("gdb_metrics_clamps", "gdb_monitors_json")

    def test_the_library_exports_the_clamp_and_monitor_symbols(self):
        self.assertEqual(gdb.ABI_VERSION_MINOR, 9)
        for name in self.SYMBOLS:
            self.assertTrue(hasattr(gdb.LIBRARY, name), f"gdb.dll has no {name}")

    def test_the_header_declares_the_clamp_and_monitor_surface(self):
        here = os.path.dirname(os.path.abspath(__file__))
        root = os.path.abspath(os.path.join(here, "..", ".."))
        path = os.path.join(root, "sdk", "ffi", "include", "gdb.h")
        with open(path, encoding="utf-8") as handle:
            header = handle.read()
        self.assertIn("typedef struct gdb_clamps", header)
        self.assertIn("gdb_screen_containing(", header)
        self.assertIn("GDB_INVALID_SCREEN = 11", header)
        self.assertIn("GDB_STALE_WINDOW = 12", header)
        for name in self.SYMBOLS:
            self.assertIn(f"{name}(", header, f"gdb.h does not declare {name}")

    def test_containing_packs_the_point_into_the_argument(self):
        # x in the high 32 bits, y in the low 32, both as uint32 - the packing
        # gdb_screen_containing does in C.
        screen = gdb.Screen.containing(1400, -300)
        self.assertEqual(screen.kind, 3)
        self.assertEqual(screen.arg, (1400 << 32) | (-300 & 0xFFFFFFFF))

        raw = screen._raw()
        self.assertEqual((raw.kind, raw.arg), (3, (1400 << 32) | 0xFFFFFED4))
        self.assertEqual((raw.arg >> 32) & 0xFFFFFFFF, 1400)
        self.assertEqual(raw.arg & 0xFFFFFFFF, 0xFFFFFED4)

        # A point inside the positive quadrant packs the same way, and the anchor
        # is not a monitor index: kind 3 carries the point, not an ordinal.
        self.assertEqual(gdb.Screen.containing(100, 200).arg, (100 << 32) | 200)

    def test_clamps_start_at_zero_and_a_refused_move_does_not_count(self):
        with gdb.Device.open_mock() as device:
            clamps = device.clamps()
            self.assertIsInstance(clamps, gdb.Clamps)
            self.assertEqual(clamps.clamped_total, 0)
            self.assertEqual(
                clamps.as_dict(),
                {"clamped_total": 0, "requested_x": 0, "requested_y": 0,
                 "applied_x": 0, "applied_y": 0},
            )

            # A destination on no screen is refused rather than clamped: the
            # counters only move for a move that happened.
            with self.assertRaises(gdb.GdbError) as caught:
                device.mouse.move_to(5000, 5000, screen=gdb.Screen.containing(5000, 5000))
            self.assertEqual(caught.exception.code, gdb.ErrorCode.UNSUPPORTED)
            self.assertEqual(device.clamps().clamped_total, 0)

            # A move the mock accepts leaves the counters alone too, and the
            # struct still round-trips through the ABI.
            device.mouse.move_to(100, 200, screen=gdb.Screen.containing(100, 200))
            self.assertEqual(device.clamps().clamped_total, 0)

    def test_the_device_remembers_its_last_failure(self):
        with gdb.Device.open_mock() as device:
            self.assertEqual(device.last_error(), (0, ""))
            with self.assertRaises(gdb.GdbError):
                device.mouse.move_to(10, 10, screen=gdb.Screen.monitor(1))
            code, message = device.last_error()
            self.assertEqual(code, gdb.ErrorCode.INVALID_SCREEN)
            self.assertIn("session=", message)
            # A success does not clear the record.
            device.barrier()
            self.assertEqual(device.last_error()[0], gdb.ErrorCode.INVALID_SCREEN)

    def test_a_closed_device_refuses_the_clamp_counters(self):
        device = gdb.Device.open_mock()
        device.close()
        with self.assertRaises(gdb.GdbError) as caught:
            device.clamps()
        self.assertEqual(caught.exception.code, gdb.ErrorCode.INVALID)

    def test_monitors_lists_the_desktop(self):
        listed = gdb.monitors()
        self.assertIsInstance(listed, list)
        self.assertTrue(listed, "the system reports at least one monitor")
        for entry in listed:
            self.assertEqual(
                set(entry), {"left", "top", "right", "bottom", "primary", "dpi"}
            )
            self.assertLess(entry["left"], entry["right"])
            self.assertLess(entry["top"], entry["bottom"])
            self.assertIsInstance(entry["primary"], bool)
            self.assertGreaterEqual(entry["dpi"], 0)
        self.assertGreaterEqual(
            sum(1 for entry in listed if entry["primary"]), 1,
            "exactly the entry Screen.primary resolves to is marked primary",
        )

    def test_the_window_client_anchor_and_the_platform_reads(self):
        state = gdb.prepare_platform()
        self.assertEqual(
            set(state), {"supported", "dpiAwareness", "value", "perMonitor"}
        )
        self.assertIsInstance(state["supported"], bool)
        if sys.platform == "win32":
            self.assertTrue(state["supported"])
            self.assertTrue(state["perMonitor"], state)

        # A handle the system does not know is a typed refusal.
        with self.assertRaises(gdb.GdbError) as caught:
            gdb.window_metrics(0)
        self.assertEqual(
            caught.exception.code,
            gdb.ErrorCode.STALE_WINDOW if sys.platform == "win32"
            else gdb.ErrorCode.UNSUPPORTED,
        )

        # kind 4 is the client area, and a mock has no windows.
        screen = gdb.Screen.window_client(0x1234)
        self.assertEqual((screen.kind, screen.arg), (4, 0x1234))
        with gdb.Device.open_mock() as device:
            with self.assertRaises(gdb.GdbError) as caught:
                device.mouse.move_to(10, 10, screen=screen)
            self.assertEqual(caught.exception.code, gdb.ErrorCode.STALE_WINDOW)
            self.assertIn("client area", caught.exception.message)

    def test_the_anchor_error_codes_are_named(self):
        self.assertEqual(gdb.ErrorCode.INVALID_SCREEN, 11)
        self.assertEqual(gdb.ErrorCode.STALE_WINDOW, 12)
        self.assertEqual(gdb.ErrorCode.NAMES[11], "INVALID_SCREEN")
        self.assertEqual(gdb.ErrorCode.NAMES[12], "STALE_WINDOW")

        with gdb.Device.open_mock() as device:
            # A mock has one screen, so index 1 names a monitor it does not report.
            with self.assertRaises(gdb.GdbError) as caught:
                device.mouse.move_to(10, 10, screen=gdb.Screen.monitor(1))
            self.assertEqual(caught.exception.code, gdb.ErrorCode.INVALID_SCREEN)
            self.assertEqual(caught.exception.name, "INVALID_SCREEN")

            with self.assertRaises(gdb.GdbError) as caught:
                device.mouse.move_to(10, 10, screen=gdb.Screen.window(0x1234))
            self.assertEqual(caught.exception.code, gdb.ErrorCode.STALE_WINDOW)
            self.assertEqual(caught.exception.name, "STALE_WINDOW")


if __name__ == "__main__":
    os.environ.setdefault("GDBOX_LIBRARY", os.path.join("target", "debug", "gdb.dll"))
    unittest.main(verbosity=2)
