// Runs the whole SDK surface against the in-memory device: no hardware.
//
//   g++ -std=c++17 -I sdk/ffi/include -I sdk/cpp sdk/cpp/examples/mock.cpp -o mock.exe
//   GDBOX_LIBRARY=<repo>/target/debug/gdb.dll ./mock.exe

#include <cassert>
#include <atomic>
#include <chrono>
#include <cstdint>
#include <filesystem>
#include <fstream>
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

    // Screen targets: monitor 0 is the mock's one screen, and the containing
    // anchor resolves a point to whichever display holds it instead of making the
    // caller name one.
    device.mouse().move_to(200, 200, gdb::Screen::monitor(0), 20);
    device.mouse().move_path({{100, 100}, {120, 120}}, gdb::Screen::monitor(0), 10);
    {
        auto movement = device.mouse().start_move_to(300, 300, gdb::Screen::monitor(0), 20);
        movement.wait(5000);
        assert(movement.finished());
    }
    {
        const gdb::Screen anchor = gdb::Screen::containing(100, 200);
        assert(anchor.kind == 3 && "the containing anchor is screen kind 3");
        device.mouse().move_to(100, 200, anchor, 20);
    }
    try {
        device.mouse().move_to(10, 10, gdb::Screen{5, 0}, 0);
        assert(false && "screen kind 5 is not defined");
    } catch (const gdb::Error& error) {
        assert(error.code() == gdb::ErrorCode::Invalid);
    }
    // kind 4 is the client area, which a mock has no window for.
    {
        const gdb::Screen client = gdb::Screen::window_client(0x1234);
        assert(client.kind == 4 && "the client anchor is screen kind 4");
        try {
            device.mouse().move_to(10, 10, client, 0);
            assert(false && "the mock has no windows");
        } catch (const gdb::Error& error) {
            assert(error.code() == gdb::ErrorCode::StaleWindow);
            assert(std::string(error.what()).find("client area") != std::string::npos);
        }
    }
    // The mock pretends to have one 1920x1080 screen, so index 1 names a monitor
    // the system does not report.
    try {
        device.mouse().move_to(10, 10, gdb::Screen::monitor(1), 0);
        assert(false && "the mock has a single screen");
    } catch (const gdb::Error& error) {
        assert(error.code() == gdb::ErrorCode::InvalidScreen);
    }
    // A window the mock does not have is a stale one, not a crash.
    try {
        device.mouse().move_to(10, 10, gdb::Screen::window(0x1234), 0);
        assert(false && "the mock has no windows");
    } catch (const gdb::Error& error) {
        assert(error.code() == gdb::ErrorCode::StaleWindow);
    }

    // The clamp counters and the monitor listing: the listing needs no handle and
    // sends no packet, and the counters say a move was pulled back - which the
    // mock never does, because its single screen covers every destination it
    // accepts.
    {
        const gdb::Clamps clamps = device.clamps();
        std::cout << "clamps total=" << clamps.clamped_total
                  << " requested=" << clamps.requested_x << "," << clamps.requested_y
                  << " applied=" << clamps.applied_x << "," << clamps.applied_y << "\n";
        assert(clamps.clamped_total == 0 && "nothing has been clamped yet");

        const std::string listed = gdb::monitors_json();
        std::cout << "monitors " << listed << "\n";
        assert(listed.size() >= 2 && listed.front() == '[' && listed.back() == ']');
        assert(listed.find("\"primary\"") != std::string::npos);
        assert(listed.find("\"dpi\"") != std::string::npos);
    }

    // A desktop the test describes: a second screen at negative coordinates and
    // a window on it, so the clamping is exercised without a real monitor.
    {
        const std::string layout = R"({
            "cursor": [100, 100],
            "monitors": [
                {"rect": [0, 0, 1920, 1080], "dpi": 96, "primary": true},
                {"rect": [-1280, 0, 0, 720], "dpi": 144}
            ],
            "windows": [
                {"hwnd": 4660, "rect": [-1278, -22, -30, 710], "client": [-1270, 9, -38, 679]}
            ]
        })";
        auto custom = gdb::Device::open_mock_layout(layout);
        const std::string listed = custom.monitors_json();
        std::cout << "layout " << listed << "\n";
        assert(listed.find("\"left\":-1280") != std::string::npos);
        assert(listed.find("\"dpi\":144") != std::string::npos);

        // The containing anchor follows the point onto the negative screen.
        const gdb::MoveReport landed = custom.mouse().move_to_report(
            -500, 300, gdb::Screen::containing(-500, 300), 80);
        assert(!landed.clamped);
        assert(landed.applied_x <= -499 && landed.applied_x >= -501);
        assert(landed.applied_y >= 299 && landed.applied_y <= 301);

        // Anchored on the primary monitor the same point is outside it, so it is
        // clamped to the edge and the report says so.
        const gdb::MoveReport clamped = custom.mouse().move_to_report(
            -500, 300, gdb::Screen::primary(), 80);
        assert(clamped.clamped);
        assert(clamped.applied_x == 0 && clamped.applied_y >= 299 && clamped.applied_y <= 301);
        assert(clamped.error_px() == 500);

        // The client area of the declared window, not its frame.
        const gdb::MoveReport client = custom.mouse().move_to_report(
            -1275, 0, gdb::Screen::window_client(4660), 80);
        assert(client.clamped);
        assert(client.applied_x == -1270 && client.applied_y >= 8 && client.applied_y <= 10);

        bool refused = false;
        try {
            gdb::Device::open_mock_layout(R"({"monitors": []})");
        } catch (const gdb::Error& error) {
            // Every failed open surfaces as the loader's Io code, and the ABI's
            // own message is what says why.
            refused = std::string(error.what()).find("at least one monitor") !=
                      std::string::npos;
        }
        assert(refused && "a layout with no monitor must be refused");
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

    // The runtime revision and the capability list: what a host reads before it
    // trusts a symbol its binding assumes.
    {
        const auto [major, minor] = gdb::abi_versions();
        std::cout << "runtime abi " << major << "." << minor << "\n";
        assert(major == gdb::abi_version());
        assert(minor == gdb::abi_minor_version());
        const std::string listed = gdb::capabilities();
        std::cout << "capabilities " << listed << "\n";
        assert(listed.find("\"click_timed\":true") != std::string::npos);
        assert(listed.find("\"replay\":true") != std::string::npos);
        assert(listed.find("\"flash\":true") != std::string::npos);
        assert(listed.find("\"mock_layout\":true") != std::string::npos);
        assert(listed.find("\"event_json\":true") != std::string::npos);
    }

    // The flashing and replay surfaces: an ELF packed and copied, a recording read
    // entry by entry and played. No drive, no toolchain and no board.
    {
        const std::string drives = gdb::flash_drives_json();
        std::cout << "drives " << drives << "\n";
        assert(drives.size() >= 2 && drives.front() == '[' && drives.back() == ']');

        // A minimal ELF32 with one loadable flash segment.
        std::vector<uint8_t> elf(84, 0);
        elf[0] = 0x7f;
        elf[1] = 'E';
        elf[2] = 'L';
        elf[3] = 'F';
        elf[4] = elf[5] = 1;
        const auto put32 = [&elf](std::size_t at, uint32_t value) {
            for (int byte = 0; byte < 4; ++byte) {
                elf[at + byte] = static_cast<uint8_t>(value >> (8 * byte));
            }
        };
        put32(28, 52);   // e_phoff
        elf[42] = 32;    // e_phentsize
        elf[44] = 1;     // e_phnum
        put32(52, 1);    // PT_LOAD
        put32(56, 84);   // p_offset
        put32(64, 0x10000000u);  // p_paddr
        put32(68, 256);  // p_filesz
        elf.resize(84 + 256, 0xAA);

        const auto stamp = std::to_string(
            std::chrono::steady_clock::now().time_since_epoch().count());
        const std::filesystem::path root = std::filesystem::temp_directory_path();
        const std::filesystem::path elf_path = root / ("gdb-cpp-" + stamp + ".elf");
        const std::filesystem::path uf2_path = root / ("gdb-cpp-" + stamp + ".uf2");
        const std::filesystem::path recording_path =
            root / ("gdb-cpp-" + stamp + ".json");
        {
            std::ofstream elf_file(elf_path, std::ios::binary);
            elf_file.write(reinterpret_cast<const char*>(elf.data()),
                           static_cast<std::streamsize>(elf.size()));
        }

        auto image = gdb::FlashImage::pack(elf_path.string());
        const gdb::FlashStats stats = image.stats();
        std::cout << "uf2 " << stats.bytes << " bytes in " << stats.blocks << " blocks\n";
        assert(stats.bytes == 512 && stats.blocks == 1);
        image.write(uf2_path.string());
        {
            std::ifstream uf2(uf2_path, std::ios::binary);
            uint8_t magic[4] = {};
            uf2.read(reinterpret_cast<char*>(magic), 4);
            assert(magic[0] == 0x55 && magic[1] == 0x46 && magic[2] == 0x32);
        }
        image.close();

        // Building is a typed refusal, not a missing symbol.
        bool refused = false;
        try {
            gdb::flash_build();
        } catch (const gdb::Error& error) {
            refused = error.code() == gdb::ErrorCode::Unsupported;
        }
        assert(refused && "flash_build must refuse by design");

        // A recording read entry by entry, then played on the mock.
        {
            std::ofstream recording(recording_path);
            recording << R"({"version":1,"seed":9,"screen":[1280,720],"started_at_ms":0,)"
                      << R"("entries":[{"at_ms":0,"op":"key","page":7,"usage":4,"down":true},)"
                      << R"({"at_ms":20,"op":"barrier"}]})";
        }
        auto replay = gdb::Replay::open(recording_path.string());
        const std::string header = replay.info_json();
        std::cout << "replay " << header << "\n";
        assert(header.find("\"entries\":2") != std::string::npos);
        int entries = 0;
        while (const auto entry = replay.next()) {
            assert(entry->find("\"op\"") != std::string::npos);
            ++entries;
        }
        assert(entries == 2);
        const gdb::ReplayOutcome outcome = replay.play(device, 1.0);
        assert(outcome.played == 2 && outcome.is_complete());
        replay.close();

        std::error_code ignored;
        std::filesystem::remove(elf_path, ignored);
        std::filesystem::remove(uf2_path, ignored);
        std::filesystem::remove(recording_path, ignored);
    }

    // The caller-timed click, the aim-and-click pair and the pursuit move.
    device.mouse().click_timed(gdb::Button::Left, 1, 5, 5, 0);
    device.mouse().click_at(150, 150, gdb::Screen::primary(), gdb::Button::Left, 1, 2, 0, 0);
    device.mouse().move_to_on_pursuit(1000, 400, gdb::Screen::primary());

    // Where the pointer ended: applied is a measurement, so the error is what the
    // move left, not an assumption.
    {
        const gdb::MoveReport report =
            device.mouse().move_to_report(300, 200, gdb::Screen::primary(), 20);
        std::cout << "move error=" << report.error_px()
                  << "px elapsed=" << report.elapsed_us << "us\n";
        assert(report.requested_x == 300 && report.requested_y == 200);
        assert(report.error_px() <= 1);
        assert(!report.clamped);
        assert(report.elapsed_us > 0);
    }

    // The two platform reads: the DPI mode, and a window handle the system does
    // not know (a typed refusal, never a zeroed answer).
    {
        const std::string prepared = gdb::prepare_platform();
        std::cout << "platform " << prepared << "\n";
        assert(prepared.find("\"perMonitor\"") != std::string::npos);
        try {
            (void)gdb::window_metrics_json(0);
            assert(false && "handle 0 is not a window");
        } catch (const gdb::Error& error) {
            assert(error.code() == gdb::ErrorCode::StaleWindow);
        }
    }

    // The device's own error record, which a success does not clear.
    {
        const auto none = device.last_error();
        assert(!none.has_value() || none->code != gdb::ErrorCode::Ok);
    }

    // Events as JSON: closing the device reports the new state.
    {
        auto watched = gdb::Device::open_mock(200, false, false);
        std::atomic<int> seen{0};
        watched.set_event_handler_json([&seen](int32_t kind, const std::string& body) {
            if (kind == 0) {
                assert(body.find("\"state\"") != std::string::npos);
                ++seen;
            }
        });
        watched.close();
        for (int waited = 0; waited < 200 && seen.load() == 0; ++waited) {
            std::this_thread::sleep_for(std::chrono::milliseconds(5));
        }
        assert(seen.load() > 0 && "closing the device produced no JSON event");
    }

    device.close();
    std::cout << "cpp mock ok\n";
    return 0;
}
