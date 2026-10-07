#pragma once
#include "V3Descriptors.h"
uint16_t vendorFeature(uint8_t id, hid_report_type_t type, uint8_t *data, uint16_t length);
void vendorSetReport(uint8_t id, hid_report_type_t type, const uint8_t *data, uint16_t length);
// One packet per endpoint is in flight. Completion means its bank was ACKed by
// the host, not just that there is space in the second hardware bank.
class ReportHid : public PluggableUSBModule {
    uint8_t ep_type_[2];
    bool vendor_;
    bool pending_ = false;
    uint8_t pending_id_ = 0;

  public:
    explicit ReportHid(bool vendor)
        : PluggableUSBModule(vendor ? 2 : 1, 1, ep_type_), vendor_(vendor) {
        ep_type_[0] = EP_TYPE_INTERRUPT_IN;
        ep_type_[1] = EP_TYPE_INTERRUPT_OUT;
        PluggableUSB().plug(this);
    }
    void reset() { pending_ = false; }
    bool ready() {
        return usb_generation == USB_ResetGeneration() && !pending_ &&
               USB_PacketComplete(pluggedEndpoint);
    }
    bool sendReport(uint8_t id, const uint8_t *data, uint8_t length) {
        if (length > 63 || !ready())
            return false;
        uint8_t report[64];
        report[0] = id;
        memcpy(report + 1, data, length);
        bool sent = false;
        // A reset cannot invalidate the generation between checking and submitting
        // a packet. The core's endpoint lock nests safely inside this AVR block.
        ATOMIC_BLOCK(ATOMIC_RESTORESTATE) {
            if (usb_generation == USB_ResetGeneration() && !pending_ &&
                USB_TrySendPacket(pluggedEndpoint, report, length + 1)) {
                pending_ = true;
                pending_id_ = id;
                sent = true;
            }
        }
        return sent;
    }
    uint8_t completed() {
        if (!USBDevice.configured() || usb_generation != USB_ResetGeneration()) {
            pending_ = false;
            return 0;
        }
        if (!pending_ || !USB_PacketComplete(pluggedEndpoint))
            return 0;
        // A cleared bank caused by USB reset is not a host ACK.
        if (usb_generation != USB_ResetGeneration()) {
            pending_ = false;
            return 0;
        }
        pending_ = false;
        return pending_id_;
    }
    void serviceRx() {
        if (!vendor_ || !USBDevice.configured())
            return;
        const uint8_t ep = pluggedEndpoint + 1;
        // Process one packet per loop so the small RX queue can be drained.
        if (USB_Available(ep)) {
            uint8_t report[64];
            const int n = USB_Recv(ep, report, sizeof(report));
            if (n > 0)
                vendorSetReport(0, HID_REPORT_TYPE_OUTPUT, report, n);
        }
    }

  protected:
    int getInterface(uint8_t *count) override {
        ++*count;
        struct Descriptor {
            InterfaceDescriptor interface;
            HIDDescDescriptor hid;
            EndpointDescriptor in;
            EndpointDescriptor out;
        };
        Descriptor d = {
            D_INTERFACE(pluggedInterface, static_cast<uint8_t>(vendor_ ? 2 : 1),
                        USB_DEVICE_CLASS_HUMAN_INTERFACE, HID_SUBCLASS_NONE, HID_PROTOCOL_NONE),
            D_HIDREPORT(vendor_ ? sizeof(VendorHidReportDescriptor) : sizeof(HidReportDescriptor)),
            D_ENDPOINT(USB_ENDPOINT_IN(pluggedEndpoint), USB_ENDPOINT_TYPE_INTERRUPT, 64, 1),
            D_ENDPOINT(USB_ENDPOINT_OUT(pluggedEndpoint + 1), USB_ENDPOINT_TYPE_INTERRUPT, 64, 1)};
        return USB_SendControl(0, &d, vendor_ ? sizeof(d) : sizeof(d) - sizeof(d.out));
    }
    int getDescriptor(USBSetup &s) override {
        if (s.bmRequestType != REQUEST_DEVICETOHOST_STANDARD_INTERFACE ||
            s.wValueH != HID_REPORT_DESCRIPTOR_TYPE || s.wIndex != pluggedInterface)
            return 0;
        return vendor_ ? USB_SendControl(TRANSFER_PGM, VendorHidReportDescriptor,
                                         sizeof(VendorHidReportDescriptor))
                       : USB_SendControl(TRANSFER_PGM, HidReportDescriptor,
                                         sizeof(HidReportDescriptor));
    }
    bool setup(USBSetup &s) override {
        if (s.wIndex != pluggedInterface)
            return false;
        if (s.bmRequestType == REQUEST_DEVICETOHOST_CLASS_INTERFACE) {
            if (vendor_ && s.bRequest == HID_GET_REPORT && s.wValueH == HID_REPORT_TYPE_FEATURE &&
                s.wValueL == 14 && s.wLength == 64) {
                uint8_t report[64];
                report[0] = 14;
                if (vendorFeature(14, HID_REPORT_TYPE_FEATURE, report + 1, 63) != 63)
                    return false;
                return USB_SendControl(0, report, sizeof(report)) == sizeof(report);
            }
            if (s.bRequest == HID_GET_IDLE || s.bRequest == HID_GET_PROTOCOL) {
                const uint8_t value = s.bRequest == HID_GET_PROTOCOL ? 1 : 0;
                USB_SendControl(0, &value, 1);
                return true;
            }
        }
        if (s.bmRequestType == REQUEST_HOSTTODEVICE_CLASS_INTERFACE) {
            if (s.bRequest == HID_SET_IDLE)
                return true;
            if (s.bRequest == HID_SET_PROTOCOL)
                return s.wValueL == 1;
            if (s.bRequest == HID_SET_REPORT && s.wValueH == HID_REPORT_TYPE_OUTPUT) {
                if (vendor_ && s.wValueL == 12 && s.wLength == 64) {
                    uint8_t report[64];
                    const int n = USB_RecvControl(report, sizeof(report));
                    if (n != 64 || report[0] != 12)
                        return false;
                    vendorSetReport(0, HID_REPORT_TYPE_OUTPUT, report, n);
                    return true;
                }
                if (!vendor_ && s.wValueL == KeyboardReportId && s.wLength == 2) {
                    uint8_t leds[2];
                    return USB_RecvControl(leds, sizeof(leds)) == 2 && leds[0] == KeyboardReportId;
                }
            }
        }
        return false;
    }
};
ReportHid usb_hid(false), usb_vendor(true);
struct DeviceFacade {
    bool mounted() { return USBDevice.configured() && !USBDevice.isSuspended(); }
} TinyUSBDevice;
