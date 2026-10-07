#pragma once
#include "Arduino.h"
using u8 = uint8_t;
#define HID_REPORT_TYPE_INPUT 1
#define HID_REPORT_TYPE_OUTPUT 2
#define HID_REPORT_TYPE_FEATURE 3
#define REQUEST_DEVICETOHOST_STANDARD_INTERFACE 0x81
#define REQUEST_DEVICETOHOST_CLASS_INTERFACE 0xA1
#define REQUEST_HOSTTODEVICE_CLASS_INTERFACE 0x21
#define HID_REPORT_DESCRIPTOR_TYPE 0x22
#define HID_GET_REPORT 1
#define HID_GET_IDLE 2
#define HID_GET_PROTOCOL 3
#define HID_SET_REPORT 9
#define HID_SET_IDLE 10
#define HID_SET_PROTOCOL 11
#define EP_TYPE_INTERRUPT_IN 1
#define EP_TYPE_INTERRUPT_OUT 2
#define USB_DEVICE_CLASS_HUMAN_INTERFACE 3
#define HID_SUBCLASS_NONE 0
#define HID_PROTOCOL_NONE 0
#define USB_ENDPOINT_TYPE_INTERRUPT 3
#define USB_ENDPOINT_IN(n) ((n) | 128)
#define USB_ENDPOINT_OUT(n) (n)
#define TRANSFER_PGM 128
struct USBSetup {
    uint8_t bmRequestType = 0, bRequest = 0, wValueL = 0, wValueH = 0;
    uint16_t wIndex = 0, wLength = 0;
};
struct InterfaceDescriptor {
    uint8_t bytes[9];
};
struct HIDDescDescriptor {
    uint8_t bytes[9];
};
struct EndpointDescriptor {
    uint8_t bytes[7];
};
#define D_INTERFACE(n, e, c, s, p)                                                                 \
    {                                                                                              \
        { 9, 4, n, 0, e, c, s, p, 0 }                                                              \
    }
#define D_HIDREPORT(n)                                                                             \
    {                                                                                              \
        { 9, 0x21, 1, 1, 0, 1, 0x22, uint8_t(n), uint8_t((n) >> 8) }                               \
    }
#define D_ENDPOINT(n, t, s, i)                                                                     \
    {                                                                                              \
        { 7, 5, uint8_t(n), t, uint8_t(s), 0, i }                                                  \
    }
class PluggableUSBModule {
  public:
    PluggableUSBModule(uint8_t e, uint8_t, uint8_t *) : endpoints(e) {}
    uint8_t endpoints;
    bool testSetup(USBSetup &s) { return setup(s); }
    int testInterface(uint8_t *n) { return getInterface(n); }
    int testDescriptor(USBSetup &s) { return getDescriptor(s); }

  protected:
    uint8_t pluggedInterface = 0, pluggedEndpoint = 0;
    virtual bool setup(USBSetup &) = 0;
    virtual int getInterface(uint8_t *) = 0;
    virtual int getDescriptor(USBSetup &) = 0;
    friend struct Pluggable;
};
struct Pluggable {
    uint8_t next_if = 0, next_ep = 1;
    bool plug(PluggableUSBModule *m) {
        m->pluggedInterface = next_if++;
        m->pluggedEndpoint = next_ep;
        next_ep += m->endpoints;
        return true;
    }
};
inline Pluggable &PluggableUSB() {
    static Pluggable p;
    return p;
}
struct UsbPacket {
    uint8_t ep;
    std::vector<uint8_t> bytes;
    bool delivered = false;
};
inline std::vector<UsbPacket> test_packets;
inline std::deque<std::vector<uint8_t>> test_out;
inline std::vector<uint8_t> control_in, control_out;
inline uint8_t ep_busy[4]{}, selected_ep = 0, UESTA0X = 0, _usbConfiguration = 1,
                             _usbSuspendState = 0;
inline std::vector<uint8_t> fifo[4];
inline uint8_t test_reset_generation = 0;
inline uint8_t USB_ResetGeneration() { return test_reset_generation; }
#define NBUSYBK0 0
#define NBUSYBK1 1
#define SUSPI 0
struct LockEP {
    explicit LockEP(uint8_t ep) {
        selected_ep = ep;
        UESTA0X = ep_busy[ep];
    }
};
inline uint8_t FifoByteCount() { return fifo[selected_ep].size(); }
inline bool ReadWriteAllowed() { return fifo[selected_ep].size() < 64; }
inline void Send8(uint8_t v) { fifo[selected_ep].push_back(v); }
inline void ReleaseTX() {
    test_packets.push_back({selected_ep, fifo[selected_ep], false});
    fifo[selected_ep].clear();
    ep_busy[selected_ep] = 1;
}
// Compile the actual bundled AVR core packet helpers, using fake endpoint registers.
#include "../../boards/leonardo_avr/hardware/goosedevil/avr/cores/arduino/HidPackets.h"
inline uint8_t USB_Available(uint8_t) { return test_out.empty() ? 0 : test_out.front().size(); }
inline int USB_Recv(uint8_t, void *d, int n) {
    if (test_out.empty())
        return 0;
    const int len = test_out.front().size();
    if (len > n)
        return -1;
    memcpy(d, test_out.front().data(), len);
    test_out.pop_front();
    return len;
}
inline int USB_SendControl(uint8_t, const void *d, int n) {
    auto p = static_cast<const uint8_t *>(d);
    control_out.assign(p, p + n);
    return n;
}
inline int USB_RecvControl(void *d, int n) {
    if (control_in.size() != static_cast<size_t>(n))
        return -1;
    memcpy(d, control_in.data(), n);
    return n;
}
