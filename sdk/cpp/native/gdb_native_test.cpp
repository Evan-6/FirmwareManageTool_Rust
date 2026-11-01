// Headless tests for gdb_native.hpp: everything runs over FakeTransport.
//
//   g++ -std=c++17 -Wall -Wextra -I sdk/cpp/native sdk/cpp/native/gdb_native_test.cpp -o target/native-check/gdb_native_test.exe
//   ./target/native-check/gdb_native_test.exe
//
// Run it from the repository root: the UF2 test packs the firmware ELF when
// firmware/app/target/... and %LOCALAPPDATA%/FirmwareManageTool_Rust/Firmware.uf2
// both exist and prints a skip line otherwise.
//
// The binary links libstdc++ dynamically, so the toolchain's own bin directory
// has to come first on PATH — MSYS-shipped and git-shipped libstdc++ DLLs are
// older than this g++ and take the process down with STATUS_ENTRYPOINT_NOT_FOUND
// (bash reports exit 127) or a crash inside iostream.

#include <array>
#include <atomic>
#include <chrono>
#include <cstdio>
#include <cstdint>
#include <cstdlib>
#include <fstream>
#include <mutex>
#include <string>
#include <string_view>
#include <thread>
#include <vector>

#include "gdb_native.hpp"

namespace {

using namespace gdb::native;

int failures = 0;

void check(bool condition, const std::string& what) {
    if (!condition) {
        std::printf("FAIL  %s\n", what.c_str());
        ++failures;
    }
}

template <typename Fn>
void check_throws(Fn&& fn, ErrorCode expected, const std::string& what) {
    try {
        fn();
    } catch (const Error& error) {
        check(error.code() == expected, what + " (got code " + std::to_string(static_cast<int>(error.code())) + ")");
        return;
    } catch (const std::exception& error) {
        check(false, what + " (unexpected exception: " + error.what() + ")");
        return;
    }
    check(false, what + " (no error)");
}

using Clock = std::chrono::steady_clock;

// The canonical vectors from shared/input-protocol/vectors.json, session
// 0x12345678. Only the fixed header and payload bytes are non-zero.
std::array<std::uint8_t, REPORT_SIZE> vector_frame(std::uint8_t opcode, std::uint32_t sequence,
                                                    std::uint8_t length,
                                                    const std::vector<std::uint8_t>& payload) {
    std::array<std::uint8_t, REPORT_SIZE> bytes{};
    bytes[0] = 12;
    bytes[1] = 3;
    bytes[2] = opcode;
    bytes[3] = length;
    bytes[5] = 0x78;
    bytes[6] = 0x56;
    bytes[7] = 0x34;
    bytes[8] = 0x12;
    bytes[9] = static_cast<std::uint8_t>(sequence & 0xFF);
    bytes[10] = static_cast<std::uint8_t>((sequence >> 8) & 0xFF);
    bytes[11] = static_cast<std::uint8_t>((sequence >> 16) & 0xFF);
    bytes[12] = static_cast<std::uint8_t>((sequence >> 24) & 0xFF);
    for (std::size_t index = 0; index < payload.size(); ++index) {
        bytes[13 + index] = payload[index];
    }
    return bytes;
}

void test_wire_vectors() {
    // open, sequence 1.
    Packet open;
    open.opcode = OPEN;
    open.session = 0x12345678u;
    open.sequence = 1;
    check(open.encode(OUT_ID) == vector_frame(OPEN, 1, 0, {}), "OPEN frame is byte-identical to vectors.json");

    // digit1_down, sequence 2: page 7, usage 30, down.
    Packet digit1;
    digit1.opcode = KEY;
    digit1.session = 0x12345678u;
    digit1.sequence = 2;
    digit1.payload = {7, 0, 30, 0, 1};
    check(digit1.encode(OUT_ID) == vector_frame(KEY, 2, 5, {7, 0, 30, 0, 1}),
          "digit1 KEY frame is byte-identical to vectors.json");

    // volume_up, sequence 4: page 12, usage 233 (0xE9), down.
    Packet volume;
    volume.opcode = KEY;
    volume.session = 0x12345678u;
    volume.sequence = 4;
    volume.payload = {12, 0, 233, 0, 1};
    check(volume.encode(OUT_ID) == vector_frame(KEY, 4, 5, {12, 0, 233, 0, 1}),
          "volume_up KEY frame is byte-identical to vectors.json");

    // barrier, sequence 5.
    Packet barrier;
    barrier.opcode = BARRIER;
    barrier.session = 0x12345678u;
    barrier.sequence = 5;
    check(barrier.encode(OUT_ID) == vector_frame(BARRIER, 5, 0, {}),
          "BARRIER frame is byte-identical to vectors.json");

    // Decode rejects a non-zero pad byte and a long payload, like v3.rs.
    auto padded = open.encode(OUT_ID);
    padded[63] = 1;
    check_throws([&] { Packet::decode(padded, OUT_ID); }, ErrorCode::Invalid, "padded report is rejected");
    auto too_long = open.encode(OUT_ID);
    too_long[3] = 52;
    check_throws([&] { Packet::decode(too_long, OUT_ID); }, ErrorCode::Invalid, "52-byte payload is rejected");

    // Round trip.
    const Packet back = Packet::decode(digit1.encode(OUT_ID), OUT_ID);
    check(back.opcode == KEY && back.session == 0x12345678u && back.sequence == 2 &&
              back.payload == digit1.payload,
          "packet round trips");
}

void test_feature_and_info() {
    auto fake = std::make_shared<FakeTransport>();
    Device device = Device::open_fake(fake, 1000, false, false);
    const DeviceInfo& info = device.info();
    check(info.firmware == "3.2.0", "firmware is 3.2.0");
    check(info.board == 2, "board is the XIAO");
    check(info.lease_ms == 2000, "lease is 2000 ms");
    check(info.poll_ms == 1, "poll interval is 1 ms");
    check(info.capabilities == 15, "all four capabilities are advertised");
    check(info.serial == "MOCK", "fake serial");
    check(device.state() == ConnectionState::Connected, "device is connected");
    check(device.session_id() != 0, "session id is non-zero");

    const std::string json = device.info_json();
    check(json.find("\"firmware\":\"3.2.0\"") != std::string::npos, "info_json names firmware");
    check(json.find("\"leaseMs\":2000") != std::string::npos, "info_json names leaseMs");
    check(json.find("\"deviceId\":[66,80,51") != std::string::npos, "info_json emits the device id");
    device.close();
}

void test_typed_frames_from_device() {
    auto fake = std::make_shared<FakeTransport>();
    Device device = Device::open_fake(fake, 1000, false, false);
    const std::uint32_t session = device.session_id();

    device.keyboard().key_down(7, 0x04);  // "a"
    const auto writes = fake->raw_writes();
    check(writes.size() == 2, "OPEN and KEY were written");
    // OPEN is sequence 1, the key is sequence 2.
    std::array<std::uint8_t, REPORT_SIZE> expected{};
    expected[0] = OUT_ID;
    expected[1] = WIRE_VERSION;
    expected[2] = KEY;
    expected[3] = 5;
    expected[5] = static_cast<std::uint8_t>(session & 0xFF);
    expected[6] = static_cast<std::uint8_t>((session >> 8) & 0xFF);
    expected[7] = static_cast<std::uint8_t>((session >> 16) & 0xFF);
    expected[8] = static_cast<std::uint8_t>((session >> 24) & 0xFF);
    expected[9] = 2;
    expected[13] = 7;
    expected[15] = 0x04;
    expected[17] = 1;
    check(writes[1] == expected, "key frame is exactly the v3 layout");
    check(fake->held().contains(7, 0x04), "the fake holds the key");

    device.mouse().move_raw(5, -3);
    const auto move = fake->raw_writes().back();
    std::array<std::uint8_t, REPORT_SIZE> want_move{};
    want_move[0] = OUT_ID;
    want_move[1] = WIRE_VERSION;
    want_move[2] = MOUSE_MOVE;
    want_move[3] = 4;
    want_move[5] = static_cast<std::uint8_t>(session & 0xFF);
    want_move[6] = static_cast<std::uint8_t>((session >> 8) & 0xFF);
    want_move[7] = static_cast<std::uint8_t>((session >> 16) & 0xFF);
    want_move[8] = static_cast<std::uint8_t>((session >> 24) & 0xFF);
    want_move[9] = 3;
    want_move[13] = 5;
    want_move[15] = 0xFD;  // -3
    want_move[16] = 0xFF;
    check(move == want_move, "mouse move frame is exactly the v3 layout");

    device.close();
}

void test_reply_matching() {
    auto fake = std::make_shared<FakeTransport>();
    Device device = Device::open_fake(fake, 500, false, false);
    const std::uint32_t session = device.session_id();

    // A reply for another owner, then one with the wrong sequence: both must be
    // skipped, and the real reply must still be matched.
    fake->inject_reply(BARRIER, session ^ 0xAA55u, 2, {0});
    fake->inject_reply(BARRIER, session, 999, {0});
    device.barrier();  // sequence 2
    check(fake->count_opcode(BARRIER) == 1, "BARRIER was sent once");

    // A foreign frame must not be mistaken for the STATUS reply.
    fake->inject_reply(STATUS, session + 1, 3, std::vector<std::uint8_t>(STATUS_BYTES, 0));
    const Status status = device.status();  // sequence 3
    check(status.lease_ms == 2000, "status came from the right session");
    check(status.received == 3, "status received counter follows the sequence");

    // A reply to a command that was never sent is ignored, not fatal.
    fake->inject_reply(STATUS_EXT, session, 77, std::vector<std::uint8_t>(STATUS_EXT_BYTES, 0));
    check(device.status_ext().sessions == 1, "stale reply is skipped");

    device.close();
}

void test_event_maps_to_error() {
    auto fake = std::make_shared<FakeTransport>();
    Device device = Device::open_fake(fake, 500, false, false);
    const std::uint32_t session = device.session_id();

    fake->inject_event(session, 5);  // lease expired
    check_throws([&] { device.status(); }, ErrorCode::Lease, "EVENT 5 maps to Lease");

    fake->inject_event(session, 8);  // overflow
    check_throws([&] { device.status(); }, ErrorCode::Overflow, "EVENT 8 maps to Overflow");

    // A device-side result byte in a control reply maps the same way.
    fake->inject_reply(BARRIER, session, 4, {2});
    check_throws([&] { device.barrier(); }, ErrorCode::Busy, "result 2 maps to Busy");

    // An EVENT for somebody else is ignored entirely.
    fake->inject_event(session ^ 0x1234u, 5);
    check(device.status().lease_ms == 2000, "foreign EVENT is ignored");

    device.close();
}

void test_heartbeat_on_idle() {
    auto fake = std::make_shared<FakeTransport>();
    Device device = Device::open_fake(fake, 500, false, true);
    check(fake->count_opcode(HEARTBEAT) == 0, "no heartbeat before the interval");
    std::this_thread::sleep_for(std::chrono::milliseconds(800));
    const int beats = fake->count_opcode(HEARTBEAT);
    check(beats >= 1, "the idle session heartbeats (saw " + std::to_string(beats) + ")");
    check(fake->count_opcode(OPEN) == 1, "OPEN was sent once");
    device.close();
    check(fake->closed(), "CLOSE released the slot");
}

void test_status_polling_keeps_heartbeat() {
    auto fake = std::make_shared<FakeTransport>();
    Device device = Device::open_fake(fake, 500, false, true);
    const auto start = Clock::now();
    int polls = 0;
    while (Clock::now() - start < std::chrono::milliseconds(900)) {
        device.status();
        ++polls;
    }
    const int beats = fake->count_opcode(HEARTBEAT);
    check(polls > 10, "STATUS was polled repeatedly (" + std::to_string(polls) + ")");
    check(beats >= 1, "STATUS polling does not suppress the heartbeat (saw " +
                          std::to_string(beats) + " beats)");
    // The reverse: the heartbeat must not have renewed the lease on our behalf
    // of a barrier-only loop; just assert BARRIER is absent here.
    check(fake->count_opcode(BARRIER) == 0, "pure STATUS polling sends no BARRIER");
    device.close();
}

void test_no_heartbeat_when_disabled() {
    auto fake = std::make_shared<FakeTransport>();
    Device device = Device::open_fake(fake, 500, false, false);
    std::this_thread::sleep_for(std::chrono::milliseconds(650));
    check(fake->count_opcode(HEARTBEAT) == 0, "heartbeat=false sends nothing");
    device.close();
}

void test_status_and_sessions() {
    auto fake = std::make_shared<FakeTransport>();
    Device device = Device::open_fake(fake, 500, false, false);
    const std::uint32_t session = device.session_id();

    device.keyboard().key_down(7, 0xE1);  // left shift
    device.keyboard().key_down(7, 0x06);  // c
    device.mouse().button(Button::Left, true);

    const Status status = device.status();
    check(status.state.contains(7, 0xE1), "STATUS snapshot holds the modifier");
    check(status.state.contains(7, 0x06), "STATUS snapshot holds the key");
    check(status.state.buttons == 0x01, "STATUS snapshot holds the button");

    const std::string status_json = device.status_json();
    for (const char* field : {"\"received\":", "\"completed\":", "\"pending\":", "\"leaseMs\":",
                              "\"rx\":", "\"tx\":", "\"hid\":", "\"failsafe\":", "\"state\":{",
                              "\"modifiers\":", "\"keys\":[", "\"consumer\":", "\"buttons\":"}) {
        check(status_json.find(field) != std::string::npos, std::string("status_json has ") + field);
    }

    const auto sessions = device.sessions();
    check(sessions.size() == 1, "one owner is listed");
    check(sessions[0].id == static_cast<std::uint16_t>(session & 0xFFFF), "session id matches");
    check(sessions[0].lease_ms == 2000, "session lease is reported");
    const std::string sessions_json = device.sessions_json();
    check(sessions_json.find("\"leaseMs\":2000") != std::string::npos, "sessions_json names leaseMs");
    check(sessions_json.find("\"held\":") != std::string::npos, "sessions_json names held");

    const StatusExt ext = device.status_ext();
    check(ext.has_fault == false, "no fault yet");
    check(ext.sessions == 1, "STATUS_EXT counts the owner");
    const std::string ext_json = device.status_ext_json();
    check(ext_json.find("\"uptimeMs\":") != std::string::npos, "status_ext_json names uptimeMs");
    check(ext_json.find("\"lastFaultAgeMs\":null") != std::string::npos,
          "status_ext_json reports no fault age as null");

    device.close();
}

void test_close_and_bootloader() {
    auto fake = std::make_shared<FakeTransport>();
    Device device = Device::open_fake(fake, 500, false, false);
    device.bootloader();
    check(fake->bootloader_requested(), "BOOTLOADER reached the device");
    device.close();
    check(fake->closed(), "the fake saw CLOSE");
    check(fake->sessions().empty(), "the slot was released");
    check(fake->count_opcode(CLOSE) == 1, "CLOSE was sent once");
    check_throws([&] { device.status(); }, ErrorCode::Disconnected, "a closed device rejects commands");
}

void test_release_all_and_buttons() {
    auto fake = std::make_shared<FakeTransport>();
    Device device = Device::open_fake(fake, 500, false, false);
    device.keyboard().key_down(7, 0x04);
    device.mouse().button(Button::Right, true);
    check(device.buttons() == 0x02, "the SDK tracks the button mask");
    device.release_all();
    check(device.buttons() == 0, "release_all clears the local mask");
    check(fake->held().keys[0] == 0 && fake->held().buttons == 0, "release_all clears the device");
    device.close();
}

void test_keyboard_table() {
    auto strokes_a = strokes("a");
    check(strokes_a.size() == 1 && strokes_a[0].usage == 0x04 && !strokes_a[0].shift, "a -> 04");
    auto strokes_shift_a = strokes("A");
    check(strokes_shift_a.size() == 1 && strokes_shift_a[0].usage == 0x04 && strokes_shift_a[0].shift,
          "A -> shift + 04");
    auto bang = strokes("!");
    check(bang.size() == 1 && bang[0].usage == 0x1E && bang[0].shift, "! -> shift + 1E");
    check(strokes("0")[0].usage == 0x27, "0 -> 27");
    check(strokes("a\r\nb").size() == 3, "CRLF drops the CR");
    check(strokes("\n")[0].usage == 0x28, "newline -> 28");
    check(strokes("\xE4\xB8\xAD").empty(), "non-US characters are dropped");
    check(strokes(",./;'[]\\`-= ").size() == 12, "punctuation maps one to one");

    auto fake = std::make_shared<FakeTransport>();
    Device device = Device::open_fake(fake, 500, false, false);
    device.keyboard().key_down(12, 0xE9);  // volume up, page 12
    check(fake->held().consumer == 0x02, "consumer bit 1 is volume up");
    check_throws([&] { device.keyboard().key_down(7, 0x74); }, ErrorCode::Invalid,
                 "an uncatalogued usage is rejected");
    check_throws([&] { device.keyboard().key_down(7, 0x32); }, ErrorCode::Invalid,
                 "0x32 is not in keys.json");
    device.close();
}

void test_motion_maths() {
    check(split_delta(0, 0).empty(), "split_delta(0,0) is empty");
    const auto parts = split_delta(1025, -1);
    check(parts.size() == 2, "1025 splits into two chunks");
    check(parts[0].first == 1024 && parts[0].second == -1, "first chunk is 1024,-1");
    check(parts[1].first == 1 && parts[1].second == 0, "second chunk is 1,0");
    const auto big = split_delta(-2500, 0);
    check(big.size() == 3 && big[0].first == -1024 && big[1].first == -1024 && big[2].first == -452,
          "large negative moves split at the HID limit");

    SplitMix64 rng(7);
    const auto nodes = trajectory_with({100, 100}, {500, 300}, Screen{}, 400, HumanProfile::linear(), rng);
    check(!nodes.empty(), "the trajectory has nodes");
    check(nodes.back().x == 500 && nodes.back().y == 300, "the trajectory ends on the target");
    bool on_line = true;
    bool monotonic = true;
    for (std::size_t index = 0; index < nodes.size(); ++index) {
        on_line = on_line && std::abs(nodes[index].y - (nodes[index].x / 2 + 50)) <= 1;
        if (index != 0) {
            monotonic = monotonic && nodes[index].at_ms >= nodes[index - 1].at_ms;
        }
    }
    check(on_line, "a linear profile stays on the line");
    check(monotonic, "at_ms never goes backwards");

    // Determinism: the same seed gives the same path.
    SplitMix64 first_rng(42);
    SplitMix64 second_rng(42);
    const auto first = trajectory_with({10, 20}, {900, 500}, Screen{}, 300, HumanProfile(), first_rng);
    const auto second = trajectory_with({10, 20}, {900, 500}, Screen{}, 300, HumanProfile(), second_rng);
    bool same = first.size() == second.size();
    for (std::size_t index = 0; same && index < first.size(); ++index) {
        same = first[index].at_ms == second[index].at_ms && first[index].x == second[index].x &&
               first[index].y == second[index].y;
    }
    check(same, "the same seed produces the same path");

    // Easing is pinned at both ends and monotonic.
    for (Ease ease : {Ease::Linear, Ease::Jerk, Ease::In, Ease::Out, Ease::InOut, Ease::CubicBezier}) {
        const double bezier_control[4] = {0.25, 0.1, 0.25, 1.0};
        check(std::fabs(ease_apply(ease, 0.0, bezier_control)) < 1e-6, "ease(0) == 0");
        check(std::fabs(ease_apply(ease, 1.0, bezier_control) - 1.0) < 1e-6, "ease(1) == 1");
        double previous = 0.0;
        bool rising = true;
        for (int step = 0; step <= 100; ++step) {
            const double value = ease_apply(ease, step / 100.0, bezier_control);
            rising = rising && value + 1e-6 >= previous;
            previous = value;
        }
        check(rising, "ease is monotonic");
    }

    // Overshoot and settle extend the path without missing the target.
    HumanProfile bouncy;
    bouncy.overshoot_pct = 12.0;
    bouncy.settle_ms = 40;
    SplitMix64 over_rng(1);
    const auto overshot = trajectory_with({0, 0}, {400, 0}, Screen{}, 200, bouncy, over_rng);
    check(overshot.back().x == 400 && overshot.back().y == 0, "overshoot still lands on the target");
    bool went_past = false;
    for (const Node& node : overshot) {
        went_past = went_past || node.x > 400 || node.x < 0;
    }
    check(went_past, "an overshoot profile goes past the target");
}

void test_move_to_converges() {
    auto fake = std::make_shared<FakeTransport>();
    Device device = Device::open_fake(fake, 500, false, false);
    HumanProfile linear = HumanProfile::linear();
    device.mouse().move_to(200, 150, 80, &linear);

    Screen screen;
    std::int32_t x = 0;
    std::int32_t y = 0;
    device.cursor_state(screen, x, y);
    check(screen.width == 1920 && screen.height == 1080, "the mock screen is 1920x1080");
    check(x == 200 && y == 150, "move_to lands on the target (at " + std::to_string(x) + "," +
                                    std::to_string(y) + ")");
    check(fake->count_opcode(BARRIER) > 0, "move_to flushes with BARRIER");

    // Off-screen targets clamp to the primary monitor.
    device.mouse().move_to(5000, -20, 80, &linear);
    device.cursor_state(screen, x, y);
    check(x == 1919 && y == 0, "targets clamp to the screen");

    // move_by is relative and split at +/-1024.
    fake->raw_writes();
    device.mouse().move_raw(1500, 0);
    check(fake->count_opcode(MOUSE_MOVE) >= 2, "move_raw splits a 1500 px move");
    device.close();
}

void test_scheduler_order() {
    auto fake = std::make_shared<FakeTransport>();
    Device device = Device::open_fake(fake, 500, false, false);
    Scheduler scheduler = device.scheduler();

    std::vector<std::string> order;
    scheduler.pause();
    scheduler.submit(0, 0, [&] { order.push_back("low"); });
    scheduler.submit(0, 10, [&] { order.push_back("high"); });
    scheduler.submit(0, 0, [&] { order.push_back("second"); });
    scheduler.resume();
    scheduler.wait_idle(2000);
    check(order.size() == 3, "three jobs ran");
    check(order == std::vector<std::string>({"high", "low", "second"}),
          "jobs run by priority then submission order");

    // Due time dominates priority.
    order.clear();
    scheduler.pause();
    scheduler.submit(60, 100, [&] { order.push_back("late-high"); });
    scheduler.submit(0, 0, [&] { order.push_back("soon-low"); });
    scheduler.resume();
    scheduler.wait_idle(2000);
    check(order == std::vector<std::string>({"soon-low", "late-high"}), "due time beats priority");

    // Cancel before it can run.
    order.clear();
    scheduler.pause();
    const std::uint64_t id = scheduler.submit(0, 0, [&] { order.push_back("cancelled"); });
    scheduler.cancel(id);
    scheduler.resume();
    scheduler.wait_idle(2000);
    check(order.empty(), "a cancelled job never runs");

    // A failing job is reported, not fatal.
    scheduler.submit(0, 0, [] { throw Error(ErrorCode::Invalid, "boom"); });
    scheduler.wait_idle(2000);
    const auto errors = scheduler.take_errors();
    check(errors.size() == 1 && errors[0].second.code() == ErrorCode::Invalid, "job errors are collected");
    check(scheduler.take_errors().empty(), "take_errors drains");

    // Repeating jobs keep running until cancelled.
    std::atomic<int> ticks{0};
    const std::uint64_t every = scheduler.every(10, 0, [&] { ticks.fetch_add(1); });
    std::this_thread::sleep_for(std::chrono::milliseconds(80));
    scheduler.cancel(every);
    const int seen = ticks.load();
    check(seen >= 2, "every() repeats (" + std::to_string(seen) + ")");
    std::this_thread::sleep_for(std::chrono::milliseconds(40));
    const int stopped = ticks.load();
    std::this_thread::sleep_for(std::chrono::milliseconds(40));
    check(stopped == ticks.load(), "a cancelled every() stops");

    scheduler.shutdown();
    device.close();
}

void test_reconnect_and_raw_command() {
    auto fake = std::make_shared<FakeTransport>();
    Device device = Device::open_fake(fake, 500, false, false);
    const std::uint32_t first = device.session_id();
    device.reconnect();
    check(device.session_id() != first, "reconnect opens a fresh session id");
    check(fake->count_opcode(OPEN) == 2, "reconnect sends OPEN again");

    // The raw escape hatch returns the packet the caller asked for.
    const auto reply = device.command(BARRIER, {}, true);
    check(reply && reply->opcode == static_cast<std::uint8_t>(BARRIER | 0x80), "raw command returns the reply");
    check(reply && reply->payload.size() == 1 && reply->payload[0] == 0, "raw command sees result 0");
    device.close();
}

void test_unsupported_absolute_move_off_screen() {
    // The mock cursor is always on screen, so this only checks the platform
    // helper's contract rather than the error path.
    Screen screen;
    std::int32_t x = 0;
    std::int32_t y = 0;
    const bool available = gdb::native::screen_and_cursor(screen, x, y);
    if (!available) {
        std::printf("note  cursor API unavailable on this platform\n");
    }
}

// ---------------------------------------------------------------------------
// Log sink
// ---------------------------------------------------------------------------

void test_log_sink() {
    std::mutex sink_mutex;
    std::vector<std::pair<std::int32_t, std::string>> lines;
    set_log_sink([&](std::int32_t level, std::string_view text) {
        std::lock_guard<std::mutex> lock(sink_mutex);
        lines.emplace_back(level, std::string(text));
    });

    auto fake = std::make_shared<FakeTransport>();
    // Info while the session opens, Trace for the frames.
    set_log_level(log::kInfo);
    Device device = Device::open_fake(fake, 500, false, false);
    const std::string session = std::to_string(device.session_id());

    set_log_level(log::kTrace);
    fake->inject_reply(BARRIER, device.session_id() ^ 0x55u, 2, {0});
    device.barrier();  // sequence 2
    set_log_level(log::kOff);
    set_log_sink(nullptr);

    bool connected = false;
    bool out = false;
    bool in = false;
    bool skip = false;
    for (const auto& line : lines) {
        if (line.second.rfind("connected serial=MOCK firmware=3.2.0 board=2", 0) == 0) {
            connected = true;
        }
        if (line.second == "out opcode=4 session=" + session + " sequence=2 payload=[]") {
            out = true;
        }
        if (line.second == "in opcode=132 session=" + session + " sequence=2 payload=[0]") {
            in = true;
        }
        if (line.second == "skip opcode=132 session=" + std::to_string(device.session_id() ^ 0x55u) +
                               " sequence=2 (not ours)") {
            skip = true;
        }
    }
    check(connected, "the sink saw the Info connect line");
    check(out, "the sink saw the Trace out line");
    check(in, "the sink saw the Trace in line");
    check(skip, "the sink saw the Trace skip line");
    check(log::level() == log::kOff && log_level() == log::kOff, "the level is off again");

    // Nothing is formatted while the sink is unattached.
    set_log_level(log::kTrace);
    set_log_sink(nullptr);
    device.barrier();
    const std::size_t before = lines.size();
    device.barrier();
    check(lines.size() == before, "no sink, no lines");
    set_log_level(log::kOff);
    device.close();
}

// ---------------------------------------------------------------------------
// Background movements
// ---------------------------------------------------------------------------

void test_background_movement() {
    auto fake = std::make_shared<FakeTransport>();
    Device device = Device::open_fake(fake, 500, false, false);
    const Mouse mouse = device.mouse();
    Screen screen;
    std::int32_t x = 0;
    std::int32_t y = 0;

    // 1. A background absolute move completes and lands on the target.
    {
        Movement movement = mouse.start_move_to(400, 300, MotionOptions::linear(120));
        check(movement.valid(), "start_move_to returns a handle");
        check(!movement.is_finished(), "the movement runs in the background");
        movement.wait(4000);
        check(movement.is_finished(), "the movement finished");
    }
    device.cursor_state(screen, x, y);
    check(x == 400 && y == 300, "the background move landed on the target (at " +
                                    std::to_string(x) + "," + std::to_string(y) + ")");

    // 2. Relative and path moves use the same handle.
    Movement relative = mouse.start_move_by(64, -32, MotionOptions::linear(60));
    relative.wait(2000);
    device.cursor_state(screen, x, y);
    check(x == 464 && y == 268, "start_move_by moved by the delta");

    Movement path = mouse.start_move_path({{300, 300}, {700, 200}}, MotionOptions::linear(80));
    path.wait(4000);
    device.cursor_state(screen, x, y);
    check(x == 700 && y == 200, "start_move_path ends on the last point");

    // 3. Cancel mid-flight: the pointer freezes and no button is left down.
    mouse.button(Button::Left, true);
    const std::int32_t start_x = x;
    Movement cancelled = mouse.start_move_to(1800, 1000, MotionOptions::linear(1500));
    std::this_thread::sleep_for(std::chrono::milliseconds(80));
    cancelled.cancel();
    check_throws([&] { cancelled.wait(3000); }, ErrorCode::Cancelled, "a cancelled movement reports Cancelled");
    device.cursor_state(screen, x, y);
    const std::int32_t stopped_x = x;
    const std::int32_t stopped_y = y;
    check(stopped_x > start_x, "the cancelled move still travelled part of the way");
    check(stopped_x != 1800 || stopped_y != 1000, "the cancelled move did not reach the target");
    std::this_thread::sleep_for(std::chrono::milliseconds(150));
    device.cursor_state(screen, x, y);
    check(x == stopped_x && y == stopped_y, "nothing moves after the cancel");
    check(device.buttons() == button_bit(Button::Left), "the SDK still tracks the held button");
    mouse.release_all();
    check(device.buttons() == 0 && fake->held().buttons == 0, "release_all leaves no button down");

    // 4. A retarget replans from wherever the pointer is now.
    Movement retargeted = mouse.start_move_to(1700, 900, MotionOptions::linear(600));
    std::this_thread::sleep_for(std::chrono::milliseconds(50));
    retargeted.retarget(120, 640);
    retargeted.wait(4000);
    device.cursor_state(screen, x, y);
    check(x == 120 && y == 640, "retarget converges on the new point (at " + std::to_string(x) +
                                    "," + std::to_string(y) + ")");

    // 5. A wait timeout leaves the worker alone and the handle usable.
    Movement slow = mouse.start_move_to(60, 1000, MotionOptions::linear(1200));
    check_throws([&] { slow.wait(10); }, ErrorCode::Timeout, "wait times out");
    check(!slow.is_finished(), "the worker keeps running after a wait timeout");
    slow.cancel();
    check_throws([&] { slow.wait(3000); }, ErrorCode::Cancelled, "the handle is usable after a timeout");

    // 6. Dropping the handle stops the thread instead of waiting for the move.
    const auto started = Clock::now();
    {
        Movement dropped = mouse.start_move_to(1900, 20, MotionOptions::linear(1500));
        std::this_thread::sleep_for(std::chrono::milliseconds(60));
    }
    check(Clock::now() - started < std::chrono::milliseconds(800),
          "the destructor joins the worker instead of finishing the move");
    device.cursor_state(screen, x, y);
    const std::int32_t frozen_x = x;
    const std::int32_t frozen_y = y;
    std::this_thread::sleep_for(std::chrono::milliseconds(200));
    device.cursor_state(screen, x, y);
    check(x == frozen_x && y == frozen_y, "the thread stopped with the handle");

    device.close();
}

// ---------------------------------------------------------------------------
// UF2 packer against the image the Rust packer produced
// ---------------------------------------------------------------------------

void test_uf2_pack_matches_reference() {
    // The task's spelling first, then the real target directory.
    const char* elfs[] = {
        "firmware/app/target/thumbv6m-nooose-eabi/release/xiao-firmware",
        "firmware/app/target/thumbv6m-none-eabi/release/xiao-firmware",
    };
    std::string elf;
    for (const char* candidate : elfs) {
        if (flash::is_regular_file(candidate)) {
            elf = candidate;
            break;
        }
    }
    std::string reference;
    const char* local = std::getenv("LOCALAPPDATA");
    if (local != nullptr) {
        reference = std::string(local) + "\\FirmwareManageTool_Rust\\Firmware.uf2";
    }
    if (!flash::is_regular_file(reference)) {
        reference = "C:\\Users\\Evan\\AppData\\Local\\FirmwareManageTool_Rust\\Firmware.uf2";
    }
    if (elf.empty() || !flash::is_regular_file(reference)) {
        std::printf("skip  UF2 packer: %s and %s are not both built\n",
                    elf.empty() ? "firmware/app/target/.../xiao-firmware" : elf.c_str(),
                    "Firmware.uf2");
        return;
    }

    const flash::Image packed = flash::uf2_pack(flash::read_binary_file(elf));
    const std::vector<std::uint8_t> expected = flash::read_binary_file(reference);
    check(packed.blocks == 128, "the image is 128 blocks (" + std::to_string(packed.blocks) + ")");
    check(packed.bytes.size() == 65536, "the image is 65536 bytes (" +
                                            std::to_string(packed.bytes.size()) + ")");
    check(flash::uf2_blocks(packed.bytes) == packed.blocks, "uf2_blocks agrees with the packer");
    check(packed.bytes.size() == expected.size(), "the reference is the same size");
    const auto same_as = [&packed](const std::vector<std::uint8_t>& other) {
        return packed.bytes.size() == other.size() &&
               std::equal(packed.bytes.begin(), packed.bytes.end(), other.begin());
    };
    bool identical = same_as(expected);
    if (!identical) {
        // The saved image can predate the ELF it was built from. When it does,
        // the port still has to match the Rust packer — that is the authority it
        // follows — so ask the Rust packer directly and report which is stale.
        const char* base = std::getenv("TEMP");
        const std::string temp =
            std::string(base != nullptr ? base : ".") + "/gdb_native_packer.uf2";
        for (const char* packer_path : {"target/release/uf2pack.exe", "target/debug/uf2pack.exe"}) {
            if (!flash::is_regular_file(packer_path)) {
                continue;
            }
            std::string packer = packer_path;
            std::string elf_argument = elf;
            std::replace(packer.begin(), packer.end(), '/', '\\');
            std::replace(elf_argument.begin(), elf_argument.end(), '/', '\\');
            std::remove(temp.c_str());
            flash::SystemRunner runner;
            flash::Token token;
            std::vector<std::string> streamed;
            const flash::Outcome outcome =
                runner.run(packer, {elf_argument, temp}, 30000, token,
                           [&streamed](flash::Stream, const std::string& line) {
                               streamed.push_back(line);
                           });
            if (!outcome.exit_code.has_value() || *outcome.exit_code != 0) {
                continue;
            }
            check(!streamed.empty(), "SystemRunner streams the child's lines");
            const std::vector<std::uint8_t> from_rust = flash::read_binary_file(temp);
            identical = same_as(from_rust);
            std::printf("note  %s predates the ELF; the Rust packer produces %s\n",
                        reference.c_str(),
                        identical ? "these exact bytes" : "different bytes");
            std::remove(temp.c_str());
            break;
        }
    }
    check(identical, "the packed bytes are byte-identical to the reference UF2 (or to the Rust "
                     "packer when that reference is stale)");

    // Structure, independent of the reference file.
    check(read_u32(&packed.bytes[0]) == 0x0A324655u, "block 0 starts with the UF2 magic");
    check(read_u32(&packed.bytes[4]) == 0x9E5D5157u, "block 0 carries the second magic word");
    check(read_u32(&packed.bytes[8]) == 0x00002000u, "block 0 carries the family flag");
    check(read_u32(&packed.bytes[16]) == 256, "each block carries 256 payload bytes");
    check(read_u32(&packed.bytes[20]) == 0, "block 0 is block 0");
    check(read_u32(&packed.bytes[24]) == 128, "block 0 knows the block count");
    check(read_u32(&packed.bytes[28]) == flash::FAMILY_RP2040, "block 0 names the RP2040 family");
    check(read_u32(&packed.bytes[12]) == 0x10000000u, "block 0 targets the start of XIP flash");
    check(read_u32(&packed.bytes[508]) == 0x0AB16F30u, "block 0 ends with the end magic");
}

// ---------------------------------------------------------------------------
// Flash flow over FakeTransport and a temporary drive
// ---------------------------------------------------------------------------

class TempDir {
public:
    explicit TempDir(const std::string& tag) {
        static std::atomic<int> next{0};
        path_ = temp_path() + "/gdb_native_" + tag + "_" + std::to_string(++next);
        make_directory(path_);
    }
    ~TempDir() {
        // Best effort: one folder and two files, no recursive walk needed.
        remove_file(path_ + "/RPI-RP2/Firmware.uf2");
        remove_file(path_ + "/RPI-RP2/INFO_UF2.TXT");
        remove_directory(path_ + "/RPI-RP2");
        remove_directory(path_);
    }
    TempDir(const TempDir&) = delete;
    TempDir& operator=(const TempDir&) = delete;

    const std::string& path() const { return path_; }

    // No <filesystem>: the MinGW runtime DLL beside this toolchain is older than
    // the header, and its filesystem entry points are missing here.
    static void make_directory(const std::string& path) {
#if defined(_WIN32)
        CreateDirectoryA(path.c_str(), nullptr);
#else
        (void)std::system(("mkdir -p '" + path + "' 2>/dev/null").c_str());
#endif
    }

    static void remove_directory(const std::string& path) {
#if defined(_WIN32)
        RemoveDirectoryA(path.c_str());
#else
        (void)std::system(("rmdir '" + path + "' 2>/dev/null").c_str());
#endif
    }

    static void remove_file(const std::string& path) { std::remove(path.c_str()); }

private:
    static std::string temp_path() {
        for (const char* name : {"TEMP", "TMP"}) {
            const char* value = std::getenv(name);
            if (value != nullptr && value[0] != '\0') {
                return std::string(value);
            }
        }
        return ".";
    }

    std::string path_;
};

void test_flash_flow() {
    TempDir temp("flash");
    const std::string drive = temp.path() + "/RPI-RP2";
    TempDir::make_directory(drive);
    {
        std::ofstream info(drive + "/INFO_UF2.TXT");
        info << "UF2 Bootloader v3.0\nModel: Raspberry Pi RP2\nBoard-ID: RPI-RP2\n";
    }

    flash::Image image;
    image.bytes.resize(4096);
    for (std::size_t index = 0; index < image.bytes.size(); ++index) {
        image.bytes[index] = static_cast<std::uint8_t>((index * 7 + 3) & 0xFF);
    }
    image.blocks = image.bytes.size() / 512;

    auto fake = std::make_shared<FakeTransport>();
    Device device = Device::open_fake(fake, 500, false, false);

    auto verify_fake = std::make_shared<FakeTransport>();
    int enumerations = 0;
    int lookups = 0;
    flash::Hooks hooks;
    // The drive only appears once this command caused the reboot.
    hooks.drives = [&drive, &enumerations] {
        return ++enumerations <= 1 ? std::vector<std::string>{}
                                   : std::vector<std::string>{drive};
    };
    // Same for the board: nothing is in runtime before the copy, one device is
    // back afterwards.
    hooks.devices = [&lookups] {
        if (++lookups <= 1) {
            return std::vector<DeviceEntry>{};
        }
        DeviceEntry entry;
        entry.path = "fake-runtime";
        entry.serial = "MOCK";
        entry.product = "fake";
        return std::vector<DeviceEntry>{entry};
    };
    hooks.open_device = [&verify_fake](const std::string&) {
        return Device::open_fake(verify_fake, 500, false, false);
    };

    flash::Config config;
    config.drive_wait_ms = 2000;
    config.handshake_wait_ms = 500;
    config.poll_ms = 40;

    flash::QuietReporter progress;
    flash::Token cancel;
    const flash::Handshake handshake = flash::push(config, &device, image, cancel, progress, hooks);
    check(handshake.ok, "the flashed board verified (" + handshake.failure + ")");
    check(handshake.verified.board == flash::BOARD_ID, "the fresh Feature 14 read reports the XIAO");
    check(handshake.verified.lease_ms == 2000, "the fresh Feature 14 read reports the lease");
    check(fake->count_opcode(BOOTLOADER) == 1, "opcode 9 was written");
    check(fake->closed(), "the runtime session was closed before the drive was waited for");
    check(verify_fake->count_opcode(OPEN) >= 1, "verification opened a second session");

    const std::vector<std::uint8_t> copied = flash::read_binary_file(drive + "/Firmware.uf2");
    check(copied == image.bytes, "the image landed on the drive byte for byte");

    // A folder that is not a UF2 drive is refused after the copy stage.
    flash::Config pinned;
    pinned.auto_bootloader = false;
    pinned.boot_drive = temp.path();
    check_throws([&] { flash::push(pinned, nullptr, image, cancel, progress, hooks); },
                 ErrorCode::Invalid, "a folder without INFO_UF2.TXT is refused");

    // No drive inside the window is a timeout, not a hang.
    auto lonely_fake = std::make_shared<FakeTransport>();
    Device lonely = Device::open_fake(lonely_fake, 500, false, false);
    flash::Hooks no_drives;
    no_drives.drives = [] { return std::vector<std::string>{}; };
    flash::Config impatient;
    impatient.drive_wait_ms = 150;
    impatient.poll_ms = 20;
    const auto started = Clock::now();
    check_throws([&] { flash::push(impatient, &lonely, image, cancel, progress, no_drives); },
                 ErrorCode::Timeout, "no drive within the window is a timeout");
    check(Clock::now() - started < std::chrono::seconds(3), "the drive wait gives up on time");
    check(lonely_fake->count_opcode(BOOTLOADER) == 1, "the timeout path still asked for BOOTSEL");

    // Two fresh drives at once: refuse rather than guess which board to flash.
    auto twin_fake = std::make_shared<FakeTransport>();
    Device twin = Device::open_fake(twin_fake, 500, false, false);
    int twin_enumerations = 0;
    flash::Hooks two_drives;
    two_drives.drives = [&drive, &twin_enumerations] {
        return ++twin_enumerations <= 1
                   ? std::vector<std::string>{}
                   : std::vector<std::string>{drive, drive + "-other"};
    };
    two_drives.devices = hooks.devices;
    bool refused = false;
    try {
        flash::push(impatient, &twin, image, cancel, progress, two_drives);
    } catch (const Error& error) {
        refused = error.code() == ErrorCode::Invalid &&
                  std::string(error.what()).find("多個 UF2 磁碟") != std::string::npos;
    }
    check(refused, "two mounted drives are refused instead of guessed");

    // The wrong board never verifies, even though the copy succeeded.
    auto wrong_fake = std::make_shared<FakeTransport>();
    wrong_fake->set_board(1);
    flash::Hooks wrong_hooks = hooks;
    wrong_hooks.open_device = [&wrong_fake](const std::string&) {
        return Device::open_fake(wrong_fake, 500, false, false);
    };
    auto second_fake = std::make_shared<FakeTransport>();
    Device second = Device::open_fake(second_fake, 500, false, false);
    flash::Config quick = config;
    quick.handshake_wait_ms = 120;
    quick.poll_ms = 20;
    enumerations = 0;
    lookups = 0;
    const flash::Handshake mismatched =
        flash::push(quick, &second, image, cancel, progress, wrong_hooks);
    check(!mismatched.ok, "a Pico build does not verify as a XIAO");
    check(mismatched.failure.find("板型不符") != std::string::npos,
          "the mismatch names the board id (" + mismatched.failure + ")");

    // An already-cancelled token stops before anything is sent.
    flash::Token stopped;
    stopped.cancel();
    check_throws([&] { flash::push(config, nullptr, image, stopped, progress, hooks); },
                 ErrorCode::Cancelled, "a cancelled token aborts the push");
}

class RecordingReporter final : public flash::Reporter {
public:
    void log(const std::string& line) override { lines.push_back(line); }
    void stage(std::uint8_t stage, const std::string& message) override {
        stages.emplace_back(stage, message);
    }

    std::vector<std::string> lines;
    std::vector<std::pair<std::uint8_t, std::string>> stages;
};

class RecordingRunner final : public flash::Runner {
public:
    explicit RecordingRunner(std::optional<int> exit_code = 0) : exit_code_(exit_code) {}

    flash::Outcome run(const std::string& program, const std::vector<std::string>& args,
                       std::uint32_t, const flash::Cancel&,
                       const std::function<void(flash::Stream, const std::string&)>& line) override {
        std::string joined = program;
        for (const std::string& arg : args) {
            joined += " " + arg;
        }
        commands.push_back(joined);
        line(flash::Stream::Stdout, "Compiling xiao-firmware v0.1.0");
        flash::Outcome outcome;
        outcome.exit_code = exit_code_;
        outcome.out = "Compiling\n";
        outcome.err = exit_code_ == 0 ? "" : "injected failure";
        return outcome;
    }

    std::vector<std::string> commands;

private:
    std::optional<int> exit_code_;
};

void test_flash_build() {
    TempDir temp("build");
    const std::string firmware = temp.path() + "/firmware";
    const std::string app = firmware + "/app";
    TempDir::make_directory(firmware);
    TempDir::make_directory(app);
    const std::string manifest = app + "/Cargo.toml";
    {
        std::ofstream file(manifest);
        file << "[package]\nname = \"xiao-firmware\"\n";
    }

    flash::Config config;
    config.firmware = firmware;
    // A stand-in cargo: any existing file will do, nothing is executed, and
    // rustup is not beside it, so the target install is skipped.
    config.cargo = manifest;

    RecordingRunner runner;
    RecordingReporter progress;
    flash::Token cancel;
    check_throws([&] { flash::build(config, runner, cancel, progress); }, ErrorCode::Invalid,
                 "a build that produced no ELF is reported by name");
    check(progress.stages.size() >= 2 && progress.stages[0].first == 1 &&
              progress.stages[1].first == 2,
          "the build reports stage 1 then stage 2");
    check(progress.stages[0].second == "檢查 Rust 工具鏈", "stage 1 names the toolchain check");
    check(progress.stages[1].second == "編譯 Rust 韌體", "stage 2 names the compile");
    check(runner.commands.size() == 1, "only cargo ran, rustup was not beside it");
    check(!runner.commands.empty() && runner.commands[0].find("build --release --target "
                                                              "thumbv6m-none-eabi") !=
                                          std::string::npos,
          "cargo builds the pinned target");
    check(!runner.commands.empty() &&
              runner.commands[0].find("--features hardware") != std::string::npos,
          "only the build carries --features");
    check(!runner.commands.empty() && runner.commands[0].find("--manifest-path ") != std::string::npos,
          "cargo is pointed at app/Cargo.toml");
    bool skipped_rustup = false;
    bool streamed = false;
    for (const std::string& line : progress.lines) {
        skipped_rustup = skipped_rustup || line.find("找不到 rustup") != std::string::npos;
        streamed = streamed || line == "Stdout: Compiling xiao-firmware v0.1.0";
    }
    check(skipped_rustup, "a missing rustup is logged, not fatal");
    check(streamed, "every child line reaches the progress log");

    // A missing manifest is refused before anything runs.
    RecordingRunner untouched;
    flash::Config nowhere;
    nowhere.firmware = firmware + "/nowhere";
    check_throws([&] { flash::build(nowhere, untouched, cancel, progress); }, ErrorCode::Invalid,
                 "a firmware folder without app/Cargo.toml is refused");
    check(untouched.commands.empty(), "nothing was executed for a missing manifest");

    // A failing child keeps its stderr and exit code in the message.
    RecordingRunner failing(9);
    flash::Config ready;
    ready.firmware = firmware;
    ready.cargo = manifest;
    ready.skip_dependencies = true;
    bool reported = false;
    try {
        flash::build(ready, failing, cancel, progress);
    } catch (const Error& error) {
        reported = error.code() == ErrorCode::Io &&
                   std::string(error.what()).find("子程序失敗，exit=9") != std::string::npos;
    }
    check(reported, "a failing cargo is reported with its exit code");

    // A cancelled token stops before the first stage.
    flash::Token stopped;
    stopped.cancel();
    check_throws([&] { flash::build(ready, untouched, stopped, progress); }, ErrorCode::Cancelled,
                 "a cancelled build never starts");

    TempDir::remove_file(manifest);
    TempDir::remove_directory(app);
    TempDir::remove_directory(firmware);
}

}  // namespace

int main() {
    test_wire_vectors();
    test_feature_and_info();
    test_typed_frames_from_device();
    test_reply_matching();
    test_event_maps_to_error();
    test_heartbeat_on_idle();
    test_status_polling_keeps_heartbeat();
    test_no_heartbeat_when_disabled();
    test_status_and_sessions();
    test_close_and_bootloader();
    test_release_all_and_buttons();
    test_keyboard_table();
    test_motion_maths();
    test_move_to_converges();
    test_scheduler_order();
    test_reconnect_and_raw_command();
    test_unsupported_absolute_move_off_screen();
    test_log_sink();
    test_background_movement();
    test_uf2_pack_matches_reference();
    test_flash_flow();
    test_flash_build();

    if (failures != 0) {
        std::printf("%d check(s) failed\n", failures);
        return 1;
    }
    std::printf("gdb_native ok\n");
    return 0;
}
