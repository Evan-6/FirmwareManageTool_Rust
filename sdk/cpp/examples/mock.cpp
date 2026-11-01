// Runs the whole SDK surface against the in-memory device: no hardware.
//
//   g++ -std=c++17 -I sdk/ffi/include -I sdk/cpp sdk/cpp/examples/mock.cpp -o mock.exe
//   GDBOX_LIBRARY=<repo>/target/debug/gdb.dll ./mock.exe

#include <cassert>
#include <atomic>
#include <cstdint>
#include <iostream>
#include <string>
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

    assert(lines.load() > 0 && "the log sink saw nothing");
    gdb::set_log_sink(nullptr);
    gdb::set_log_level(0);

    device.close();
    std::cout << "cpp mock ok\n";
    return 0;
}
