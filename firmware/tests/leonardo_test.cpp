// Real AVR firmware and nonblocking core USB helpers with fake endpoint ACKs.
#include <assert.h>
#include <new>
#include <stdio.h>
unsigned long test_time = 0;
#include "../boards/leonardo_avr/Firmware.cpp"
void tick(unsigned n = 1) {
    for (unsigned i = 0; i < n; ++i) {
        for (auto &p : test_packets)
            if (!p.delivered) {
                p.delivered = true;
                ep_busy[p.ep] = 0;
            }
        ++test_time;
        Firmware::loop();
    }
}
void fresh() {
    USBDevice.configured_ = false;
    usb_hid.completed();
    usb_vendor.completed();
    output.~OutputScheduler();
    new (&output) OutputScheduler;
    binary::resetSessions();
    binary::rx = {};
    binary::tx = {};
    binary::overflow = false;
    binary::invalid_report = false;
    binary::boot_requested = false;
    binary::boot_reply_inflight = false;
    binary::boot_reply_completed = false;
    bootloader_reset_pending = false;
    protocol_owner = ProtocolOwner::None;
    for (auto &b : ep_busy)
        b = 0;
    test_packets.clear();
    test_out.clear();
    USBDevice.configured_ = true;
    USBDevice.suspended_ = false;
    Firmware::setup();
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
    Firmware::loop();
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
    assert(binary::find(session));
    assert(replied(session, 129, 1));
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
    assert(usb_vendor.testSetup(feature));
    assert(control_out.size() == 64 && control_out[0] == 14);
    assert(memcmp(control_out.data() + 1, "FMT3", 4) == 0 && control_out[6] == 3 &&
           control_out[7] == 3 && control_out[8] == 1);
    assert(binary::read32(control_out.data() + 15) == 23 && control_out[12] == 0 &&
           control_out[13] == 0);
    uint8_t interfaces = 0;
    assert(usb_hid.testInterface(&interfaces) == 25);
    assert(control_out[4] == 1);
    assert(usb_vendor.testInterface(&interfaces) == 32 && interfaces == 2 && control_out[4] == 2);
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
    assert(downs == 1 && ups >= 1 && !output.completed.held());
    // Eight owners, overlapping keys/buttons, and own release preserves a peer.
    for (uint32_t id = 43; id < 50; ++id)
        open(id);
    assert(binary::count() == 8);
    send(binary::Open, 1, {}, 99);
    tick(4);
    assert(replied(99, 129, 1, 2) && binary::count() == 8);
    send(binary::Key, 5, {7, 0, 4, 0, 1});
    tick(6);
    send(binary::Buttons, 2, {3}, 43);
    tick(6);
    send(binary::Release, 6);
    tick(12);
    assert(replied(42, 133, 6) && !output.completed.keys[0] && output.completed.buttons == 3);
    send(binary::Bootloader, 7);
    tick(4);
    assert(replied(42, 134, 7, 2) && !bootloader_reset_pending);
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
    assert(binary::count() == 0 && !output.completed.held());
    for (uint32_t id = 42; id < 50; ++id)
        assert(replied(id, 127, id == 42 ? 11 : (id == 43 ? 3 : 1), 8));
    // Eight idle leases can expire together without filling the two-slot output queue.
    fresh();
    for (uint32_t id = 42; id < 50; ++id)
        open(id);
    test_packets.clear();
    test_time += 2001;
    Firmware::loop();
    tick(25);
    assert(!binary::count() && !output.completed.held());
    for (uint32_t id = 42; id < 50; ++id)
        assert(replied(id, 127, 1, 5));
    fresh();
    open();
    send(binary::Key, 2, {7, 0, 4, 0, 1});
    tick(6);
    USBDevice.suspended_ = true;
    Firmware::loop();
    USBDevice.suspended_ = false;
    tick(15);
    assert(binary::count() == 0 && !output.completed.held());
    fresh();
    open();
    send(binary::Key, 2, {7, 0, 4, 0, 1});
    tick(6);
    ++test_reset_generation;
    for (auto &b : ep_busy)
        b = 0;
    Firmware::loop();
    tick(12);
    assert(!binary::count() && !output.completed.held() && replied(42, 127, 2, 7));
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
    assert(!usb_vendor.completed());
    Firmware::loop();
    tick(12);
    assert(!bootloader_reset_pending && !binary::count());
    fresh();
    open();
    send(binary::Bootloader, 2);
    for (unsigned n = 0; n < 12 && !replied(42, 134, 2); ++n)
        tick();
    assert(replied(42, 134, 2) && !bootloader_reset_pending);
    tick();
    assert(bootloader_reset_pending);
    fresh();
    open();
    uint8_t old[64] = {10, 'd', ':', 'a'};
    vendorSetReport(0, HID_REPORT_TYPE_OUTPUT, old, 64);
    tick(12);
    assert(!binary::count() && !output.completed.held() && replied(42, 127, 1, 1));
    puts("Leonardo v3: real firmware, packet ACKs/no ZLP, capabilities/descriptors, rapid "
         "taps/barriers, eight sessions, isolated release/status, overflow broadcast, suspend and "
         "boot completion passed");
}
