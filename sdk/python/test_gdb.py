"""Behaviour tests for the Python binding; no hardware required.

    python sdk/python/test_gdb.py
"""

from __future__ import annotations

import json
import os
import threading
import time
import unittest

import gdb


class DeviceTests(unittest.TestCase):
    def test_abi_and_version(self):
        self.assertEqual(gdb.LIBRARY.gdb_abi_version(), gdb.ABI_VERSION)
        self.assertEqual(gdb.LIBRARY.gdb_version(), b"0.1.0")

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

    def test_invalid_usage_is_a_typed_error(self):
        with gdb.Device.open_mock() as device:
            with self.assertRaises(gdb.GdbError) as caught:
                device.keyboard.key_down(7, 0x01)
            self.assertEqual(caught.exception.code, gdb.ErrorCode.INVALID)

    def test_mouse_surface(self):
        with gdb.Device.open_mock() as device:
            device.mouse.move_by(120, -80, duration_ms=20)
            device.mouse.move_path([(10, 10), (20, 20)], duration_ms=10)
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
        with gdb.Device.open_mock() as device:
            scheduler = device.scheduler()
            order: list[str] = []
            scheduler.pause()
            scheduler.submit(0, 0, lambda: order.append("low") or 0)
            scheduler.submit(0, 10, lambda: order.append("high") or 0)
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

    def test_missing_selector_is_disconnected(self):
        with self.assertRaises(gdb.GdbError) as caught:
            gdb.Device.open("no-such-serial")
        self.assertEqual(caught.exception.code, gdb.ErrorCode.DISCONNECTED)

    def test_closed_device_rejects_calls(self):
        device = gdb.Device.open_mock()
        device.close()
        with self.assertRaises(gdb.GdbError):
            _ = device.info


if __name__ == "__main__":
    os.environ.setdefault("GDBOX_LIBRARY", os.path.join("target", "debug", "gdb.dll"))
    unittest.main(verbosity=2)
