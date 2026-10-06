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
    active_session = received_sequence = completed_sequence = 0;
    binary::pending = {};
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
int main() {
    fresh();
    for (const auto &wire : WireVectors) {
        vendorSetReport(0, HID_REPORT_TYPE_INVALID, wire, 64);
        Firmware::loop();
        tick(12);
    }
    assert(output.desired.keys[30 / 8] & (1 << (30 % 8)));
    assert(output.desired.keys[89 / 8] & (1 << (89 % 8)));
    assert(output.desired.consumer == 2);
    assert(completed_sequence == 5);
    fresh();
    uint8_t feature[63];
    assert(binary::feature(14, HID_REPORT_TYPE_FEATURE, feature, 63) == 63);
    assert(memcmp(feature, "FMT3", 4) == 0);
    assert(binary::read16(feature + 9) == 2000);
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
    assert(active_session == 43 && replied(binary::Open, 1));
    send(binary::Key, 3, {7, 0, 4, 0, 1}, 42);
    tick(8);
    assert(!output.desired.held());
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
    puts("RP2040 firmware: short taps, NKRO/media, barriers, v2 compatibility, ownership, sequence "
         "faults, lease, overflow, stale output and disconnect passed");
}
