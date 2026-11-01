// Runs the whole SDK surface against the in-memory device: no hardware.
//
//   g++ -std=c++17 -I sdk/ffi/include -I sdk/cpp sdk/cpp/examples/mock.cpp -o mock.exe
//   GDBOX_LIBRARY=<repo>/target/debug/gdb.dll ./mock.exe

#include <cassert>
#include <atomic>
#include <chrono>
#include <cstdint>
#include <iostream>
#include <string>
#include <thread>
#include <vector>

#include "gdb.hpp"

int main() {
    std::cout << "abi " << gdb::abi_version() << " sdk " << gdb::version() << "\n";

    // The sink is process-wide: install it before anything else can log.
    std::atomic<int> lines{0};
    gdb::set_log_level(5);
    gdb::set_log_sink([&lines](int32_t, const std::string&) { ++lines; });

    auto device = gdb::Device::open_mock();
    const std::string info = device.info_json();
    std::cout << "info " << info << "\n";
    assert(info.find("\"firmware\":\"3.2.0\"") != std::string::npos);
    assert(device.state() == gdb::ConnectionState::Connected);

    device.keyboard().type_text("hello", 1, 1, 0);
    device.keyboard().chord({0xE0, 0x06}, 5);
    device.release_all();

    device.mouse().move_by(60, -30, 10);
    device.mouse().move_to(400, 300, 80);
    device.mouse().move_path({{100, 100}, {200, 150}}, 20);
    device.mouse().click(gdb::Button::Left);
    device.mouse().wheel(120);

    {
        auto movement = device.mouse().start_move_by(400, 200, 300);
        movement.cancel();
        try {
            movement.wait(2000);
            assert(false && "cancelled movement must not succeed");
        } catch (const gdb::Error& error) {
            assert(error.code() == gdb::ErrorCode::Cancelled);
        }
    }

    auto scheduler = device.scheduler();
    int ticks = 0;
    scheduler->pause();
    scheduler->submit(0, 0, [&] { ++ticks; return static_cast<int32_t>(0); });
    scheduler->submit(0, 10, [&] { ticks += 10; return static_cast<int32_t>(0); });
    scheduler->resume();
    scheduler->wait_idle(2000);
    assert(ticks == 11);

    const std::string status = device.status_json();
    std::cout << "status " << status << "\n";

    // Raw moves split at the report limit instead of failing.
    device.mouse().move_raw(2400, -1200);

    // Raw command: STATUS answers, MOUSE_MOVE is fire and forget.
    const auto reply = device.command(0x06);
    assert(reply.has_value());
    assert(reply->opcode == 0x86);
    assert(reply->payload.size() == 51);
    assert(!device.command(0x12, {0x2c, 0x01, 0xd8, 0xff}).has_value());

    // Presence: Feature 14 only, so it is safe while another program holds the
    // session. No board matches these selectors, so both calls are typed
    // disconnects instead of crashes - and neither opens a session.
    try {
        (void)gdb::probe("cpp-mock-no-such-board");
        assert(false && "no board matches that selector");
    } catch (const gdb::Error& error) {
        assert(error.code() == gdb::ErrorCode::Disconnected);
    }
    const std::string boards = gdb::probe_all();
    assert(boards.size() >= 2 && boards.front() == '[' && boards.back() == ']');
    try {
        (void)gdb::Observer::start("cpp-mock-no-such-board", 100);
        assert(false && "no board matches that selector");
    } catch (const gdb::Error& error) {
        assert(error.code() == gdb::ErrorCode::Disconnected);
    }

    // The second surface loads from the same DLL and reports a missing broker
    // as a typed error instead of crashing: attach is how a second program
    // reaches a device another program holds.
    try {
        (void)gdb::Remote::attach("cpp-mock-no-broker");
        assert(false && "no broker is listening under that name");
    } catch (const gdb::Error& error) {
        assert(error.code() == gdb::ErrorCode::Disconnected);
        assert(std::string(error.what()).find("broker") != std::string::npos);
    }

    // A shared handle is the same board for a caller that is not alone on it:
    // it answers like any other, and the counter says whether a reply that was
    // not ours has been seen.
    {
        auto shared = gdb::Device::open_mock_shared();
        assert(shared.state() == gdb::ConnectionState::Connected);
        assert(shared.stolen_frames() == 0);
        (void)shared.status_json();
        assert(shared.stolen_frames() == 0);
        shared.close();
    }

    // Metrics: one sent frame shows up in the counters, and a reset zeroes them.
    // No heartbeat, so nothing moves between the calls and the numbers mean
    // exactly what this block did.
    {
        auto quiet = gdb::Device::open_mock(1000, false, false);
        quiet.reset_metrics();
        (void)quiet.status_json();
        const gdb::Metrics counted = quiet.metrics();
        std::cout << "metrics sent=" << counted.frames_sent
                  << " received=" << counted.frames_received
                  << " p50=" << counted.latency_p50_us << "us\n";
        assert(counted.frames_sent > 0 && "a STATUS frame must be counted");
        assert(counted.frames_received > 0 && "the reply must be counted");
        assert(counted.latency_count > 0 && "a command that waited must be timed");

        quiet.reset_metrics();
        const gdb::Metrics empty = quiet.metrics();
        assert(empty.frames_sent == 0 && empty.frames_received == 0);
        assert(empty.latency_count == 0 && empty.retries == 0);
        quiet.close();
    }

    // Screen targets: monitor 0 is the mock's one screen, and a kind the ABI does
    // not define is the caller's mistake rather than a move.
    device.mouse().move_to(200, 200, gdb::Screen::monitor(0), 20);
    device.mouse().move_path({{100, 100}, {120, 120}}, gdb::Screen::monitor(0), 10);
    {
        auto movement = device.mouse().start_move_to(300, 300, gdb::Screen::monitor(0), 20);
        movement.wait(5000);
        assert(movement.finished());
    }
    try {
        device.mouse().move_to(10, 10, gdb::Screen{3, 0}, 0);
        assert(false && "screen kind 3 is not defined");
    } catch (const gdb::Error& error) {
        assert(error.code() == gdb::ErrorCode::Invalid);
    }
    // The mock pretends to have one 1920x1080 screen, so index 1 names nothing.
    try {
        device.mouse().move_to(10, 10, gdb::Screen::monitor(1), 0);
        assert(false && "the mock has a single screen");
    } catch (const gdb::Error& error) {
        assert(error.code() == gdb::ErrorCode::Unsupported);
    }

    // The watch: the bus right now, and silence once it is stopped. Whether a
    // board is attached decides whether anything arrives, so only the listing is
    // asserted.
    {
        std::atomic<int> events{0};
        auto watched = gdb::Watch::start(
            [&events](int32_t, const std::string&) { ++events; }, 100);
        const std::string bus = watched.listing_json();
        std::cout << "listing " << bus << "\n";
        assert(bus.find("\"devices\"") != std::string::npos);
        assert(bus.find("\"volumes\"") != std::string::npos);
        watched.stop();
        const int seen = events.load();
        std::this_thread::sleep_for(std::chrono::milliseconds(300));
        assert(events.load() == seen && "a stopped watch must not call back");
        watched.close();
    }

    assert(lines.load() > 0 && "the log sink saw nothing");
    gdb::set_log_sink(nullptr);
    gdb::set_log_level(0);

    device.close();
    std::cout << "cpp mock ok\n";
    return 0;
}
