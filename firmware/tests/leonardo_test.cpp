// Real AVR firmware and nonblocking core USB helpers with fake endpoint ACKs.
#include <assert.h>
#include <new>
#include <stdio.h>
unsigned long test_time = 0;
#include "../boards/leonardo_avr/src/runtime/FirmwareRuntime.h"
#include "../boards/leonardo_avr/src/platform/UsbDescriptors.h"
#include <optional>
#include "Rp2040UsbGolden.h"
#include "LeonardoUsbGolden.h"
using namespace hidfw;
namespace binary = hidfw::wire;
std::optional<FirmwareRuntime> fixture;
FirmwareRuntime &runtime() { return *fixture; }
unsigned bootloader_entries = 0;
namespace hidfw {
namespace avr {
void enterBootloader() { ++bootloader_entries; }
} // namespace avr
} // namespace hidfw
void tick(unsigned n = 1) {
    for (unsigned i = 0; i < n; ++i) {
        for (auto &p : test_packets)
            if (!p.delivered) {
                p.delivered = true;
                ep_busy[p.ep] = 0;
            }
        ++test_time;
        runtime().loop();
    }
}
void fresh() {
    fixture.reset();
    PluggableUSB().next_if = 0;
    PluggableUSB().next_ep = 1;
    for (auto &b : ep_busy)
        b = 0;
    for (auto &f : fifo)
        f.clear();
    test_packets.clear();
    test_out.clear();
    control_in.clear();
    control_out.clear();
    USBDevice.configured_ = true;
    USBDevice.suspended_ = false;
    test_allow_write = true;
    bootloader_entries = 0;
    fixture.emplace();
    runtime().begin();
    tick(12);
    test_packets.clear();
}
void send(uint8_t op, uint32_t seq, std::vector<uint8_t> data = {}, uint32_t session = 42) {
    std::vector<uint8_t> r(64);
    r[0] = 12;
    r[1] = 3;
    r[2] = op;
    r[3] = data.size();
    binary::write32(r.data() + 5, session);
    binary::write32(r.data() + 9, seq);
    if (!data.empty())
        memcpy(r.data() + 13, data.data(), data.size());
    test_out.push_back(r);
    runtime().loop();
}
bool replied(uint32_t session, uint8_t op, uint32_t seq, uint8_t code = 0) {
    for (const auto &p : test_packets)
        if (p.ep == 2 && p.bytes[2] == op && binary::read32(p.bytes.data() + 5) == session &&
            binary::read32(p.bytes.data() + 9) == seq && p.bytes[13] == code)
            return true;
    return false;
}
void open(uint32_t session = 42) {
    send(binary::Open, 1, {}, session);
    tick(12);
    assert(runtime().engine().session(session));
    assert(replied(session, 129, 1));
}
void testGoldenUsbAndControlOut() {
    fresh();
    assert(board::HidReportDescriptorLength == sizeof(usb_golden::InputDescriptor));
    assert(memcmp(board::HidReportDescriptor, usb_golden::InputDescriptor,
                  board::HidReportDescriptorLength) == 0);
    assert(board::VendorHidReportDescriptorLength == sizeof(usb_golden::VendorDescriptor));
    assert(memcmp(board::VendorHidReportDescriptor, usb_golden::VendorDescriptor,
                  board::VendorHidReportDescriptorLength) == 0);
    uint8_t feature[63];
    assert(runtime().transport().feature(14, 3, feature, 63) == 63);
    assert(memcmp(feature, usb_golden::LeonardoFeature, 63) == 0);
    assert(runtime().transport().feature(14, 3, feature, 62) == 0);
    assert(runtime().transport().feature(13, 3, feature, 63) == 0);
    assert(runtime().transport().feature(14, 2, feature, 63) == 0);
    // Control OUT feeds the same mailbox as the interrupt OUT endpoint.
    control_in.assign(64, 0);
    control_in[0] = 12;
    control_in[1] = 3;
    control_in[2] = 1;
    binary::write32(control_in.data() + 5, 42);
    binary::write32(control_in.data() + 9, 1);
    USBSetup setup;
    setup.bmRequestType = REQUEST_HOSTTODEVICE_CLASS_INTERFACE;
    setup.bRequest = HID_SET_REPORT;
    setup.wValueH = 2;
    setup.wValueL = 12;
    setup.wLength = 64;
    setup.wIndex = 1;
    assert(runtime().transport().vendorHid().testSetup(setup));
    runtime().loop();
    tick(12);
    assert(replied(42, 129, 1));
}
void testRejectedSendsAndRxFaults() {
    fresh();
    open();
    test_allow_write = false;
    send(binary::Status, 2);
    assert(!runtime().engine().txEmpty() && !replied(42, 131, 2));
    assert(runtime().engine().counters().tx == 0);
    test_allow_write = true;
    tick(6);
    assert(replied(42, 131, 2));
    fresh();
    open();
    test_allow_write = false;
    send(binary::Key, 2, {7, 0, 4, 0, 1});
    assert(runtime().engine().sessionCount() == 0 && runtime().engine().counters().hid == 1);
    test_allow_write = true;
    tick(12);
    assert(replied(42, 127, 2, 6) && !runtime().engine().completedState().held());
    fresh();
    open();
    uint8_t r[63]{};
    r[0] = 3;
    r[1] = 2;
    binary::write32(r + 4, 42);
    binary::write32(r + 8, 2);
    runtime().transport().receive(12, 2, r, 63);
    runtime().transport().receive(12, 2, r, 63); // RX overflow.
    runtime().transport().receive(12, 2, r, 62); // Framing takes precedence.
    runtime().transport().receive(12, 2, r, 62);
    runtime().loop();
    tick(12);
    assert(runtime().engine().counters().rx == 3 && replied(42, 127, 1, 1));
    fresh();
    open();
    ep_busy[2] = 1;
    for (uint32_t seq = 2; seq <= 10; ++seq)
        send(binary::Status, seq);
    runtime().loop();
    assert(runtime().engine().counters().tx == 1);
    assert(runtime().engine().sessionCount() == 0);
    ep_busy[2] = 0;
    tick(12);
    assert(replied(42, 127, 10, 8));
}
void testResetBeforeNewOutPacket() {
    fresh();
    open();
    send(binary::Key, 2, {7, 0, 4, 0, 1});
    tick(6);
    ++test_reset_generation;
    for (auto &b : ep_busy)
        b = 0;
    // The first OPEN after reset may be busy while release completes, but must
    // receive its reply rather than disappear with the old callback mailbox.
    send(binary::Open, 1, {}, 55);
    tick(12);
    assert(replied(42, 127, 2, 7) && replied(55, 129, 1, 2));
    send(binary::Open, 1, {}, 55);
    tick(12);
    assert(runtime().engine().session(55) && replied(55, 129, 1));
}
void testBootDeadlineAndResetAfterAck() {
    fresh();
    open();
    send(binary::Bootloader, 2);
    while (!replied(42, 134, 2))
        tick();
    assert(!runtime().bootloader().pending() && !bootloader_entries);
    tick();
    assert(runtime().bootloader().pending());
    const uint32_t ack = test_time;
    test_time = ack + 119;
    runtime().loop();
    assert(!bootloader_entries);
    test_time = ack + 120;
    runtime().loop();
    assert(bootloader_entries == 1);
    fresh();
    open();
    send(binary::Bootloader, 2);
    while (!replied(42, 134, 2))
        tick();
    tick();
    assert(runtime().bootloader().pending());
    ++test_reset_generation;
    for (auto &b : ep_busy)
        b = 0;
    runtime().loop();
    test_time += 200;
    runtime().loop();
    assert(!runtime().bootloader().pending() && !bootloader_entries);
}
int main() {
    // Full packets have one submission, no ZLP, and completion waits for ACK even
    // though the AVR's second hardware bank still has room.
    uint8_t r[64]{};
    assert(USB_TrySendPacket(1, r, 64));
    assert(test_packets.size() == 1 && test_packets.back().bytes.size() == 64);
    assert(!USB_PacketComplete(1));
    assert(!USB_TrySendPacket(1, r, 64));
    ep_busy[1] = 0;
    assert(USB_PacketComplete(1));
    fresh();
    USBSetup feature;
    feature.bmRequestType = REQUEST_DEVICETOHOST_CLASS_INTERFACE;
    feature.bRequest = HID_GET_REPORT;
    feature.wValueL = 14;
    feature.wValueH = 3;
    feature.wLength = 64;
    feature.wIndex = 1;
    assert(runtime().transport().vendorHid().testSetup(feature));
    assert(control_out.size() == 64 && control_out[0] == 14);
    assert(memcmp(control_out.data() + 1, "FMT3", 4) == 0 && control_out[6] == 3 &&
           control_out[7] == 3 && control_out[8] == 1);
    assert(binary::read32(control_out.data() + 15) == 23 && control_out[12] == 0 &&
           control_out[13] == 0);
    uint8_t interfaces = 0;
    assert(runtime().transport().inputHid().testInterface(&interfaces) == 25);
    assert(control_out[4] == 1);
    assert(runtime().transport().vendorHid().testInterface(&interfaces) == 32 && interfaces == 2 &&
           control_out[4] == 2);
    open();
    // A rapid down/up followed by BARRIER fits the two-slot AVR output queue.
    send(binary::Key, 2, {7, 0, 4, 0, 1});
    send(binary::Key, 3, {7, 0, 4, 0, 0});
    send(binary::Barrier, 4);
    assert(!replied(42, 132, 4));
    tick(12);
    assert(replied(42, 132, 4));
    unsigned downs = 0, ups = 0;
    for (const auto &p : test_packets)
        if (p.ep == 1 && p.bytes[0] == 2) {
            downs += (p.bytes[2] & 16) != 0;
            ups += (p.bytes[2] & 16) == 0;
        }
    assert(downs == 1 && ups >= 1 && !runtime().engine().completedState().held());
    // Eight owners, overlapping keys/buttons, and own release preserves a peer.
    for (uint32_t id = 43; id < 50; ++id)
        open(id);
    assert(runtime().engine().sessionCount() == 8);
    send(binary::Open, 1, {}, 99);
    tick(4);
    assert(replied(99, 129, 1, 2) && runtime().engine().sessionCount() == 8);
    send(binary::Key, 5, {7, 0, 4, 0, 1});
    tick(6);
    send(binary::Buttons, 2, {3}, 43);
    tick(6);
    send(binary::Release, 6);
    tick(12);
    assert(replied(42, 133, 6) && !runtime().engine().completedState().keys[0] &&
           runtime().engine().completedState().buttons == 3);
    send(binary::Bootloader, 7);
    tick(4);
    assert(replied(42, 134, 7, 2) && !runtime().bootloader().pending());
    // A client's pending count does not include another client's queued output.
    send(binary::Buttons, 3, {1}, 43);
    send(binary::Status, 8);
    tick(12);
    bool local_pending = false;
    for (const auto &p : test_packets)
        if (p.ep == 2 && p.bytes[2] == 131 && binary::read32(p.bytes.data() + 5) == 42)
            local_pending = p.bytes[22] == 0;
    assert(local_pending);
    // Output overflow broadcasts a fault to all eight owners and releases HID.
    test_packets.clear();
    send(binary::Key, 9, {7, 0, 4, 0, 1});
    send(binary::Key, 10, {7, 0, 5, 0, 1});
    send(binary::Key, 11, {7, 0, 6, 0, 1});
    tick(30);
    assert(runtime().engine().sessionCount() == 0 && !runtime().engine().completedState().held());
    for (uint32_t id = 42; id < 50; ++id)
        assert(replied(id, 127, id == 42 ? 11 : (id == 43 ? 3 : 1), 8));
    // Eight idle leases can expire together without filling the two-slot output queue.
    fresh();
    for (uint32_t id = 42; id < 50; ++id)
        open(id);
    test_packets.clear();
    test_time += 2001;
    runtime().loop();
    tick(25);
    assert(!runtime().engine().sessionCount() && !runtime().engine().completedState().held());
    for (uint32_t id = 42; id < 50; ++id)
        assert(replied(id, 127, 1, 5));
    fresh();
    open();
    send(binary::Key, 2, {7, 0, 4, 0, 1});
    tick(6);
    USBDevice.suspended_ = true;
    runtime().loop();
    USBDevice.suspended_ = false;
    tick(15);
    assert(runtime().engine().sessionCount() == 0 && !runtime().engine().completedState().held());
    fresh();
    open();
    send(binary::Key, 2, {7, 0, 4, 0, 1});
    tick(6);
    ++test_reset_generation;
    for (auto &b : ep_busy)
        b = 0;
    runtime().loop();
    tick(12);
    assert(!runtime().engine().sessionCount() && !runtime().engine().completedState().held() &&
           replied(42, 127, 2, 7));
    // A reset which frees an unacknowledged vendor bank cannot complete BOOTLOADER.
    fresh();
    open();
    send(binary::Bootloader, 2);
    for (unsigned n = 0; n < 12 && !replied(42, 134, 2); ++n)
        tick();
    assert(replied(42, 134, 2));
    ++test_reset_generation;
    for (auto &b : ep_busy)
        b = 0;
    assert(!runtime().transport().vendorHid().completed());
    runtime().loop();
    tick(12);
    assert(!runtime().bootloader().pending() && !runtime().engine().sessionCount());
    fresh();
    open();
    send(binary::Bootloader, 2);
    for (unsigned n = 0; n < 12 && !replied(42, 134, 2); ++n)
        tick();
    assert(replied(42, 134, 2) && !runtime().bootloader().pending());
    tick();
    assert(runtime().bootloader().pending());
    fresh();
    open();
    uint8_t old[64] = {10, 'd', ':', 'a'};
    runtime().transport().receive(0, HID_REPORT_TYPE_OUTPUT, old, 64);
    tick(12);
    assert(!runtime().engine().sessionCount() && !runtime().engine().completedState().held() &&
           replied(42, 127, 1, 1));
    testGoldenUsbAndControlOut();
    testRejectedSendsAndRxFaults();
    testBootDeadlineAndResetAfterAck();
    testResetBeforeNewOutPacket();
    puts("Leonardo v3: real firmware, packet ACKs/no ZLP, capabilities/descriptors, rapid "
         "taps/barriers, eight sessions, isolated release/status, overflow broadcast, suspend and "
         "boot ACK/deadline/reset, golden USB bytes, control OUT, rejected sends and RX/TX faults "
         "passed");
}
