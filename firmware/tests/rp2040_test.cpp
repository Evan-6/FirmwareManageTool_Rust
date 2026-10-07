// Exercise the real firmware through fake USB completions, not a mirrored implementation.
#include <assert.h>
#include <new>
#include <stdio.h>
unsigned long test_time = 0;
#include "../boards/rp2040/src/runtime/FirmwareRuntime.h"
#include "../boards/rp2040/src/platform/UsbDescriptors.h"
#include <Arduino.h>
#include <device/dcd.h>
extern "C" void tud_event_hook_cb(uint8_t, uint32_t, bool);
#include <optional>
using namespace hidfw;
using namespace hidfw::board;
using hidfw::wire::ConsumerReportId;
using hidfw::wire::KeyboardReportId;
using hidfw::wire::MouseReportId;
namespace binary = hidfw::wire;
std::optional<FirmwareRuntime> fixture;
FirmwareRuntime &runtime() { return *fixture; }
extern "C" void tud_hid_report_failed_cb(uint8_t, hid_report_type_t, const uint8_t *, uint16_t);
#include "ProtocolVectors.h"
#include "Rp2040UsbGolden.h"
void tick(unsigned n = 1) {
    for (unsigned i = 0; i < n; ++i) {
        const size_t count = test_reports.size();
        for (size_t j = 0; j < count; ++j)
            if (!test_reports[j].delivered) {
                auto &r = test_reports[j];
                r.delivered = true;
                if (r.instance == 0)
                    Adafruit_USBD_HID::devices[0]->ready_ = true;
                else
                    Adafruit_USBD_HID::devices[1]->ready_ = true;
                std::vector<uint8_t> bytes{r.id};
                bytes.insert(bytes.end(), r.data.begin(), r.data.end());
                tud_hid_report_complete_cb(r.instance, bytes.data(), bytes.size());
            }
        ++test_time;
        runtime().loop();
    }
}
void fresh() {
    fixture.reset();
    Adafruit_USBD_HID::next = 0;
    test_reports.clear();
    TinyUSBDevice.mounted_ = true;
    rp2040.rebooted = false;
    fixture.emplace();
    runtime().begin();
    tick(12);
    test_reports.clear();
}
void send(uint8_t op, uint32_t seq, std::vector<uint8_t> payload = {}, uint32_t session = 42) {
    uint8_t r[64] = {12, 3, op, static_cast<uint8_t>(payload.size()), 0};
    binary::write32(r + 5, session);
    binary::write32(r + 9, seq);
    if (!payload.empty())
        memcpy(r + 13, payload.data(), payload.size());
    runtime().transport().receive(0, HID_REPORT_TYPE_INVALID, r, 64);
    runtime().loop();
}
void open() {
    send(binary::Open, 1);
    tick(12);
    assert(runtime().engine().sessionCount() != 0);
    test_reports.clear();
}
bool replied(uint8_t op, uint32_t seq) {
    for (const auto &r : test_reports)
        if (r.id == 13 && r.data[1] == (op | 128) && binary::read32(r.data.data() + 8) == seq)
            return true;
    return false;
}
void testUsbGoldenBytes() {
    fresh();
    assert(HidReportDescriptorLength == sizeof(usb_golden::InputDescriptor));
    assert(memcmp(HidReportDescriptor, usb_golden::InputDescriptor, HidReportDescriptorLength) ==
           0);
    assert(VendorHidReportDescriptorLength == sizeof(usb_golden::VendorDescriptor));
    assert(memcmp(VendorHidReportDescriptor, usb_golden::VendorDescriptor,
                  VendorHidReportDescriptorLength) == 0);
    uint8_t expected[63], actual[63];
    memcpy(expected, usb_golden::PicoFeature, sizeof(expected));
#ifdef ARDUINO_SEEED_XIAO_RP2040
    expected[5] = 2;
#endif
    assert(runtime().transport().feature(14, HID_REPORT_TYPE_FEATURE, actual, 63) == 63);
    assert(memcmp(actual, expected, 63) == 0);
    assert(runtime().transport().feature(14, HID_REPORT_TYPE_FEATURE, actual, 62) == 0);
    assert(runtime().transport().feature(13, HID_REPORT_TYPE_FEATURE, actual, 63) == 0);
    assert(runtime().transport().feature(14, HID_REPORT_TYPE_OUTPUT, actual, 63) == 0);
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
    for (size_t i = 0; i + 1 < HidReportDescriptorLength; ++i)
        if (HidReportDescriptor[i] == 0x85)
            ids |= 1u << HidReportDescriptor[i + 1];
    assert(ids == ((1u << KeyboardReportId) | (1u << ConsumerReportId) | (1u << MouseReportId)));
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
            runtime().loop();
        } else if (scenario == 2) {
            tud_hid_report_failed_cb(0, HID_REPORT_TYPE_INPUT, nullptr, 0);
            runtime().loop();
        } else {
            TinyUSBDevice.mounted_ = false;
            runtime().loop();
            TinyUSBDevice.mounted_ = true;
        }
        tick(15);
        assert(!runtime().engine().completedState().held());
        if (scenario == 0)
            assert(replied(binary::Release, 3));
        else
            assert(runtime().engine().sessionCount() == 0);
        assertClicks(1);
    }
}
void testOldAbsoluteCommandsAreRejected() {
    fresh();
    open();
    send(binary::Buttons, 2, {1});
    tick(8);
    send(binary::Absolute, 3, {0, 0x40, 0, 0x60});
    tick(15);
    assert(runtime().engine().sessionCount() == 0 && !runtime().engine().completedState().held());
    assertClicks(1);
}
void openPeer(uint32_t id = 43) {
    send(binary::Open, 1, {}, id);
    tick(12);
    assert(runtime().engine().session(id) && runtime().engine().session(id)->completed == 1);
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
    assert(runtime().engine().completedState().keys[0] == 16 &&
           runtime().engine().completedState().buttons == 1);
    std::vector<uint8_t> snapshot(31);
    snapshot[0] = 2;
    snapshot[1] = 16; // Same A key, independently owned.
    snapshot[29] = 2;
    snapshot[30] = 3;
    send(binary::Snapshot, 2, snapshot, 43);
    send(binary::Release, 4);
    send(binary::Barrier, 3, {}, 43);
    tick(20);
    assert(runtime().engine().completedState().keys[0] == 16 &&
           runtime().engine().completedState().modifiers == 2 &&
           runtime().engine().completedState().consumer == 2 &&
           runtime().engine().completedState().buttons == 3);
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
    assert(!runtime().engine().completedState().held());
    // A released client can resume while the other remains connected.
    send(binary::Key, 6, {7, 0, 5, 0, 1});
    tick(8);
    assert(runtime().engine().completedState().keys[0] == 32 &&
           runtime().engine().sessionCount() == 2);
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
        assert(!runtime().engine().session(42) && runtime().engine().session(43));
        assert(runtime().engine().completedState().keys[0] == 32 &&
               runtime().engine().completedState().buttons == 2);
        send(binary::Barrier, scenario == 3 ? 5 : 4, {}, 43);
        tick(12);
        assert(runtime().engine().session(43));
    }
}
void testReleasedGuiLeaseDuringLatencyWorker() {
    fresh();
    open(); // GUI session, released before handing off to the worker process.
    send(binary::Release, 2);
    tick(12);
    openPeer(43);
    const uint16_t failsafe = runtime().engine().counters().failsafe;
    const uint16_t hid = runtime().engine().counters().hid, tx = runtime().engine().counters().tx;
    test_time += 1500;
    send(binary::Heartbeat, 2, {}, 43);
    test_time += 501;
    runtime().loop();
    tick(12);
    assert(!runtime().engine().session(42) && runtime().engine().session(43));
    assert(runtime().engine().counters().failsafe == uint16_t(failsafe + 1));
    assert(runtime().engine().counters().hid == hid && runtime().engine().counters().tx == tx);
    assert(responseFor(42, binary::Event, 2, 5));
    assert(!responseFor(43, binary::Event, 2, 5));
    send(binary::Key, 3, {7, 0, 4, 0, 1}, 43);
    tick(8);
    assert(runtime().engine().completedState().keys[0] == 16);
    send(binary::Key, 4, {7, 0, 4, 0, 0}, 43);
    send(binary::Barrier, 5, {}, 43);
    tick(12);
    assert(!runtime().engine().completedState().held() &&
           responseFor(43, binary::Barrier | 128, 5));
    send(binary::Status, 6, {}, 43);
    tick(5);
    assert(responseFor(43, binary::Status | 128, 6));
    for (const auto &r : test_reports)
        if (r.id == 13 && r.data[1] == (binary::Status | 128) &&
            binary::read32(r.data.data() + 4) == 43)
            assert(binary::read16(r.data.data() + 30) == uint16_t(failsafe + 1));
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
        assert(!runtime().engine().completedState().held());
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
    assert(motion == 50 && runtime().engine().completedState().buttons == 1);
    assert(responseFor(43, binary::Barrier | 128, 4));
}
void testSessionCapacityAndBootloader() {
    fresh();
    open();
    Adafruit_USBD_HID::devices[1]->ready_ = false;
    send(binary::Bootloader, 2);
    tick(12);
    assert(runtime().engine().bootRequested() && !runtime().bootloader().pending());
    test_time += 2001;
    runtime().loop();
    tick(12);
    assert(!runtime().engine().bootRequested() && !runtime().engine().sessionCount());
    Adafruit_USBD_HID::devices[1]->ready_ = true;
    openPeer(43);
    assert(!runtime().bootloader().pending());
    fresh();
    open();
    send(binary::Key, 2, {7, 0, 4, 0, 1});
    tick(8);
    send(binary::Key, 2, {7, 0, 4, 0, 0});
    assert(runtime().engine().sessionCount() == 0);
    tick(12);
    assert(!runtime().engine().completedState().held());
    fresh();
    open();
    for (uint32_t id = 43; id < 42 + SessionLimit; ++id)
        openPeer(id);
    send(binary::Open, 1, {}, 99);
    tick(4);
    assert(runtime().engine().sessionCount() == SessionLimit && !runtime().engine().session(99));
    assert(responseFor(99, binary::Open | 128, 1, 2));
    send(binary::Key, 2, {7, 0, 4, 0, 1});
    tick(8);
    send(binary::Open, 1);
    tick(4);
    assert(runtime().engine().session(42)->received == 2 &&
           runtime().engine().completedState().keys[0] == 16);
    send(binary::Bootloader, 3);
    tick(4);
    assert(responseFor(42, binary::Bootloader | 128, 3, 2));
    assert(!runtime().bootloader().pending() && !runtime().engine().bootRequested());
    // Shared USB failure invalidates all sessions and broadcasts one event per client.
    tud_hid_report_failed_cb(0, HID_REPORT_TYPE_INPUT, nullptr, 0);
    tick(25);
    assert(runtime().engine().sessionCount() == 0 && !runtime().engine().completedState().held());
    for (uint32_t id = 42; id < 42 + SessionLimit; ++id)
        assert(responseFor(id, binary::Event, id == 42 ? 3 : 1, 6));
}
void testRendererLivesOnCoreOne() {
#if FIRMWARE_ENABLE_NEOPIXEL
    fresh();
    open();
    test_pixel_frames.clear();
    runtime().beginLed();
    const auto initial = test_pixel_frames.size();
    send(binary::Key, 2, {7, 0, 4, 0, 1});
    tick(12);
    assert(test_pixel_frames.size() == initial);
    runtime().renderLed();
    assert(test_pixel_frames.size() == initial + 1);
    const auto rendered = test_pixel_frames.size();
    runtime().renderLed();
    assert(test_pixel_frames.size() == rendered);
#endif
}
void testRxBudgetFaultPrecedenceAndCount() {
    fresh();
    open();
    uint8_t r[63] = {3, binary::Heartbeat, 0, 0};
    binary::write32(r + 4, 42);
    for (uint32_t seq = 2; seq < 34; ++seq) {
        binary::write32(r + 8, seq);
        vendorReceive(12, HID_REPORT_TYPE_OUTPUT, r, 63);
    }
    runtime().loop();
    assert(runtime().engine().session(42)->received == 5);
    runtime().loop();
    assert(runtime().engine().session(42)->received == 9);
    tick(6);
    assert(runtime().engine().session(42)->received == 33);
    fresh();
    open();
    for (uint32_t seq = 2; seq < 35; ++seq) {
        binary::write32(r + 8, seq);
        vendorReceive(12, HID_REPORT_TYPE_OUTPUT, r, 63);
    }
    vendorReceive(12, HID_REPORT_TYPE_OUTPUT, r, 62);
    vendorReceive(10, HID_REPORT_TYPE_OUTPUT, r, 63);
    runtime().loop();
    tick(12);
    assert(!runtime().engine().sessionCount());
    assert(runtime().engine().counters().rx == 3);
    assert(responseFor(42, binary::Event, 1, 1));
}
void testRejectedSendAndMismatchedCompletion() {
    fresh();
    open();
    test_reports.clear();
    Adafruit_USBD_HID::devices[1]->reject_next = true;
    send(binary::Status, 2);
    assert(!replied(binary::Status, 2));
    runtime().loop();
    assert(replied(binary::Status, 2));
    assert(runtime().engine().counters().tx == 0);
    fresh();
    open();
    Adafruit_USBD_HID::devices[0]->reject_next = true;
    ++test_time;
    send(binary::Key, 2, {7, 0, 4, 0, 1});
    tick(12);
    assert(runtime().engine().counters().hid == 1 && !runtime().engine().sessionCount());
    assert(!runtime().engine().completedState().held());
    fresh();
    open();
    test_reports.clear();
    ++test_time;
    send(binary::Key, 2, {7, 0, 4, 0, 1});
    assert(!test_reports.empty() && test_reports[0].instance == 0);
    test_reports[0].delivered = true; // Keep the input physically pending.
    uint8_t wrong[2] = {ConsumerReportId, 0};
    tud_hid_report_complete_cb(0, wrong, 2);
    send(binary::Barrier, 3);
    runtime().loop();
    assert(!replied(binary::Barrier, 3));
    const auto held = test_reports[0];
    std::vector<uint8_t> ack{held.id};
    ack.insert(ack.end(), held.data.begin(), held.data.end());
    Adafruit_USBD_HID::devices[0]->ready_ = true;
    tud_hid_report_complete_cb(0, ack.data(), ack.size());
    runtime().loop();
    assert(replied(binary::Barrier, 3));
}
void testResetAndBootDeadline() {
    fresh();
    open();
    ++test_time;
    send(binary::Key, 2, {7, 0, 4, 0, 1});
    const auto old = test_reports.back();
    tud_event_hook_cb(0, DCD_EVENT_BUS_RESET, true);
    std::vector<uint8_t> ack{old.id};
    ack.insert(ack.end(), old.data.begin(), old.data.end());
    tud_hid_report_complete_cb(old.instance, ack.data(), ack.size());
    Adafruit_USBD_HID::devices[0]->ready_ = true;
    Adafruit_USBD_HID::devices[1]->ready_ = true;
    runtime().loop();
    tick(12);
    assert(!runtime().engine().sessionCount() && !runtime().engine().completedState().held());
    assert(responseFor(42, binary::Event, 2, 7));
    openPeer();
    assert(!rp2040.rebooted);

    fresh();
    open();
    send(binary::Bootloader, 2);
    while (!replied(binary::Bootloader, 2))
        tick();
    const auto boot_reply = test_reports.back();
    tud_event_hook_cb(0, DCD_EVENT_BUS_RESET, true);
    ack = {boot_reply.id};
    ack.insert(ack.end(), boot_reply.data.begin(), boot_reply.data.end());
    tud_hid_report_complete_cb(1, ack.data(), ack.size());
    Adafruit_USBD_HID::devices[0]->ready_ = true;
    Adafruit_USBD_HID::devices[1]->ready_ = true;
    runtime().loop();
    tick(12);
    test_time += 120;
    runtime().loop();
    assert(!runtime().bootloader().pending() && !rp2040.rebooted);
    openPeer();
    assert(!rp2040.rebooted);

    fresh();
    open();
    send(binary::Bootloader, 2);
    while (!replied(binary::Bootloader, 2))
        tick();
    assert(!runtime().bootloader().pending());
    tick();
    assert(runtime().bootloader().pending());
    const auto at = test_time;
    test_time = at + 119;
    runtime().loop();
    assert(!rp2040.rebooted);
    test_time = at + 120;
    runtime().loop();
    assert(rp2040.rebooted);
}

int main() {
    testConcurrentStateAndRelease();
    testConcurrentFaultAndLease();
    testReleasedGuiLeaseDuringLatencyWorker();
    testConcurrentQueuedWork();
    testSessionCapacityAndBootloader();
    testRelativeMouseOnly();
    testRealRapidClicksArePreserved();
    testMouseReleaseAndFaults();
    testOldAbsoluteCommandsAreRejected();
    fresh();
    for (const auto &wire : WireVectors) {
        runtime().transport().receive(0, HID_REPORT_TYPE_INVALID, wire, 64);
        runtime().loop();
        tick(12);
    }
    assert(runtime().engine().desiredState().keys[30 / 8] & (1 << (30 % 8)));
    assert(runtime().engine().desiredState().keys[89 / 8] & (1 << (89 % 8)));
    assert(runtime().engine().desiredState().consumer == 2);
    assert(runtime().engine().session(0x12345678)->completed == 5);
    fresh();
    uint8_t feature[63];
    assert(runtime().transport().feature(14, HID_REPORT_TYPE_FEATURE, feature, 63) == 63);
    assert(memcmp(feature, "FMT3", 4) == 0);
    assert(feature[6] == 3 && feature[7] == 1 && feature[8] == 0);
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
    assert(runtime().engine().desiredState().consumer == 2);
    assert(runtime().engine().desiredState().keys[0] == 0xf0);
    send(binary::Release, 13);
    tick(12);
    assert(replied(binary::Release, 13));
    assert(!runtime().engine().completedState().held());
    fresh();
    open();
    send(binary::Key, 2, {7, 0, 4, 0, 1});
    tick(10);
    test_time += 2001;
    runtime().loop();
    tick(12);
    assert(runtime().engine().sessionCount() == 0);
    assert(!runtime().engine().completedState().held());
    fresh();
    open();
    send(binary::Key, 3, {7, 0, 4, 0, 1});
    tick(12);
    assert(runtime().engine().sessionCount() == 0);
    assert(!runtime().engine().completedState().held());
    fresh();
    open();
    send(binary::Key, 2, {7, 0, 4, 0, 1});
    send(binary::Key, 2, {7, 0, 4, 0, 1});
    tick(12);
    assert(runtime().engine().sessionCount() == 0);
    fresh();
    open();
    for (uint32_t i = 2; i < 36; ++i)
        send(binary::Key, i, {7, 0, 4, 0, uint8_t(i & 1)});
    tick(12);
    assert(runtime().engine().sessionCount() == 0);
    assert(!runtime().engine().completedState().held());
    fresh();
    open();
    send(binary::Key, 2, {7, 0, 4, 0, 1});
    test_time += 51;
    runtime().loop();
    tick(12);
    assert(runtime().engine().sessionCount() == 0);
    assert(!runtime().engine().completedState().held());
    fresh();
    open();
    send(binary::Key, 2, {7, 0, 4, 0, 1});
    tick(10);
    TinyUSBDevice.mounted_ = false;
    runtime().loop();
    assert(runtime().engine().sessionCount() == 0);
    TinyUSBDevice.mounted_ = true;
    tick(12);
    assert(!runtime().engine().completedState().held());

    fresh();
    open();
    send(binary::Key, 2, {7, 0, 4, 0, 1});
    tud_hid_report_failed_cb(0, HID_REPORT_TYPE_INPUT, nullptr, 0);
    runtime().loop();
    assert(runtime().engine().sessionCount() == 0 && !runtime().engine().desiredState().held());
    tick(12);
    openPeer(43);
    assert(!runtime().engine().desiredState().held());
    // A completed explicit release permits immediate handoff, with no lease-length pause.
    fresh();
    open();
    send(binary::Release, 2);
    tick(12);
    send(binary::Open, 1, {}, 43);
    tick(12);
    assert(runtime().engine().session(43) != nullptr && replied(binary::Open, 1));
    assert(!runtime().engine().desiredState().held());
    send(binary::Release, 3, {}, 42);
    tick(8);
    send(binary::Release, 2, {}, 43);
    tick(12);
    assert(runtime().engine().sessionCount() == 2 && !runtime().engine().desiredState().held());
    // Old text reports are rejected and cannot execute input.
    uint8_t old_report[64] = {10, 'd', ':', 'b', '\n'};
    runtime().transport().receive(0, HID_REPORT_TYPE_INVALID, old_report, 64);
    tick(12);
    assert(runtime().engine().sessionCount() == 0 && !runtime().engine().completedState().held());
    for (const auto &r : test_reports)
        assert(r.id != 11);
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
    runtime().loop();
    send(binary::Release, 3);
    tick(15);
    assert(replied(binary::Release, 3));
    assert(!runtime().engine().completedState().held());
    // USB completion failure terminates the session and prioritizes release.
    fresh();
    open();
    send(binary::Key, 2, {7, 0, 4, 0, 1});
    tud_hid_report_failed_cb(0, HID_REPORT_TYPE_INPUT, nullptr, 0);
    runtime().loop();
    tick(15);
    assert(runtime().engine().sessionCount() == 0 && !runtime().engine().completedState().held());
    // Both TinyUSB callback forms are accepted; malformed headers cannot execute input.
    fresh();
    uint8_t direct[63] = {3, 1, 0, 0};
    binary::write32(direct + 4, 42);
    binary::write32(direct + 8, 1);
    runtime().transport().receive(12, HID_REPORT_TYPE_OUTPUT, direct, 63);
    runtime().loop();
    tick(12);
    assert(runtime().engine().sessionCount() != 0);
    direct[0] = 2;
    direct[1] = 16;
    direct[2] = 5;
    binary::write32(direct + 8, 2);
    runtime().transport().receive(12, HID_REPORT_TYPE_OUTPUT, direct, 63);
    runtime().loop();
    tick(12);
    assert(runtime().engine().sessionCount() == 0 && !runtime().engine().completedState().held());
    fresh();
    open();
    runtime().transport().receive(12, HID_REPORT_TYPE_OUTPUT, direct, 62);
    runtime().loop();
    tick(12);
    assert(runtime().engine().sessionCount() == 0);
    bool framed_error = false;
    for (const auto &report : test_reports)
        if (report.id == 13 && report.data[1] == binary::Event && report.data[12] == 1)
            framed_error = true;
    assert(framed_error);
    // Bootloader is scheduled only after the release and the reply USB completion.
    fresh();
    open();
    send(binary::Bootloader, 2);
    assert(!runtime().bootloader().pending());
    for (unsigned i = 0; i < 12 && !replied(binary::Bootloader, 2); ++i)
        tick();
    assert(replied(binary::Bootloader, 2) && !runtime().bootloader().pending());
    tick();
    assert(runtime().bootloader().pending() && !runtime().engine().completedState().held());
    testRxBudgetFaultPrecedenceAndCount();
    testRejectedSendAndMismatchedCompletion();
    testResetAndBootDeadline();
    testRendererLivesOnCoreOne();
    testUsbGoldenBytes();
    puts("RP2040 firmware: relative-only descriptor, five buttons, relative drag, rapid clicks, "
         "eight concurrent sessions, isolated releases/leases/faults, per-session barriers, "
         "mouse fault release, short taps, NKRO/media, barriers, old report rejection, sequence "
         "faults, lease, overflow, stale output, reset, send rejection, boot ACK/deadline, "
         "RX budget, USB golden bytes and LED ownership passed");
}
