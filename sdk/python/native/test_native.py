"""Behaviour tests for the native GDBOX SDK; no hardware, no gdb.dll.

    python sdk/python/native/test_native.py
    python -m unittest discover -s sdk/python/native -p "test_*.py"
"""

from __future__ import annotations

import json
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
if HERE not in sys.path:
    sys.path.insert(0, HERE)

import gdb  # noqa: E402  (the module under test)

ROOT = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
VECTORS = os.path.join(ROOT, "shared", "input-protocol", "vectors.json")
KEYS = os.path.join(ROOT, "shared", "input-protocol", "keys.json")


def mock_device(**kwargs):
    transport = gdb.MockTransport()
    device = gdb.Device.open_mock(transport=transport, heartbeat=False, **kwargs)
    return transport, device


def status_body(lease_ms=2000, sequence=0, pressed=b""):
    body = bytearray(gdb.STATUS_BYTES)
    body[1:5] = struct.pack("<I", sequence)
    body[5:9] = struct.pack("<I", sequence)
    body[10:12] = struct.pack("<H", lease_ms)
    body[20:51] = pressed or bytes(gdb.SNAPSHOT_BYTES)
    return bytes(body)


def elf_image(segments):
    """Minimal ELF32 with the given ``(paddr, payload)`` loadable segments."""
    phoff = 52
    data_at = phoff + len(segments) * 32
    out = bytearray(data_at)
    out[0:4] = b"\x7fELF"
    out[4] = 1
    out[5] = 1
    out[28:32] = struct.pack("<I", phoff)
    out[42:44] = struct.pack("<H", 32)
    out[44:46] = struct.pack("<H", len(segments))
    cursor = data_at
    for index, (paddr, payload) in enumerate(segments):
        base = phoff + index * 32
        out[base : base + 4] = struct.pack("<I", 1)
        out[base + 4 : base + 8] = struct.pack("<I", cursor)
        out[base + 12 : base + 16] = struct.pack("<I", paddr)
        out[base + 16 : base + 20] = struct.pack("<I", len(payload))
        out.extend(payload)
        cursor += len(payload)
    return bytes(out)


def block_field(uf2, block, index):
    """One u32 of a UF2 block header (or the trailing magic word)."""
    at = block * gdb.flash.uf2.BLOCK + index * 4
    return struct.unpack("<I", uf2[at : at + 4])[0]


def differing_words(left, right):
    """Offsets of the u32 words that differ, or ``None`` if the difference is not that.

    Used to tell "the ELF came from another commit" (the build hash moved, twice)
    apart from "the packer is wrong" (anything else).
    """
    if len(left) != len(right):
        return None
    offsets = [index for index in range(len(left)) if left[index] != right[index]]
    if not offsets:
        return None
    words = sorted({index - index % 4 for index in offsets})
    if len(words) > 2:
        return None
    for word in words:
        if any(index not in offsets for index in range(word, word + 4)):
            return None
    return words


class WireFormatTests(unittest.TestCase):
    def test_golden_vectors_are_reproduced_byte_for_byte(self):
        with open(VECTORS, "r", encoding="utf-8") as handle:
            vectors = json.load(handle)
        transport = gdb.MockTransport()
        session = gdb.Session.open(transport, timeout_ms=200, session_id=0x12345678)
        # OPEN already went out with sequence 1; the vectors then expect exactly
        # these four commands.
        session.key(7, 30, True)
        session.key(7, 89, True)
        session.key(12, 233, True)
        session.barrier()

        self.assertEqual(len(transport.reports), len(vectors))
        for vector, sent in zip(vectors, transport.reports):
            self.assertEqual(list(sent), vector["bytes"], vector["name"])
            self.assertEqual(gdb.decode_packet(sent, gdb.OUT_ID).opcode, vector["opcode"])

    def test_packet_decoding_rejects_malformed_frames(self):
        good = gdb.encode_packet(gdb.KEY, 0x1020304, 7, bytes([7, 0, 30, 0, 1]))
        self.assertEqual(gdb.decode_packet(good, gdb.OUT_ID).payload, bytes([7, 0, 30, 0, 1]))
        for mutate in (
            lambda frame: frame[:3] + bytes([52]) + frame[4:],  # length > 51
            lambda frame: frame[:63] + bytes([1]),  # non-zero padding
            lambda frame: frame[:1] + bytes([2]) + frame[2:],  # wrong version
            lambda frame: frame[:4] + bytes([1]) + frame[5:],  # non-zero flags
        ):
            broken = mutate(bytearray(good))
            with self.assertRaises(gdb.GdbError):
                gdb.decode_packet(broken, gdb.OUT_ID)

    def test_key_catalog_matches_keys_json(self):
        if not os.path.exists(KEYS):
            self.skipTest("keys.json is not in this checkout")
        with open(KEYS, "r", encoding="utf-8") as handle:
            source = json.load(handle)
        expected = {(row["page"], row["usage"]) for row in source["keys"]}
        self.assertEqual(expected, set(gdb.KEY_CATALOG))

    def test_interface_path_is_built_from_the_pnp_instance_id(self):
        instance = r"HID\VID_03F0&PID_0024&MI_01\7&2F3A1B&0&0000"
        self.assertEqual(
            gdb.interface_path(instance),
            r"\\?\hid#vid_03f0&pid_0024&mi_01#7&2f3a1b&0&0000#"
            "{4d1e55b2-f16f-11cf-88cb-001111000030}",
        )
        with self.assertRaises(gdb.GdbError):
            gdb.interface_path("USB\\VID_1234&PID_5678\\1")

    def test_session_list_skips_empty_slots(self):
        self.assertEqual(gdb.parse_session_list(bytes(gdb.SESSION_LIST_BYTES)), [])
        body = bytearray(gdb.SESSION_LIST_BYTES)
        body[6:8] = struct.pack("<H", 0x0201)
        body[8:10] = struct.pack("<H", 900)
        entries = gdb.parse_session_list(bytes(body))
        self.assertEqual([entry.id for entry in entries], [0x0201])

    def test_capabilities_report_what_the_firmware_supports(self):
        transport = gdb.MockTransport()
        capabilities = gdb.Capabilities.parse(transport.feature())
        self.assertEqual(capabilities.board, 2)
        self.assertEqual(capabilities.firmware, "3.2.0")
        self.assertEqual(capabilities.lease_ms, 2000)
        self.assertTrue(capabilities.supports(7, 0x04))
        self.assertTrue(capabilities.supports(12, 0xE9))
        self.assertFalse(capabilities.supports(7, 0x01))
        self.assertFalse(capabilities.supports(12, 0x01))

    def test_input_state_rejects_reserved_bits(self):
        state = gdb.InputState()
        state.set(7, 0x04, True)
        state.set(12, 0xE9, True)
        self.assertTrue(gdb.InputState.decode(state.encode()).contains(7, 0x04))
        reserved = bytearray(state.encode())
        reserved[1] = 1  # usage 0 is not in the catalog
        with self.assertRaises(gdb.GdbError):
            gdb.InputState.decode(bytes(reserved))


class DeviceTests(unittest.TestCase):
    def test_mock_reports_the_firmware_contract(self):
        transport, device = mock_device()
        with device:
            info = device.info
            self.assertEqual(info["board"], 2)
            self.assertEqual(info["firmware"], "3.2.0")
            self.assertEqual(info["leaseMs"], 2000)
            self.assertEqual(info["pollMs"], 1)
            self.assertEqual(info["capabilities"], 15)
            self.assertEqual(device.state, gdb.ConnectionState.CONNECTED)

            status = device.status()
            self.assertEqual(status.lease_ms, 2000)
            self.assertEqual(status["leaseMs"], 2000)  # also answers JSON keys
            self.assertEqual(status.pending, 0)
            self.assertEqual(len(status.state.keys), 28)

            ext = device.status_ext()
            self.assertGreater(ext.uptime_ms, 0)
            self.assertEqual(ext.boot_count, 1)
            self.assertIsNone(ext.last_fault_age_ms)

            sessions = device.sessions()
            self.assertEqual(len(sessions), 1)
            ours = transport.session & 0xFFFF
            self.assertEqual(sessions[0].id, ours, "id is the low 16 bits of ours")

    def test_open_sends_feature_then_open_with_sequence_one(self):
        transport, device = mock_device()
        with device:
            self.assertEqual(len(transport.reports), 1)
            packet = gdb.decode_packet(transport.reports[0], gdb.OUT_ID)
            self.assertEqual(packet.opcode, gdb.OPEN)
            self.assertEqual(packet.sequence, 1)
            self.assertNotEqual(packet.session, 0)
            self.assertEqual(packet.payload, b"")
            self.assertEqual(transport.session, packet.session)

    def test_closed_device_rejects_calls(self):
        _transport, device = mock_device()
        device.close()
        self.assertEqual(device.state, gdb.ConnectionState.DISCONNECTED)
        with self.assertRaises(gdb.GdbError) as caught:
            _ = device.info
        self.assertEqual(caught.exception.code, gdb.ErrorCode.INVALID)
        with self.assertRaises(gdb.GdbError) as caught:
            device.barrier()
        self.assertEqual(caught.exception.code, gdb.ErrorCode.DISCONNECTED)

    def test_close_sends_close_and_clears_held_buttons(self):
        transport, device = mock_device()
        device.mouse.button(gdb.Button.LEFT, True)
        device.close()
        self.assertEqual(transport.writes[-1].opcode, gdb.CLOSE)
        self.assertTrue(transport.is_closed())

    def test_reconnect_opens_a_working_session(self):
        _transport, device = mock_device()
        with device:
            device.reconnect()
            self.assertEqual(device.state, gdb.ConnectionState.CONNECTED)
            self.assertEqual(device.status().lease_ms, 2000)

    def test_sequence_exhaustion_is_reported_before_wrapping(self):
        transport, device = mock_device()
        with device:
            device._session.sequence = 0xFFFFFFFF
            with self.assertRaises(gdb.GdbError) as caught:
                device.barrier()
            self.assertEqual(caught.exception.code, gdb.ErrorCode.INVALID)
            self.assertIn("sequence exhausted", caught.exception.message)
            device._session.sequence = 0

    def test_log_sink_sees_frames_when_trace_is_on(self):
        lines = []
        gdb.set_log_sink(lambda level, message: lines.append(message))
        gdb.set_log_level(gdb.Level.TRACE)
        try:
            _transport, device = mock_device()
            with device:
                device.barrier()
        finally:
            gdb.set_log_level(gdb.Level.OFF)
            gdb.set_log_sink(None)
        self.assertTrue(any("opcode=" in line for line in lines), lines)

    def test_events_fire_on_close(self):
        _transport, device = mock_device()
        seen = []
        device.set_event_handler(lambda kind, code, message: seen.append((kind, code, message)))
        device.close()
        self.assertEqual(seen, [(0, gdb.ConnectionState.DISCONNECTED, "")])


class ReplyMatchingTests(unittest.TestCase):
    def test_foreign_session_frames_are_ignored(self):
        transport, device = mock_device()
        with device:
            ours = transport.session
            transport.push_reply(gdb.STATUS, ours ^ 0x7F7F, 99, status_body(lease_ms=17))
            status = device.status()
            self.assertEqual(status.lease_ms, 2000)

    def test_mismatched_sequence_is_ignored(self):
        transport, device = mock_device()
        with device:
            transport.push_reply(gdb.STATUS, transport.session, 12345, status_body(lease_ms=99))
            self.assertEqual(device.status().lease_ms, 2000)

    def test_event_for_our_session_is_a_typed_error(self):
        transport = gdb.MockTransport()
        device = gdb.Device.open_mock(
            transport=transport, heartbeat=False, auto_reconnect=False
        )
        with device:
            transport.push_reply(gdb.EVENT, transport.session, 1, bytes([gdb.ErrorCode.LEASE]))
            with self.assertRaises(gdb.GdbError) as caught:
                device.barrier()
            self.assertEqual(caught.exception.code, gdb.ErrorCode.LEASE)
            self.assertEqual(caught.exception.name, "LEASE")

    def test_a_lease_event_reconnects_and_retries_when_auto_reconnect_is_on(self):
        transport, device = mock_device()
        with device:
            transport.push_reply(gdb.EVENT, transport.session, 1, bytes([gdb.ErrorCode.LEASE]))
            device.barrier()  # recoverable: reopen a fresh session, then retry
            self.assertEqual(device.state, gdb.ConnectionState.CONNECTED)
            self.assertEqual(device.status().lease_ms, 2000)

    def test_event_for_another_session_is_ignored(self):
        transport, device = mock_device()
        with device:
            transport.push_reply(gdb.EVENT, transport.session + 1, 1, bytes([7]))
            device.barrier()  # no exception: the frame is not ours

    def test_device_error_result_byte_maps_to_a_typed_error(self):
        transport, device = mock_device()
        with device:
            sequence = transport.writes[-1].sequence + 1
            transport.push_reply(gdb.BARRIER, transport.session, sequence, bytes([2]))
            with self.assertRaises(gdb.GdbError) as caught:
                device.barrier()
            self.assertEqual(caught.exception.code, gdb.ErrorCode.BUSY)

    def test_session_list_has_no_result_byte(self):
        transport, device = mock_device()
        with device:
            ours = transport.session
            body = bytearray(gdb.SESSION_LIST_BYTES)
            body[0:2] = struct.pack("<H", 0x1234)  # non-zero first byte: an id
            body[2:4] = struct.pack("<H", 1500)
            body[4] = 3
            body[5] = 2
            next_sequence = transport.writes[-1].sequence + 1
            transport.push_reply(gdb.SESSION_LIST, ours, next_sequence, bytes(body))
            sessions = device.sessions()
            self.assertEqual([entry.id for entry in sessions], [0x1234])
            self.assertEqual(sessions[0].lease_ms, 1500)
            self.assertEqual(sessions[0].held, 3)
            self.assertEqual(sessions[0].flags, 2)


class HeartbeatTests(unittest.TestCase):
    def test_heartbeat_keeps_the_lease_when_the_host_is_idle(self):
        transport = gdb.MockTransport()
        device = gdb.Device.open_mock(transport=transport, heartbeat=True, heartbeat_ms=30)
        ours = transport.session
        try:
            time.sleep(0.2)
        finally:
            device.close()
        beats = transport.packets(gdb.HEARTBEAT)
        self.assertGreaterEqual(len(beats), 2)
        self.assertTrue(all(beat.session == ours for beat in beats))
        self.assertTrue(all(beat.payload == b"" for beat in beats))
        sequences = [beat.sequence for beat in transport.writes]
        self.assertEqual(sequences, sorted(sequences), "sequence numbers must only grow")

    def test_status_polls_do_not_suppress_the_heartbeat(self):
        transport = gdb.MockTransport()
        device = gdb.Device.open_mock(transport=transport, heartbeat=True, heartbeat_ms=40)
        polls = 0
        try:
            deadline = time.monotonic() + 0.3
            while time.monotonic() < deadline:
                self.assertEqual(device.status().lease_ms, 2000)
                polls += 1
                time.sleep(0.002)
        finally:
            device.close()
        self.assertGreaterEqual(polls, 2)
        beats = transport.packets(gdb.HEARTBEAT)
        self.assertGreaterEqual(len(beats), 2)
        # STATUS is the one query that must not renew, so the beat had to happen.
        self.assertGreater(len(transport.packets(gdb.STATUS)), len(beats))


class KeyboardTests(unittest.TestCase):
    def test_key_frames_are_exact(self):
        transport, device = mock_device()
        with device:
            device.keyboard.key_down(7, 0x04)
            packet = transport.writes[-1]
            self.assertEqual(packet.opcode, gdb.KEY)
            self.assertEqual(packet.payload, bytes([7, 0, 4, 0, 1]))
            self.assertEqual(
                transport.reports[-1],
                gdb.encode_packet(gdb.KEY, packet.session, packet.sequence, bytes([7, 0, 4, 0, 1])),
            )

            device.keyboard.key_up(7, 0x04)
            self.assertEqual(transport.writes[-1].payload, bytes([7, 0, 4, 0, 0]))
            self.assertEqual(
                transport.writes[-1].sequence, packet.sequence + 1
            )

    def test_media_key_uses_the_consumer_page(self):
        transport, device = mock_device()
        with device:
            device.keyboard.key_down(12, 0xE9)
            self.assertEqual(transport.writes[-1].payload, bytes([12, 0, 233, 0, 1]))
            self.assertTrue(transport.held.contains(12, 0xE9))

    def test_press_pairs_down_and_up(self):
        transport, device = mock_device()
        with device:
            device.keyboard.press(7, 0x05)
            presses = [packet.payload for packet in transport.packets(gdb.KEY)]
            self.assertEqual(presses, [bytes([7, 0, 5, 0, 1]), bytes([7, 0, 5, 0, 0])])

    def test_type_text_follows_the_us_stroke_table(self):
        self.assertEqual([(s.usage, s.shift) for s in gdb.strokes("aA")], [(0x04, False), (0x04, True)])
        self.assertEqual(gdb.strokes("!")[0].usage, 0x1E)
        self.assertTrue(gdb.strokes("!")[0].shift)
        self.assertEqual(gdb.strokes("0")[0].usage, 0x27)
        self.assertEqual(gdb.strokes("a\r\nb").__len__(), 3)
        self.assertEqual(gdb.strokes("\n")[0].usage, 0x28)
        self.assertEqual(gdb.strokes("中"), [])

        transport, device = mock_device()
        with device:
            device.keyboard.type_text("aA", interval_ms=1, hold_ms=1, jitter_ms=0)
            payloads = [packet.payload for packet in transport.packets(gdb.KEY)]
            # a, A = shift down, A, shift up
            self.assertEqual(
                payloads,
                [
                    bytes([7, 0, 0x04, 0, 1]),
                    bytes([7, 0, 0x04, 0, 0]),
                    bytes([7, 0, 0xE1, 0, 1]),
                    bytes([7, 0, 0x04, 0, 1]),
                    bytes([7, 0, 0x04, 0, 0]),
                    bytes([7, 0, 0xE1, 0, 0]),
                ],
            )

    def test_invalid_usage_is_a_typed_error_and_writes_nothing(self):
        transport, device = mock_device()
        with device:
            before = len(transport.reports)
            with self.assertRaises(gdb.GdbError) as caught:
                device.keyboard.key_down(7, 0x01)
            self.assertEqual(caught.exception.code, gdb.ErrorCode.INVALID)
            self.assertEqual(len(transport.reports), before)

    def test_chord_requires_a_usage(self):
        _transport, device = mock_device()
        with device:
            with self.assertRaises(gdb.GdbError) as caught:
                device.keyboard.chord([], 5)
            self.assertEqual(caught.exception.code, gdb.ErrorCode.INVALID)

            device.keyboard.chord([0xE0, 0x06], 5)
            payloads = [packet.payload for packet in _transport.packets(gdb.KEY)]
            self.assertEqual(payloads[0], bytes([7, 0, 0xE0, 0, 1]))
            self.assertEqual(payloads[1], bytes([7, 0, 0x06, 0, 1]))
            self.assertEqual(payloads[-1], bytes([7, 0, 0xE0, 0, 0]))


class MouseTests(unittest.TestCase):
    def test_wheel_and_button_frames(self):
        transport, device = mock_device()
        with device:
            device.mouse.wheel(120, -120)
            self.assertEqual(transport.writes[-1].opcode, gdb.WHEEL)
            self.assertEqual(transport.writes[-1].payload, struct.pack("<hh", 120, -120))

            device.mouse.button(gdb.Button.LEFT, True)
            self.assertEqual(transport.writes[-1].opcode, gdb.MOUSE_BUTTONS)
            self.assertEqual(transport.writes[-1].payload, bytes([0x01]))
            device.mouse.button(gdb.Button.RIGHT, True)
            self.assertEqual(transport.writes[-1].payload, bytes([0x03]))
            device.mouse.button(gdb.Button.LEFT, False)
            self.assertEqual(transport.writes[-1].payload, bytes([0x02]))

            with self.assertRaises(gdb.GdbError) as caught:
                device.mouse.wheel(4000, 0)
            self.assertEqual(caught.exception.code, gdb.ErrorCode.INVALID)
            with self.assertRaises(gdb.GdbError) as caught:
                device.mouse.button(9, True)
            self.assertEqual(caught.exception.code, gdb.ErrorCode.INVALID)

    def test_move_raw_splits_at_the_hid_limit(self):
        transport, device = mock_device()
        with device:
            device.mouse.move_raw(1500, -1)
            moves = [packet.payload for packet in transport.packets(gdb.MOUSE_MOVE)]
            self.assertEqual(moves, [struct.pack("<hh", 1024, -1), struct.pack("<hh", 476, 0)])

    def test_move_by_barriers_every_chunk(self):
        transport, device = mock_device()
        with device:
            device.mouse.move_by(1500, 0, duration_ms=0)
            moves = transport.packets(gdb.MOUSE_MOVE)
            self.assertEqual(len(moves), 2)
            self.assertEqual(transport.count(gdb.BARRIER), 2)
            # 960 + 1500 runs off the 1920-wide mock screen, so the pointer clamps.
            self.assertEqual(device.cursor, (1919, 540))

    def test_absolute_move_converges_on_the_mock_cursor(self):
        _transport, device = mock_device()
        with device:
            device.mouse.move_to(200, 300, duration_ms=60, profile=gdb.Profile.linear())
            x, y = device.cursor
            self.assertLessEqual(abs(x - 200), 1, f"cursor at {device.cursor}")
            self.assertLessEqual(abs(y - 300), 1, f"cursor at {device.cursor}")

    def test_absolute_move_off_the_primary_monitor_is_unsupported(self):
        _transport, device = mock_device()
        with device:
            device.set_screen_source(lambda: (gdb.Screen(1920, 1080), (-5, 10)))
            with self.assertRaises(gdb.GdbError) as caught:
                device.mouse.move_to(200, 300, duration_ms=60, profile=gdb.Profile.linear())
            self.assertEqual(caught.exception.code, gdb.ErrorCode.UNSUPPORTED)
            self.assertEqual(
                caught.exception.message,
                "absolute coordinates are relative to the primary monitor "
                "(1920x1080 at 0,0) and the cursor is at (-5, 10), "
                "so the move would start off that screen",
            )

    def test_movement_can_be_cancelled(self):
        _transport, device = mock_device()
        with device:
            # Three HID-sized chunks, paced: the cancel lands mid-flight.
            movement = device.mouse.start_move_by(3000, 0, duration_ms=400)
            movement.cancel()
            with self.assertRaises(gdb.GdbError) as caught:
                movement.wait(2000)
            self.assertEqual(caught.exception.code, gdb.ErrorCode.CANCELLED)
            self.assertTrue(movement.finished)

            movement = device.mouse.start_move_by(10, 0, duration_ms=0)
            movement.wait(5000)
            self.assertTrue(movement.finished)


class MotionTests(unittest.TestCase):
    def test_a_straight_profile_hits_the_target_and_stays_on_the_line(self):
        nodes = gdb.trajectory_with(
            (100, 100),
            (500, 300),
            gdb.Screen(1920, 1080),
            400,
            gdb.Profile.linear(),
            gdb.SplitMix64(7),
        )
        last = nodes[-1]
        self.assertEqual((last.x, last.y), (500, 300))
        for node in nodes:
            self.assertLessEqual(abs(node.y - (node.x // 2 + 50)), 1, node)
        at_ms = [node.at_ms for node in nodes]
        self.assertEqual(at_ms, sorted(at_ms))

    def test_the_same_seed_produces_the_same_path(self):
        def build():
            return gdb.trajectory_with(
                (10, 20),
                (900, 500),
                gdb.Screen(),
                300,
                gdb.Profile(),
                gdb.SplitMix64(42),
            )

        self.assertEqual(build(), build())
        self.assertNotEqual(
            build(),
            gdb.trajectory_with(
                (10, 20), (900, 500), gdb.Screen(), 300, gdb.Profile(), gdb.SplitMix64(43)
            ),
        )

    def test_overshoot_and_settle_extend_the_path_without_missing_the_target(self):
        profile = gdb.Profile(overshoot_pct=12.0, settle_ms=40)
        nodes = gdb.trajectory_with(
            (0, 0), (400, 0), gdb.Screen(), 200, profile, gdb.SplitMix64(1)
        )
        self.assertEqual((nodes[-1].x, nodes[-1].y), (400, 0))
        self.assertTrue(any(node.x > 400 or node.x < 0 for node in nodes))

    def test_split_delta_covers_large_moves_in_hid_sized_chunks(self):
        self.assertEqual(gdb.split_delta(0, 0), [])
        self.assertEqual(gdb.split_delta(1025, -1), [(1024, -1), (1, 0)])
        self.assertEqual(gdb.split_delta(-3000, 0), [(-1024, 0), (-1024, 0), (-952, 0)])

    def test_tracker_converges_from_an_unknown_position(self):
        for gain in (0.5, 1.0, 2.0, 4.0):
            tracker = gdb.RelativeTracker()
            position = (-300, 700)
            target = (1600, 200)
            arrived = False
            for step in range(38):
                result = tracker.step(position, target, start_time + gdb.RelativeTracker.TICK_MS * step / 1000.0)
                if result.kind == gdb.STEP_MOVE:
                    self.assertLessEqual(abs(result.dx), 512)
                    position = (
                        position[0] + int(round(result.dx * gain)),
                        position[1] + int(round(result.dy * gain)),
                    )
                elif result.kind == gdb.STEP_ARRIVED:
                    arrived = True
                    break
            self.assertTrue(arrived, f"gain={gain} position={position}")


start_time = 1000.0


class SchedulerTests(unittest.TestCase):
    def setUp(self):
        self._transport, self.device = mock_device()
        self.scheduler = gdb.Scheduler(self.device)

    def tearDown(self):
        self.scheduler.close()
        self.device.close()

    def test_runs_by_due_time_then_priority_then_submission_order(self):
        order = []
        self.scheduler.pause()
        self.scheduler.submit(0, 0, lambda: order.append("low") or 0)
        self.scheduler.submit(0, 10, lambda: order.append("high") or 0)
        self.scheduler.submit(0, 0, lambda: order.append("second") or 0)
        self.scheduler.resume()
        self.scheduler.wait_idle(2000)
        self.assertEqual(order, ["high", "low", "second"])

    def test_due_time_wins_over_priority(self):
        order = []
        self.scheduler.pause()
        self.scheduler.submit(0, 0, lambda: order.append("due-now") or 0)
        self.scheduler.submit(40, 100, lambda: order.append("late-but-urgent") or 0)
        self.scheduler.resume()
        self.scheduler.wait_idle(2000)
        self.assertEqual(order, ["due-now", "late-but-urgent"])

    def test_cancelled_jobs_never_run(self):
        ran = []
        self.scheduler.pause()
        task = self.scheduler.submit(0, 0, lambda: ran.append(1) or 0)
        self.scheduler.cancel(task)
        self.scheduler.resume()
        self.scheduler.wait_idle(2000)
        self.assertEqual(ran, [])

    def test_pause_blocks_execution_until_resume(self):
        ran = []
        self.scheduler.pause()
        self.scheduler.submit(0, 0, lambda: ran.append(1) or 0)
        time.sleep(0.06)
        self.assertEqual(ran, [])
        with self.assertRaises(gdb.GdbError) as caught:
            self.scheduler.wait_idle(50)
        self.assertEqual(caught.exception.code, gdb.ErrorCode.TIMEOUT)
        self.scheduler.resume()
        self.scheduler.wait_idle(2000)
        self.assertEqual(ran, [1])

    def test_repeating_jobs_run_until_cancelled_and_errors_are_reported(self):
        ticks = []
        task = self.scheduler.every(10, 0, lambda: ticks.append(1) or 0)
        time.sleep(0.08)
        self.scheduler.cancel(task)
        self.assertGreaterEqual(len(ticks), 2)
        seen = len(ticks)
        time.sleep(0.05)
        self.assertEqual(len(ticks), seen, "a cancelled repeat must stop")

        self.scheduler.submit(0, 0, lambda: gdb.ErrorCode.INVALID)
        self.scheduler.wait_idle(2000)
        errors = self.scheduler.take_errors()
        self.assertEqual(len(errors), 1)
        self.assertEqual(errors[0]["code"], gdb.ErrorCode.INVALID)
        self.assertIn("taskId", errors[0])
        self.assertEqual(self.scheduler.take_errors(), [])

    def test_zero_interval_is_invalid(self):
        with self.assertRaises(gdb.GdbError) as caught:
            self.scheduler.every(0, 0, lambda: 0)
        self.assertEqual(caught.exception.code, gdb.ErrorCode.INVALID)


class ErrorMappingTests(unittest.TestCase):
    def test_protocol_codes_map_to_stable_sdk_codes(self):
        expected = {
            0: gdb.ErrorCode.OK,
            1: gdb.ErrorCode.FRAMING,
            2: gdb.ErrorCode.BUSY,
            3: gdb.ErrorCode.INVALID,
            5: gdb.ErrorCode.LEASE,
            6: gdb.ErrorCode.DISCONNECTED,
            7: gdb.ErrorCode.DISCONNECTED,
            8: gdb.ErrorCode.OVERFLOW,
            9: gdb.ErrorCode.FRAMING,
            10: gdb.ErrorCode.FRAMING,
            200: gdb.ErrorCode.IO,
        }
        for raw, mapped in expected.items():
            self.assertEqual(gdb.error_from_result(raw).code, mapped, raw)
        self.assertEqual(gdb.error_from_result(200).message, "device error code 200")

    def test_the_eleven_numeric_codes_are_the_abi(self):
        self.assertEqual(len(gdb.ErrorCode.NAMES), 11)
        values = {
            "OK": 0,
            "INVALID": 1,
            "TIMEOUT": 2,
            "DISCONNECTED": 3,
            "BUSY": 4,
            "LEASE": 5,
            "FRAMING": 6,
            "OVERFLOW": 7,
            "UNSUPPORTED": 8,
            "CANCELLED": 9,
            "IO": 10,
        }
        for name, value in values.items():
            self.assertEqual(getattr(gdb.ErrorCode, name), value)
            self.assertEqual(gdb.ErrorCode.NAMES[value], name)

    def test_error_strings_carry_code_and_name(self):
        error = gdb.GdbError(gdb.ErrorCode.BUSY, "device busy")
        self.assertEqual(str(error), "BUSY: device busy")
        self.assertEqual(gdb.GdbError(gdb.ErrorCode.TIMEOUT).name, "TIMEOUT")


class TransportTests(unittest.TestCase):
    def test_missing_feature_report_is_disconnected(self):
        transport = gdb.MockTransport()
        transport.set_feature_missing(True)
        with self.assertRaises(gdb.GdbError) as caught:
            gdb.Device.open_mock(transport=transport, heartbeat=False)
        self.assertEqual(caught.exception.code, gdb.ErrorCode.DISCONNECTED)

    def test_a_transport_may_be_any_duck_typed_object(self):
        class Silent(gdb.Transport):
            def __init__(self):
                self.internal = gdb.MockTransport()
                self.inner = self.internal

            def feature(self):
                return self.inner.feature()

            def write(self, report):
                self.inner.write(report)

            def read(self, timeout_ms):
                return self.inner.read(timeout_ms)

        device = gdb.Device.open_mock(transport=Silent(), heartbeat=False)
        with device:
            self.assertEqual(device.status().lease_ms, 2000)

    def test_disconnect_is_reported_as_disconnected(self):
        transport = gdb.MockTransport()
        module = __import__("gdb")
        device = module.Device.open_mock(transport=transport, heartbeat=False, auto_reconnect=False)
        transport.set_disconnect_after(0)
        with self.assertRaises(gdb.GdbError) as caught:
            device.barrier()
        self.assertEqual(caught.exception.code, gdb.ErrorCode.DISCONNECTED)
        device.close()


class Uf2Tests(unittest.TestCase):
    """RP2040 ELF → UF2 (sdk/rust/src/flash/uf2.rs), byte for byte."""

    def test_blocks_are_aligned_and_ordered_by_address(self):
        # .boot2 last in file order but lowest address, plus an unaligned start.
        image = elf_image([(0x10000100, bytes([1, 2, 3, 4])), (0x10000000, b"\xaa" * 256)])
        packed = gdb.flash.uf2.pack(image)
        self.assertEqual(gdb.flash.uf2.blocks(packed), 2)
        self.assertEqual(block_field(packed, 0, 3), 0x10000000)
        self.assertEqual(block_field(packed, 1, 3), 0x10000100)
        for index in range(2):
            self.assertEqual(block_field(packed, index, 0), 0x0A324655)
            self.assertEqual(block_field(packed, index, 1), 0x9E5D5157)
            self.assertEqual(block_field(packed, index, 2), 0x00002000)
            self.assertEqual(block_field(packed, index, 4), 256)
            self.assertEqual(block_field(packed, index, 5), index)
            self.assertEqual(block_field(packed, index, 6), 2)
            self.assertEqual(block_field(packed, index, 7), 0xE48BFF56)
            end = index * 512 + 508
            self.assertEqual(struct.unpack("<I", packed[end : end + 4])[0], 0x0AB16F30)
        # Block 0 carries boot2, block 1 the unaligned segment plus erased gap.
        self.assertEqual(packed[32:288], b"\xaa" * 256)
        self.assertEqual(packed[512 + 32 : 512 + 36], bytes([1, 2, 3, 4]))
        self.assertEqual(packed[512 + 36 : 512 + 288], b"\xff" * 252)

    def test_an_image_without_flash_segments_is_rejected(self):
        with self.assertRaises(gdb.GdbError) as caught:
            gdb.flash.uf2.pack(elf_image([(0x20000000, bytes([1, 2, 3, 4]))]))
        self.assertEqual(caught.exception.code, gdb.ErrorCode.INVALID)
        for junk in (b"not an elf", bytes(52), b"\x7fELF" + bytes(80)):
            with self.assertRaises(gdb.GdbError):
                gdb.flash.uf2.pack(junk)

    def test_the_packer_reproduces_the_shipped_uf2(self):
        elf = os.path.join(
            ROOT, "firmware", "app", "target", "thumbv6m-none-eabi", "release", "xiao-firmware"
        )
        reference = os.path.join(
            os.environ.get("LOCALAPPDATA", ""), "FirmwareManageTool_Rust", "Firmware.uf2"
        )
        if not os.path.isfile(elf):
            # One build is enough; the point is the bytes, not the compiler.
            cargo = shutil.which("cargo.exe") or shutil.which("cargo")
            if cargo is None:
                self.skipTest("no firmware ELF and no cargo: cannot compare with Firmware.uf2")
            subprocess.run(
                [
                    cargo,
                    "build",
                    "--release",
                    "--target",
                    "thumbv6m-none-eabi",
                    "--manifest-path",
                    os.path.join(ROOT, "firmware", "app", "Cargo.toml"),
                    "--features",
                    "hardware",
                ],
                capture_output=True,
                text=True,
            )
        if not os.path.isfile(elf):
            self.skipTest(f"no firmware ELF at {elf}: cannot compare with Firmware.uf2")
        if not os.path.isfile(reference):
            self.skipTest(f"no reference image at {reference}: nothing to compare against")

        with open(reference, "rb") as handle:
            expected = handle.read()
        self.assertEqual(len(expected), 65536, "the shipped image is 128 blocks")
        with open(elf, "rb") as handle:
            packed = gdb.flash.uf2.pack(handle.read())
        self.assertEqual(gdb.flash.uf2.blocks(packed), 128)

        if packed == expected:
            return
        # The ELF carries the first four bytes of HEAD twice (main.rs BUILD_HASH).
        # A rebuild after a new commit moves those words and nothing else; any
        # other difference is a packer bug, so it must fail, not skip.
        words = differing_words(packed, expected)
        if words is None:
            self.assertEqual(packed, expected, "the packed image is not the shipped one")
            return
        self.skipTest(
            "the ELF was built from a different commit than Firmware.uf2 "
            f"(build hash words at {words}); the other {len(packed) - 4 * len(words)} bytes match"
        )

    def test_the_packer_agrees_with_the_rust_tool(self):
        """The authority is the Rust packer; compare on the real firmware ELF."""
        tool = os.path.join(ROOT, "target", "release", "uf2pack.exe")
        elf = os.path.join(
            ROOT, "firmware", "app", "target", "thumbv6m-none-eabi", "release", "xiao-firmware"
        )
        if not os.path.isfile(elf):
            self.skipTest(f"no firmware ELF at {elf}: nothing to pack")
        if not os.path.isfile(tool):
            self.skipTest(f"no {tool}: run `cargo build --release --bin uf2pack` to compare")
        out = os.path.join(tempfile.mkdtemp(prefix="gdb-uf2-"), "oracle.uf2")
        self.addCleanup(shutil.rmtree, os.path.dirname(out), True)
        result = subprocess.run([tool, elf, out], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        with open(elf, "rb") as handle:
            mine = gdb.flash.uf2.pack(handle.read())
        with open(out, "rb") as handle:
            theirs = handle.read()
        self.assertEqual(len(theirs), 65536, "128 blocks of 512 bytes")
        self.assertEqual(mine, theirs, "the Python packer must match gdb::flash::uf2")


class Token:
    """Cancellation as the flashing sequence needs it: one question, asked often."""

    def __init__(self, cancelled: bool = False):
        self._cancelled = cancelled

    def cancelled(self) -> bool:
        return self._cancelled

    def cancel(self) -> None:
        self._cancelled = True


class FakeRunner:
    """A Runner seam: records the arguments, optionally fails or produces the ELF."""

    def __init__(self, fail_on: str = "", produce=None):
        self.fail_on = fail_on
        self.produce = produce
        self.calls = []

    def run(self, program, args, timeout_s, cancel, log):
        joined = [str(arg) for arg in args]
        self.calls.append(joined)
        if self.produce is not None:
            self.produce(joined)
        if log is not None:
            log(gdb.flash.Stream.STDOUT, "Compiling xiao-firmware")
        if self.fail_on and self.fail_on in " ".join(joined):
            return gdb.flash.ProcessOutcome(exit=9, stderr="injected failure")
        return gdb.flash.ProcessOutcome(exit=0, stdout="ok")


class FlashBuildTests(unittest.TestCase):
    def setUp(self):
        self.root = os.path.join(tempfile.mkdtemp(prefix="gdb-build-"), "firmware")
        self.addCleanup(shutil.rmtree, os.path.dirname(self.root) or self.root, True)
        os.makedirs(os.path.join(self.root, "app"))
        with open(os.path.join(self.root, "app", "Cargo.toml"), "w", encoding="utf-8") as handle:
            handle.write("")
        self.cargo = os.path.join(os.path.dirname(self.root), "cargo.exe")
        with open(self.cargo, "w", encoding="utf-8") as handle:
            handle.write("")
        self.config = gdb.flash.Config(cargo_path=self.cargo, manifest_dir=self.root)
        self.elf = elf_image([(0x10000000, b"\xc0\xde" * 256)])

    def test_the_build_is_pinned_to_one_crate_and_target(self):
        config = gdb.flash.Config()
        self.assertEqual(gdb.flash.Config().target, "thumbv6m-none-eabi")
        self.assertEqual(gdb.flash.APP_CRATE, "xiao-firmware")
        self.assertEqual(gdb.flash.FEATURES, "hardware")
        self.assertEqual(gdb.flash.BOARD_ID, 2)
        self.assertTrue(config.app.endswith(os.path.join("firmware", "app")))
        self.assertTrue(
            config.elf.endswith(
                os.path.join("app", "target", "thumbv6m-none-eabi", "release", "xiao-firmware")
            )
        )
        # Pointing at the crate itself is the same app folder.
        crate = gdb.flash.Config(manifest_dir=os.path.join(ROOT, "firmware", "app"))
        self.assertEqual(os.path.basename(crate.app), "app")

    def test_build_stages_one_to_four_produce_a_packed_image(self):
        def produce(_args):
            os.makedirs(os.path.dirname(self.config.elf), exist_ok=True)
            with open(self.config.elf, "wb") as handle:
                handle.write(self.elf)

        runner = FakeRunner(produce=produce)
        stages, logs = [], []
        image = gdb.flash.build(
            self.config,
            runner=runner,
            progress=lambda stage, message: (
                stages.append((stage, message)) if stage is not None else logs.append(message)
            ),
        )
        self.assertEqual(
            [stage for stage, _ in stages],
            [1, 2, 3, 4],
            "the stage numbers a host shows",
        )
        self.assertEqual(
            [message for _, message in stages], [gdb.flash.STAGES[n] for n in (1, 2, 3, 4)]
        )
        self.assertEqual(image.bytes, gdb.flash.uf2.pack(self.elf))
        self.assertEqual(image.blocks, 2)
        self.assertIn("UF2：2 個區塊", logs)
        self.assertTrue(any("找不到 rustup" in line for line in logs), logs)
        # Only the build itself may carry --features.
        self.assertTrue(any("build" in call for call in runner.calls))
        for call in runner.calls:
            if "--features" in call:
                self.assertIn("build", call)
            if "--target" in call:
                self.assertEqual(call[call.index("--target") + 1], "thumbv6m-none-eabi")

    def test_a_failing_build_is_reported_and_keeps_the_child_output(self):
        error = None
        with self.assertRaises(gdb.GdbError) as caught:
            gdb.flash.build(self.config, runner=FakeRunner(fail_on="--features"))
        error = caught.exception
        self.assertEqual(error.code, gdb.ErrorCode.IO)
        self.assertIn("子程序失敗", error.message)
        self.assertIn("injected failure", error.message)

    def test_a_missing_firmware_folder_is_reported_before_building(self):
        runner = FakeRunner()
        config = gdb.flash.Config(
            cargo_path=self.cargo, manifest_dir=os.path.join(self.root, "nowhere")
        )
        with self.assertRaises(gdb.GdbError) as caught:
            gdb.flash.build(config, runner=runner)
        self.assertEqual(caught.exception.code, gdb.ErrorCode.INVALID)
        self.assertIn("app/Cargo.toml", caught.exception.message)
        self.assertEqual(runner.calls, [], "nothing may run before the manifest is checked")

    def test_a_cargo_path_that_does_not_exist_is_rejected(self):
        config = gdb.flash.Config(
            cargo_path=os.path.join(self.root, "no-cargo.exe"), manifest_dir=self.root
        )
        with self.assertRaises(gdb.GdbError) as caught:
            gdb.flash.build(config, runner=FakeRunner())
        self.assertIn("不存在", caught.exception.message)

    def test_cancellation_never_reports_success(self):
        token = Token(cancelled=True)
        with self.assertRaises(gdb.GdbError) as caught:
            gdb.flash.build(self.config, runner=FakeRunner(), cancel=token)
        self.assertEqual(caught.exception.code, gdb.ErrorCode.CANCELLED)
        self.assertEqual(caught.exception.message, "操作已取消")


class FlashPushTests(unittest.TestCase):
    """The BOOTSEL hand-over, the drive wait, the copy and the hello verify."""

    def setUp(self):
        self.root = tempfile.mkdtemp(prefix="gdb-flash-")
        self.addCleanup(shutil.rmtree, self.root, True)
        self.drive = os.path.join(self.root, "drive")
        os.makedirs(self.drive)
        with open(os.path.join(self.drive, "INFO_UF2.TXT"), "w", encoding="utf-8") as handle:
            handle.write("UF2 Bootloader v3.0\nModel: Raspberry Pi RP2\nBoard-ID: RPI-RP2\n")
        self.image = gdb.flash.Image(bytes=b"\x55" * 65536, blocks=128)
        self.entry = {"path": r"\\?\\hid#vid_03f0&pid_0024&mi_01#7&1&0000#{guid}", "serial": "MOCK"}

    def config(self, **kwargs):
        kwargs.setdefault("boot_target", "")
        return gdb.flash.Config(**kwargs)

    def reenumerated(self, entries):
        """The first enumeration happens before the copy; the board shows up after it."""
        state = {"calls": 0}

        def enumerate_devices():
            state["calls"] += 1
            return [] if state["calls"] == 1 else [dict(entry) for entry in entries]

        return enumerate_devices

    def test_bootloader_goes_out_as_opcode_nine_and_the_session_closes_first(self):
        self.assertEqual(gdb.BOOTLOADER, 0x09, "protocol 3.2.0 numbers BOOTLOADER 0x09")
        transport, device = mock_device()
        stages, logs = [], []
        seen = {"drives": 0, "devices": 0}

        def discover():
            seen["drives"] += 1
            # Nothing mounted before the command; the drive appears afterwards.
            return [] if seen["drives"] == 1 else [self.drive]

        def enumerate_devices():
            seen["devices"] += 1
            return [] if seen["devices"] == 1 else [dict(self.entry)]

        def open_device(entry):
            return (2, "3.2.0", 2000)

        outcome = gdb.flash.push(
            device,
            self.image,
            progress=lambda stage, message: (
                stages.append((stage, message)) if stage is not None else logs.append(message)
            ),
            config=self.config(),
            discover=discover,
            enumerate_devices=enumerate_devices,
            open_device=open_device,
        )

        self.assertEqual(transport.count(gdb.BOOTLOADER), 1)
        sent = transport.packets(gdb.BOOTLOADER)[0]
        self.assertEqual(sent.opcode, 9)
        self.assertEqual(sent.payload, b"")
        # The board refuses BOOTLOADER while a host keeps polling it.
        self.assertTrue(transport.is_closed(), "the runtime session must be closed")
        self.assertEqual(transport.writes[-1].opcode, gdb.CLOSE)
        # Stage numbering continues where build() left off, and 7 is the verify.
        self.assertEqual([stage for stage, _ in stages], [5, 6, 7])
        self.assertEqual(
            [message for _, message in stages],
            [gdb.flash.STAGES[n] for n in (5, 6, 7)],
        )
        self.assertTrue(any("已確認 enter_bootloader" in line for line in logs), logs)
        with open(os.path.join(self.drive, "Firmware.uf2"), "rb") as handle:
            self.assertEqual(handle.read(), self.image.bytes)
        self.assertTrue(outcome.verified)
        self.assertEqual((outcome.board, outcome.firmware), (2, "3.2.0"))
        self.assertEqual(outcome.message, "board=2, fw=3.2.0, lease=2000 ms")
        self.assertIsNone(outcome.error)

    def test_the_drive_wait_times_out_with_nothing_mounted(self):
        stages, logs = [], []
        started = time.monotonic()
        with self.assertRaises(gdb.GdbError) as caught:
            gdb.flash.push(
                None,
                self.image,
                progress=lambda stage, message: (
                    stages.append(stage) if stage is not None else logs.append(message)
                ),
                config=self.config(drive_wait_s=0.05),
                discover=lambda: [],
                enumerate_devices=lambda: self.fail("must not enumerate before the copy"),
            )
        self.assertEqual(caught.exception.code, gdb.ErrorCode.TIMEOUT)
        self.assertIn("未偵測到 RP2040 UF2 磁碟", caught.exception.message)
        self.assertLess(time.monotonic() - started, 5.0)
        self.assertEqual(stages, [5])
        self.assertTrue(any("未選取 runtime HID" in line for line in logs), logs)

    def test_an_explicit_drive_is_copied_without_a_bootloader_command(self):
        calls = []

        def open_device(entry):
            calls.append(entry)
            return (2, "3.2.0", 2000)

        outcome = gdb.flash.push(
            None,
            self.image,
            config=self.config(boot_target=self.drive),
            enumerate_devices=self.reenumerated([self.entry]),
            open_device=open_device,
            discover=lambda: self.fail("an explicit drive must not be scanned"),
        )
        self.assertEqual(len(calls), 1)
        self.assertTrue(outcome.verified)
        with open(os.path.join(self.drive, "Firmware.uf2"), "rb") as handle:
            self.assertEqual(handle.read(), self.image.bytes)

    def test_a_plain_directory_is_not_a_uf2_drive(self):
        bare = os.path.join(self.root, "bare")
        os.makedirs(bare)
        with self.assertRaises(gdb.GdbError) as caught:
            gdb.flash.push(
                None,
                self.image,
                config=self.config(boot_target=bare),
                enumerate_devices=lambda: [],
                open_device=lambda entry: (2, "3.2.0", 2000),
            )
        self.assertEqual(caught.exception.code, gdb.ErrorCode.INVALID)
        self.assertIn("不是 UF2 bootloader 磁碟", caught.exception.message)
        self.assertFalse(os.path.exists(os.path.join(bare, "Firmware.uf2")))

    def test_the_handshake_retries_and_rejects_a_foreign_board(self):
        attempts = {"n": 0}

        def open_device(_entry):
            attempts["n"] += 1
            if attempts["n"] == 1:
                raise gdb.GdbError(gdb.ErrorCode.TIMEOUT, "delayed handshake")
            return (2, "3.2.0", 2000)

        outcome = gdb.flash.push(
            None,
            self.image,
            config=self.config(boot_target=self.drive, handshake_wait_s=2.0),
            enumerate_devices=self.reenumerated([self.entry]),
            open_device=open_device,
        )
        self.assertTrue(outcome.verified)
        self.assertEqual(attempts["n"], 2, "the first handshake failed, the second answered")

        def wrong_board(_entry):
            raise gdb.GdbError(gdb.ErrorCode.INVALID, "重新列舉的板型不符：1（預期 2）")

        outcome = gdb.flash.push(
            None,
            self.image,
            config=self.config(boot_target=self.drive, handshake_wait_s=0.05),
            enumerate_devices=self.reenumerated([self.entry]),
            open_device=wrong_board,
        )
        self.assertFalse(outcome.verified)
        self.assertFalse(outcome.ok)
        self.assertEqual(outcome.error.code, gdb.ErrorCode.TIMEOUT)
        self.assertIn("重新列舉的板型不符：1（預期 2）", outcome.message)

    def test_an_ambiguous_reenumeration_is_never_verified(self):
        entries = [dict(self.entry), dict(self.entry, path="other")]
        outcome = gdb.flash.push(
            None,
            self.image,
            config=self.config(boot_target=self.drive, handshake_wait_s=0.05),
            enumerate_devices=self.reenumerated(entries),
            open_device=lambda _entry: self.fail("must not pick the first device"),
        )
        self.assertFalse(outcome.verified)
        self.assertIn("多個符合裝置", outcome.message)

    def test_a_board_that_never_comes_back_is_unverified_not_failed(self):
        outcome = gdb.flash.push(
            None,
            self.image,
            config=self.config(boot_target=self.drive, handshake_wait_s=0.05),
            enumerate_devices=lambda: [],
            open_device=lambda _entry: self.fail("nothing to open"),
        )
        self.assertFalse(outcome.verified)
        self.assertEqual(outcome.error.code, gdb.ErrorCode.TIMEOUT)
        self.assertIn("runtime hello 驗證逾時", outcome.message)
        self.assertIn("尚未列舉", outcome.message)
        # The image is on the drive even though the board did not answer.
        with open(os.path.join(self.drive, "Firmware.uf2"), "rb") as handle:
            self.assertEqual(len(handle.read()), len(self.image.bytes))

    def test_two_new_drives_are_refused_rather_than_guessed(self):
        other = os.path.join(self.root, "other")
        os.makedirs(other)
        with self.assertRaises(gdb.GdbError) as caught:
            gdb.flash.push(
                None,
                self.image,
                config=self.config(drive_wait_s=2.0),
                discover=lambda: [self.drive, other],
                enumerate_devices=lambda: [],
            )
        self.assertEqual(caught.exception.code, gdb.ErrorCode.INVALID)
        self.assertIn("同時偵測到多個 UF2 磁碟", caught.exception.message)

    def test_an_unconfirmed_bootloader_command_still_continues(self):
        transport, device = mock_device(timeout_ms=100)
        transport.set_silent(True)  # the board never acknowledges BOOTLOADER
        logs = []
        seen = {"drives": 0}

        def discover():
            seen["drives"] += 1
            return [] if seen["drives"] == 1 else [self.drive]

        outcome = gdb.flash.push(
            device,
            self.image,
            progress=lambda stage, message: None if stage is not None else logs.append(message),
            config=self.config(),
            discover=discover,
            enumerate_devices=self.reenumerated([self.entry]),
            open_device=lambda _entry: (2, "3.2.0", 2000),
        )
        self.assertEqual(transport.count(gdb.BOOTLOADER), 1)
        self.assertTrue(outcome.verified)
        self.assertTrue(
            any("bootloader 命令未收到完整確認" in line for line in logs), logs
        )

    def test_an_empty_image_is_rejected(self):
        with self.assertRaises(gdb.GdbError) as caught:
            gdb.flash.push(None, gdb.flash.Image(), config=self.config(boot_target=self.drive))
        self.assertEqual(caught.exception.code, gdb.ErrorCode.INVALID)

    def test_a_cancelled_copy_leaves_no_finished_file(self):
        token = Token()
        missing = os.path.join(self.root, "missing", "output.uf2")
        with self.assertRaises(gdb.GdbError) as caught:
            gdb.flash.write_image(missing, b"\x01\x02\x03\x04", token)
        self.assertEqual(caught.exception.code, gdb.ErrorCode.IO)
        self.assertIn("UF2 目的檔", caught.exception.message)

        target = os.path.join(self.root, "out.uf2")
        gdb.flash.write_image(target, b"\x01\x02\x03\x04", token)
        self.assertTrue(os.path.isfile(target), "a successful write goes through")

        token.cancel()
        other = os.path.join(self.root, "cancelled.uf2")
        with self.assertRaises(gdb.GdbError) as caught:
            gdb.flash.write_image(other, b"\x01\x02\x03\x04", token)
        self.assertEqual(caught.exception.code, gdb.ErrorCode.CANCELLED)
        self.assertFalse(os.path.exists(other), "a cancelled copy must not look finished")

        with self.assertRaises(gdb.GdbError) as caught:
            gdb.flash.push(
                None, self.image, cancel=token, config=self.config(boot_target=self.drive)
            )
        self.assertEqual(caught.exception.code, gdb.ErrorCode.CANCELLED)


class LoggingTests(unittest.TestCase):
    def setUp(self):
        self.lines = []
        gdb.set_log_sink(lambda level, message: self.lines.append((level, message)))
        gdb.set_log_level(gdb.Level.TRACE)

    def tearDown(self):
        gdb.set_log_level(gdb.Level.OFF)
        gdb.set_log_sink(None)

    def messages(self, prefix=""):
        return [message for _level, message in self.lines if message.startswith(prefix)]

    def test_a_send_and_a_reply_are_both_traced(self):
        transport, device = mock_device()
        session = transport.session
        with device:
            device.keyboard.key_down(7, 0x04)
            device.barrier()
        outs = self.messages("out opcode=")
        ins = self.messages("in opcode=")
        self.assertTrue(outs, self.lines)
        self.assertTrue(ins, self.lines)
        self.assertIn(f"out opcode={gdb.KEY} session={session}", outs[1])
        self.assertIn("payload=[7, 0, 4, 0, 1]", outs[1])
        self.assertIn(f"out opcode={gdb.BARRIER} session={session}", outs[2])
        self.assertTrue(outs[2].endswith("payload=[]"), outs[2])
        self.assertIn(f"in opcode={gdb.OPEN | 0x80} session={session}", ins[0])
        replies = [line for line in ins if line.startswith(f"in opcode={gdb.BARRIER | 0x80} ")]
        self.assertEqual(len(replies), 1, ins)
        self.assertIn(f"session={session}", replies[0])
        self.assertTrue(replies[0].endswith("payload=[0]"), replies[0])
        self.assertTrue(all(level == gdb.Level.TRACE for level, _ in self.lines if " opcode=" in _))

    def test_a_foreign_frame_is_reported_as_skipped(self):
        transport, device = mock_device()
        ours = transport.session
        with device:
            transport.push_reply(gdb.STATUS, ours ^ 0x7F7F, 99, status_body())
            device.status()
        skips = self.messages("skip opcode=")
        self.assertTrue(skips, self.lines)
        self.assertIn("(not ours)", skips[0])
        self.assertIn("sequence=99", skips[0])

    def test_status_polling_is_quiet_below_trace(self):
        gdb.set_log_level(gdb.Level.INFO)
        _transport, device = mock_device()
        with device:
            for _ in range(3):
                self.assertEqual(device.status().lease_ms, 2000)
        frames = [message for _level, message in self.lines if "opcode=" in message]
        self.assertEqual(frames, [], "STATUS polling must not spam the log")
        # Info is still delivered: connect and close are what a host logs.
        self.assertTrue(any(m.startswith("connected serial=") for _, m in self.lines), self.lines)
        self.assertTrue(any(m.startswith("closed serial=") for _, m in self.lines), self.lines)

    def test_the_level_is_validated_and_reported(self):
        self.assertEqual(gdb.log_level(), gdb.Level.TRACE)
        for level in range(gdb.Level.OFF, gdb.Level.TRACE + 1):
            gdb.set_log_level(level)
            self.assertEqual(gdb.log_level(), level)
        with self.assertRaises(gdb.GdbError) as caught:
            gdb.set_log_level(6)
        self.assertEqual(caught.exception.code, gdb.ErrorCode.INVALID)
        with self.assertRaises(gdb.GdbError):
            gdb.set_log_level(-1)
        self.assertEqual(gdb.log_level(), gdb.Level.TRACE)

    def test_off_delivers_nothing(self):
        gdb.set_log_level(gdb.Level.OFF)
        _transport, device = mock_device()
        with device:
            device.barrier()
        self.assertEqual(self.lines, [])


class EventKindTests(unittest.TestCase):
    def test_the_numbers_are_the_c_abi(self):
        self.assertEqual(gdb.EventKind.STATE, 0)
        self.assertEqual(gdb.EventKind.STATUS, 1)
        self.assertEqual(gdb.EventKind.ERROR, 2)

    def test_state_and_error_events_carry_their_payloads(self):
        _transport, device = mock_device()
        seen = []
        device.set_event_handler(lambda kind, code, message: seen.append((kind, code, message)))
        device.close()
        self.assertEqual(seen, [(gdb.EventKind.STATE, gdb.ConnectionState.DISCONNECTED, "")])

        transport, device = mock_device(auto_reconnect=False)
        seen = []
        device.set_event_handler(lambda kind, code, message: seen.append((kind, code, message)))
        transport.set_disconnect_after(0)
        with self.assertRaises(gdb.GdbError):
            device.barrier()
        self.assertEqual(
            seen,
            [
                (
                    gdb.EventKind.ERROR,
                    gdb.ErrorCode.DISCONNECTED,
                    "mock transport disconnected",
                )
            ],
        )
        device.close()


if __name__ == "__main__":
    unittest.main(verbosity=2)
