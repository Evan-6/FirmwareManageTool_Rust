// Exercise the real firmware through fake USB completions, not a mirrored implementation.
#include <assert.h>
#include <new>
#include <stdio.h>
unsigned long test_time = 0;
#include "../boards/rp2040/Firmware.cpp"
#include "ProtocolVectors.h"
void tick(unsigned n = 1) {
    for (unsigned i = 0; i < n; ++i) {
        const size_t count = test_reports.size();
        for (size_t j = 0; j < count; ++j)
            if (!test_reports[j].delivered) {
                auto &r = test_reports[j];
                r.delivered = true;
                if (r.instance == 0)
                    usb_hid.ready_ = true;
                else
                    usb_vendor.ready_ = true;
                std::vector<uint8_t> bytes{r.id};
                bytes.insert(bytes.end(), r.data.begin(), r.data.end());
                tud_hid_report_complete_cb(r.instance, bytes.data(), bytes.size());
            }
        ++test_time;
        Firmware::loop();
    }
}
void fresh() {
    output.~OutputScheduler();
    new (&output) OutputScheduler;
    binary_released = false;
    releasing_protocol = protocol_owner = ProtocolOwner::None;
    binary::resetSessions();
    binary::boot_requested = false;
    binary::overflow.store(false);
    binary::invalid_report.store(false);
    binary::boot_reply_inflight = false;
    binary::boot_reply_completed = false;
    usb_hid.ready_ = usb_vendor.ready_ = true;
    test_reports.clear();
    TinyUSBDevice.mounted_ = true;
    bootloader_reset_pending = false;
    Firmware::setup();
    tick(12);
    test_reports.clear();
}
void send(uint8_t op, uint32_t seq, std::vector<uint8_t> payload = {}, uint32_t session = 42) {
    uint8_t r[64] = {12, 3, op, static_cast<uint8_t>(payload.size()), 0};
    binary::write32(r + 5, session);
    binary::write32(r + 9, seq);
    if (!payload.empty())
        memcpy(r + 13, payload.data(), payload.size());
    vendorSetReport(0, HID_REPORT_TYPE_INVALID, r, 64);
    Firmware::loop();
}
void open() {
    send(binary::Open, 1);
    tick(12);
    assert(protocol_owner == ProtocolOwner::Binary);
    test_reports.clear();
}
void legacy(const char *command) {
    char line[128];
    snprintf(line, sizeof(line), "%s", command);
    handleCommand(line);
}
bool replied(uint8_t op, uint32_t seq) {
    for (const auto &r : test_reports)
        if (r.id == 13 && r.data[1] == (op | 128) && binary::read32(r.data.data() + 8) == seq)
            return true;
    return false;
}
void assertClicks(uint8_t button, unsigned expected = 1) {
    uint8_t previous = 0;
    unsigned downs = 0, ups = 0;
    for (const auto &r : test_reports) {
        assert(r.id != 4); // Removed absolute mouse report must never be emitted.
        if (r.id != MouseReportId)
            continue;
        const bool before = (previous & button) != 0;
        const bool after = (r.data[0] & button) != 0;
        downs += !before && after;
        ups += before && !after;
        previous = r.data[0];
    }
    assert(downs == expected && ups == expected && previous == 0);
}
void testRelativeMouseOnly() {
    unsigned ids = 0;
    for (size_t i = 0; i + 1 < sizeof(HidReportDescriptor); ++i)
        if (HidReportDescriptor[i] == 0x85)
            ids |= 1u << HidReportDescriptor[i + 1];
    assert(ids == ((1u << KeyboardReportId) | (1u << ConsumerReportId) |
                   (1u << MouseReportId)));
    for (uint8_t button : {1, 2, 4, 8, 16}) {
        fresh();
        open();
        send(binary::Buttons, 2, {button});
        send(binary::Move, 3, {10, 0, 20, 0});
        send(binary::Wheel, 4, {1, 0, 0, 0});
        std::vector<uint8_t> snapshot(31);
        snapshot[30] = button;
        send(binary::Snapshot, 5, snapshot);
        send(binary::Heartbeat, 6);
        tick(15);
        for (const auto &r : test_reports)
            if (r.id == MouseReportId)
                assert(r.data[0] == button);
        send(binary::Buttons, 7, {0});
        send(binary::Snapshot, 8, std::vector<uint8_t>(31));
        send(binary::Barrier, 9);
        tick(12);
        assert(replied(binary::Barrier, 9));
        assertClicks(button);
    }
}
void testRealRapidClicksArePreserved() {
    fresh();
    open();
    for (uint32_t seq = 2; seq < 8; ++seq)
        send(binary::Buttons, seq, {uint8_t(!(seq & 1))});
    send(binary::Barrier, 8);
    tick(15);
    assert(replied(binary::Barrier, 8));
    assertClicks(1, 3);
}
void testMouseReleaseAndFaults() {
    for (unsigned scenario = 0; scenario < 4; ++scenario) {
        fresh();
        open();
        send(binary::Buttons, 2, {1});
        tick(scenario == 0 ? 1 : 8);
        if (scenario == 0)
            send(binary::Release, 3);
        else if (scenario == 1) {
            test_time += 2001;
            Firmware::loop();
        } else if (scenario == 2) {
            tud_hid_report_failed_cb(0, HID_REPORT_TYPE_INPUT, nullptr, 0);
            Firmware::loop();
        } else {
            TinyUSBDevice.mounted_ = false;
            Firmware::loop();
            TinyUSBDevice.mounted_ = true;
        }
        tick(15);
        assert(!output.completed.held());
        if (scenario == 0)
            assert(replied(binary::Release, 3));
        else
            assert(protocol_owner == ProtocolOwner::None);
        assertClicks(1);
    }
}
void testLegacyThenBinaryClick() {
    fresh();
    legacy("mouse_button:1,down");
    legacy("mouse_move:10,20");
    legacy("mouse_button:1,up");
    tick(15);
    assert(output.idle() && protocol_owner == ProtocolOwner::Legacy);
    assertClicks(1);
    legacy("reset");
    tick(12);
    open();
    send(binary::Buttons, 2, {1});
    send(binary::Move, 3, {20, 0, 10, 0});
    send(binary::Buttons, 4, {0});
    send(binary::Barrier, 5);
    tick(15);
    assert(replied(binary::Barrier, 5));
    assertClicks(1);
}
void testOldAbsoluteCommandsAreRejected() {
    fresh();
    legacy("mouse_abs:16384,24576");
    tick(8);
    for (const auto &r : test_reports)
        assert(r.id != MouseReportId && r.id != 4);
    legacy("reset");
    tick(12);
    open();
    send(binary::Buttons, 2, {1});
    tick(8);
    send(binary::Absolute, 3, {0, 0x40, 0, 0x60});
    tick(15);
    assert(protocol_owner == ProtocolOwner::None && !output.completed.held());
    assertClicks(1);
}
void openPeer(uint32_t id = 43) {
    send(binary::Open, 1, {}, id);
    tick(12);
    assert(binary::find(id) && binary::find(id)->completed == 1);
}
bool responseFor(uint32_t id, uint8_t op, uint32_t seq, uint8_t code = 0) {
    for (const auto &r : test_reports)
        if (r.id == 13 && r.data[1] == op && binary::read32(r.data.data() + 4) == id &&
            binary::read32(r.data.data() + 8) == seq && r.data[12] == code)
            return true;
    return false;
}
void testConcurrentStateAndRelease() {
    fresh();
    open();
    send(binary::Key, 2, {7, 0, 4, 0, 1});
    send(binary::Buttons, 3, {1});
    tick(10);
    openPeer(); // OPEN must preserve the existing client's held key and button.
    assert(output.completed.keys[0] == 16 && output.completed.buttons == 1);
    std::vector<uint8_t> snapshot(31);
    snapshot[0] = 2;
    snapshot[1] = 16; // Same A key, independently owned.
    snapshot[29] = 2;
    snapshot[30] = 3;
    send(binary::Snapshot, 2, snapshot, 43);
    send(binary::Release, 4);
    send(binary::Barrier, 3, {}, 43);
    tick(20);
    assert(output.completed.keys[0] == 16 && output.completed.modifiers == 2 &&
           output.completed.consumer == 2 && output.completed.buttons == 3);
    assert(responseFor(42, binary::Release | 128, 4));
    assert(responseFor(43, binary::Barrier | 128, 3));
    send(binary::Status, 5);
    send(binary::Status, 4, {}, 43);
    tick(5);
    for (const auto &r : test_reports) {
        if (r.id != 13 || r.data[1] != (binary::Status | 128))
            continue;
        const uint32_t id = binary::read32(r.data.data() + 4);
        assert(binary::read32(r.data.data() + 13) == (id == 42 ? 5u : 4u));
        InputState state;
        memcpy(&state, r.data.data() + 32, 31);
        assert(state.held() == (id == 43));
    }
    send(binary::Release, 5, {}, 43);
    tick(12);
    assert(!output.completed.held());
    // A released client can resume while the other remains connected.
    send(binary::Key, 6, {7, 0, 5, 0, 1});
    tick(8);
    assert(output.completed.keys[0] == 32 && binary::count() == 2);
}
void testConcurrentFaultAndLease() {
    for (uint8_t scenario : {0, 1, 2, 3}) {
        fresh();
        open();
        openPeer();
        send(binary::Key, 2, {7, 0, 4, 0, 1});
        send(binary::Key, 2, {7, 0, 5, 0, 1}, 43);
        send(binary::Buttons, 3, {1});
        send(binary::Buttons, 3, {2}, 43);
        tick(15);
        if (scenario == 0)
            send(binary::Key, 3, {7, 0, 4, 0, 0}); // Duplicate.
        else if (scenario == 1)
            send(binary::Key, 5, {7, 0, 4, 0, 0}); // Gap.
        else if (scenario == 2)
            send(binary::Buttons, 4, {32}); // Invalid input.
        else {
            test_time += 1500;
            send(binary::Heartbeat, 4, {}, 43);
            test_time += 501;
            send(binary::Heartbeat, 4); // Late heartbeat must not revive this session.
        }
        tick(15);
        assert(!binary::find(42) && binary::find(43));
        assert(output.completed.keys[0] == 32 && output.completed.buttons == 2);
        send(binary::Barrier, scenario == 3 ? 5 : 4, {}, 43);
        tick(12);
        assert(binary::find(43));
    }
}
void testConcurrentQueuedWork() {
    for (uint8_t scenario : {0, 1}) {
        fresh();
        open();
        openPeer();
        send(binary::Key, 2, {7, 0, 4, 0, 1});
        tick(8);
        // Other client's relative transfer is already in flight when A releases.
        send(binary::Move, 2, {200, 0, 0, 0}, 43);
        if (scenario)
            tick();
        send(binary::Release, 3);
        send(binary::Buttons, 3, {1}, 43);
        send(binary::Buttons, 4, {0}, 43);
        send(binary::Barrier, 5, {}, 43);
        tick(30);
        int motion = 0;
        for (const auto &r : test_reports)
            if (r.id == MouseReportId)
                motion += int8_t(r.data[1]);
        assert(motion == 200);
        assertClicks(1);
        assert(responseFor(42, binary::Release | 128, 3));
        assert(responseFor(43, binary::Barrier | 128, 5));
        assert(!output.completed.held());
    }
    fresh();
    open();
    openPeer();
    // Queued A movement must be cancelled without deleting B's movement or barriers.
    send(binary::Buttons, 2, {1}, 43);
    send(binary::Move, 2, {100, 0, 0, 0});
    send(binary::Move, 3, {50, 0, 0, 0}, 43);
    send(binary::Barrier, 4, {}, 43);
    send(binary::Release, 3);
    tick(25);
    int motion = 0;
    for (const auto &r : test_reports)
        if (r.id == MouseReportId)
            motion += int8_t(r.data[1]);
    assert(motion == 50 && output.completed.buttons == 1);
    assert(responseFor(43, binary::Barrier | 128, 4));
}
void testSessionCapacityAndBootloader() {
    fresh();
    open();
    usb_vendor.ready_ = false;
    send(binary::Bootloader, 2);
    tick(12);
    assert(binary::boot_requested && !bootloader_reset_pending);
    test_time += 2001;
    Firmware::loop();
    tick(12);
    assert(!binary::boot_requested && !binary::count());
    usb_vendor.ready_ = true;
    openPeer(43);
    assert(!bootloader_reset_pending);
    fresh();
    open();
    send(binary::Key, 2, {7, 0, 4, 0, 1});
    tick(8);
    send(binary::Key, 2, {7, 0, 4, 0, 0});
    legacy("d:b");
    assert(protocol_owner == ProtocolOwner::None);
    tick(12);
    legacy("d:b");
    tick(8);
    assert(protocol_owner == ProtocolOwner::Legacy && output.completed.keys[0] == 32);
    fresh();
    open();
    for (uint32_t id = 43; id < 42 + binary::SessionLimit; ++id)
        openPeer(id);
    send(binary::Open, 1, {}, 99);
    tick(4);
    assert(binary::count() == binary::SessionLimit && !binary::find(99));
    assert(responseFor(99, binary::Open | 128, 1, 2));
    send(binary::Key, 2, {7, 0, 4, 0, 1});
    tick(8);
    send(binary::Open, 1);
    tick(4);
    assert(binary::find(42)->received == 2 && output.completed.keys[0] == 16);
    send(binary::Bootloader, 3);
    tick(4);
    assert(responseFor(42, binary::Bootloader | 128, 3, 2));
    assert(!bootloader_reset_pending && !binary::boot_requested);
    // Shared USB failure invalidates all sessions and broadcasts one event per client.
    tud_hid_report_failed_cb(0, HID_REPORT_TYPE_INPUT, nullptr, 0);
    tick(25);
    assert(binary::count() == 0 && !output.completed.held());
    for (uint32_t id = 42; id < 42 + binary::SessionLimit; ++id)
        assert(responseFor(id, binary::Event, id == 42 ? 3 : 1, 6));
}
int main() {
    testConcurrentStateAndRelease();
    testConcurrentFaultAndLease();
    testConcurrentQueuedWork();
    testSessionCapacityAndBootloader();
    testRelativeMouseOnly();
    testRealRapidClicksArePreserved();
    testMouseReleaseAndFaults();
    testLegacyThenBinaryClick();
    testOldAbsoluteCommandsAreRejected();
    fresh();
    for (const auto &wire : WireVectors) {
        vendorSetReport(0, HID_REPORT_TYPE_INVALID, wire, 64);
        Firmware::loop();
        tick(12);
    }
    assert(output.desired.keys[30 / 8] & (1 << (30 % 8)));
    assert(output.desired.keys[89 / 8] & (1 << (89 % 8)));
    assert(output.desired.consumer == 2);
    assert(binary::find(0x12345678)->completed == 5);
    fresh();
    uint8_t feature[63];
    assert(binary::feature(14, HID_REPORT_TYPE_FEATURE, feature, 63) == 63);
    assert(memcmp(feature, "FMT3", 4) == 0);
    assert(feature[6] == 3 && feature[7] == 0 && feature[8] == 3);
    assert(binary::read16(feature + 9) == 2000);
    assert(binary::read32(feature + 14) == 23);
    open();
    send(binary::Key, 2, {7, 0, 4, 0, 1});
    send(binary::Key, 3, {7, 0, 4, 0, 0});
    send(binary::Barrier, 4);
    assert(!replied(binary::Barrier, 4));
    tick(12);
    assert(replied(binary::Barrier, 4));
    std::vector<int> states;
    for (const auto &r : test_reports)
        if (r.id == 2)
            states.push_back((r.data[1] >> 4) & 1);
    assert((states == std::vector<int>{1, 0}));
    for (const auto &r : test_reports)
        if (r.id == 13)
            assert(r.data[1] != (binary::Key | 128));
    fresh();
    open();
    for (uint8_t u = 4; u < 14; ++u)
        send(binary::Key, u - 2, {7, 0, u, 0, 1});
    send(binary::Key, 12, {12, 0, 0xe9, 0, 1});
    tick(25);
    assert(output.desired.consumer == 2);
    assert(output.desired.keys[0] == 0xf0);
    send(binary::Release, 13);
    tick(12);
    assert(replied(binary::Release, 13));
    assert(!output.completed.held());
    fresh();
    open();
    legacy("d:a");
    assert(output.desired.keys[0] == 0);
    assert(protocol_owner == ProtocolOwner::Binary);
    fresh();
    for (const char *k : {"d:a", "d:b", "d:c", "d:d", "d:e", "d:f", "d:g"})
        legacy(k);
    tick(20);
    assert(output.desired.keys[1] == 3);
    assert(!(output.desired.keys[1] & 4));
    send(binary::Open, 1);
    tick(12);
    assert(protocol_owner == ProtocolOwner::Legacy);
    legacy("reset");
    tick(12);
    open();
    fresh();
    open();
    send(binary::Key, 2, {7, 0, 4, 0, 1});
    tick(10);
    test_time += 2001;
    Firmware::loop();
    tick(12);
    assert(protocol_owner == ProtocolOwner::None);
    assert(!output.completed.held());
    fresh();
    open();
    send(binary::Key, 3, {7, 0, 4, 0, 1});
    tick(12);
    assert(protocol_owner == ProtocolOwner::None);
    assert(!output.completed.held());
    fresh();
    open();
    send(binary::Key, 2, {7, 0, 4, 0, 1});
    send(binary::Key, 2, {7, 0, 4, 0, 1});
    tick(12);
    assert(protocol_owner == ProtocolOwner::None);
    fresh();
    open();
    for (uint32_t i = 2; i < 36; ++i)
        send(binary::Key, i, {7, 0, 4, 0, uint8_t(i & 1)});
    tick(12);
    assert(protocol_owner == ProtocolOwner::None);
    assert(!output.completed.held());
    fresh();
    open();
    send(binary::Key, 2, {7, 0, 4, 0, 1});
    test_time += 51;
    Firmware::loop();
    tick(12);
    assert(protocol_owner == ProtocolOwner::None);
    assert(!output.completed.held());
    fresh();
    open();
    send(binary::Key, 2, {7, 0, 4, 0, 1});
    tick(10);
    TinyUSBDevice.mounted_ = false;
    Firmware::loop();
    assert(protocol_owner == ProtocolOwner::None);
    TinyUSBDevice.mounted_ = true;
    tick(12);
    assert(!output.completed.held());

    fresh();
    open();
    send(binary::Key, 2, {7, 0, 4, 0, 1});
    protocolFault(6);
    legacy("d:b");
    assert(protocol_owner == ProtocolOwner::None && !output.desired.held());
    tick(12);
    legacy("d:b");
    tick(12);
    assert(protocol_owner == ProtocolOwner::Legacy && output.desired.held());
    // A completed explicit release permits immediate handoff, with no lease-length pause.
    fresh();
    open();
    send(binary::Release, 2);
    tick(12);
    send(binary::Open, 1, {}, 43);
    tick(12);
    assert(binary::find(43) != nullptr && replied(binary::Open, 1));
    assert(!output.desired.held());
    send(binary::Release, 3, {}, 42);
    tick(8);
    send(binary::Release, 2, {}, 43);
    legacy("d:b");
    assert(protocol_owner == ProtocolOwner::Binary);
    tick(12);
    legacy("d:b");
    tick(12);
    assert(protocol_owner == ProtocolOwner::Legacy && output.desired.held());
    // Motion coalescing preserves sums on each side of a button transition.
    fresh();
    open();
    for (uint32_t seq = 2; seq < 7; ++seq)
        send(binary::Move, seq, {10, 0, 0, 0});
    send(binary::Buttons, 7, {1});
    for (uint32_t seq = 8; seq < 13; ++seq)
        send(binary::Move, seq, {20, 0, 0, 0});
    send(binary::Barrier, 13);
    tick(35);
    int before = 0, after = 0;
    for (const auto &r : test_reports)
        if (r.id == 3) {
            if (r.data[0] & 1)
                after += int8_t(r.data[1]);
            else
                before += int8_t(r.data[1]);
        }
    assert(before == 50 && after == 100 && replied(binary::Barrier, 13));
    // A release invalidates an already in-flight press, then completes an empty report.
    fresh();
    open();
    send(binary::Key, 2, {7, 0, 4, 0, 1});
    ++test_time;
    Firmware::loop();
    send(binary::Release, 3);
    tick(15);
    assert(replied(binary::Release, 3));
    assert(!output.completed.held());
    // USB completion failure terminates the session and prioritizes release.
    fresh();
    open();
    send(binary::Key, 2, {7, 0, 4, 0, 1});
    tud_hid_report_failed_cb(0, HID_REPORT_TYPE_INPUT, nullptr, 0);
    Firmware::loop();
    tick(15);
    assert(protocol_owner == ProtocolOwner::None && !output.completed.held());
    // Both TinyUSB callback forms are accepted; malformed headers cannot execute input.
    fresh();
    uint8_t direct[63] = {3, 1, 0, 0};
    binary::write32(direct + 4, 42);
    binary::write32(direct + 8, 1);
    vendorSetReport(12, HID_REPORT_TYPE_OUTPUT, direct, 63);
    Firmware::loop();
    tick(12);
    assert(protocol_owner == ProtocolOwner::Binary);
    direct[0] = 2;
    direct[1] = 16;
    direct[2] = 5;
    binary::write32(direct + 8, 2);
    vendorSetReport(12, HID_REPORT_TYPE_OUTPUT, direct, 63);
    Firmware::loop();
    tick(12);
    assert(protocol_owner == ProtocolOwner::None && !output.completed.held());
    fresh();
    open();
    vendorSetReport(12, HID_REPORT_TYPE_OUTPUT, direct, 62);
    Firmware::loop();
    tick(12);
    assert(protocol_owner == ProtocolOwner::None);
    bool framed_error = false;
    for (const auto &report : test_reports)
        if (report.id == 13 && report.data[1] == binary::Event && report.data[12] == 1)
            framed_error = true;
    assert(framed_error);
    // Bootloader is scheduled only after the release and the reply USB completion.
    fresh();
    open();
    send(binary::Bootloader, 2);
    assert(!bootloader_reset_pending);
    for (unsigned i = 0; i < 12 && !replied(binary::Bootloader, 2); ++i)
        tick();
    assert(replied(binary::Bootloader, 2) && !bootloader_reset_pending);
    tick();
    assert(bootloader_reset_pending && !output.completed.held());
    puts("RP2040 firmware: relative-only descriptor, five buttons, relative drag, rapid clicks, "
         "eight concurrent sessions, isolated releases/leases/faults, per-session barriers, "
         "mouse fault release, short taps, NKRO/media, barriers, v2 compatibility, ownership, sequence "
         "faults, lease, overflow, stale output and disconnect passed");
}
