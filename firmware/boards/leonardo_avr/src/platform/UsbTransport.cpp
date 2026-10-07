#include "UsbTransport.h"
#include "UsbDescriptors.h"
#include <util/atomic.h>
#if !defined(USBCON) || !defined(CDC_DISABLED)
#error "Leonardo v3 requires the bundled native USB core with CDC_DISABLED."
#endif
namespace hidfw {
using namespace wire;
ReportHid::ReportHid(bool vendor, UsbTransport &owner)
    : PluggableUSBModule(vendor ? 2 : 1, 1, ep_type_), owner_(owner), vendor_(vendor) {
    ep_type_[0] = EP_TYPE_INTERRUPT_IN;
    ep_type_[1] = EP_TYPE_INTERRUPT_OUT;
    PluggableUSB().plug(this);
}
void ReportHid::reset() { pending_ = false; }
bool ReportHid::ready() {
    return owner_.generationMatches() && !pending_ && USB_PacketComplete(pluggedEndpoint);
}
bool ReportHid::sendReport(uint8_t id, const uint8_t *data, uint8_t length) {
    if (length > 63 || !ready())
        return false;
    uint8_t report[64];
    report[0] = id;
    memcpy(report + 1, data, length);
    bool sent = false;
    ATOMIC_BLOCK(ATOMIC_RESTORESTATE) {
        if (owner_.generationMatches() && !pending_ &&
            USB_TrySendPacket(pluggedEndpoint, report, length + 1)) {
            pending_ = true;
            pending_id_ = id;
            sent = true;
        }
    }
    return sent;
}
uint8_t ReportHid::completed() {
    if (!USBDevice.configured() || !owner_.generationMatches()) {
        pending_ = false;
        return 0;
    }
    if (!pending_ || !USB_PacketComplete(pluggedEndpoint))
        return 0;
    if (!owner_.generationMatches()) {
        pending_ = false;
        return 0;
    }
    pending_ = false;
    return pending_id_;
}
void ReportHid::serviceRx() {
    if (!vendor_ || !USBDevice.configured())
        return;
    const uint8_t ep = pluggedEndpoint + 1;
    if (USB_Available(ep)) {
        uint8_t report[64];
        const int n = USB_Recv(ep, report, sizeof(report));
        if (n > 0)
            owner_.receive(0, HID_REPORT_TYPE_OUTPUT, report, n);
    }
}
int ReportHid::getInterface(uint8_t *count) {
    ++*count;
    struct Descriptor {
        InterfaceDescriptor interface;
        HIDDescDescriptor hid;
        EndpointDescriptor in, out;
    };
    Descriptor d = {
        D_INTERFACE(pluggedInterface, static_cast<uint8_t>(vendor_ ? 2 : 1),
                    USB_DEVICE_CLASS_HUMAN_INTERFACE, HID_SUBCLASS_NONE, HID_PROTOCOL_NONE),
        D_HIDREPORT(vendor_ ? board::VendorHidReportDescriptorLength
                            : board::HidReportDescriptorLength),
        D_ENDPOINT(USB_ENDPOINT_IN(pluggedEndpoint), USB_ENDPOINT_TYPE_INTERRUPT, 64, 1),
        D_ENDPOINT(USB_ENDPOINT_OUT(pluggedEndpoint + 1), USB_ENDPOINT_TYPE_INTERRUPT, 64, 1)};
    return USB_SendControl(0, &d, vendor_ ? sizeof(d) : sizeof(d) - sizeof(d.out));
}
int ReportHid::getDescriptor(USBSetup &s) {
    if (s.bmRequestType != REQUEST_DEVICETOHOST_STANDARD_INTERFACE ||
        s.wValueH != HID_REPORT_DESCRIPTOR_TYPE || s.wIndex != pluggedInterface)
        return 0;
    return vendor_ ? USB_SendControl(TRANSFER_PGM, board::VendorHidReportDescriptor,
                                     board::VendorHidReportDescriptorLength)
                   : USB_SendControl(TRANSFER_PGM, board::HidReportDescriptor,
                                     board::HidReportDescriptorLength);
}
bool ReportHid::setup(USBSetup &s) {
    if (s.wIndex != pluggedInterface)
        return false;
    if (s.bmRequestType == REQUEST_DEVICETOHOST_CLASS_INTERFACE) {
        if (vendor_ && s.bRequest == HID_GET_REPORT && s.wValueH == HID_REPORT_TYPE_FEATURE &&
            s.wValueL == 14 && s.wLength == 64) {
            uint8_t report[64];
            report[0] = 14;
            if (owner_.feature(14, HID_REPORT_TYPE_FEATURE, report + 1, 63) != 63)
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
                owner_.receive(0, HID_REPORT_TYPE_OUTPUT, report, n);
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
UsbTransport::UsbTransport() : input_(false, *this), vendor_(true, *this) {}
void UsbTransport::begin() { usb_generation_ = USB_ResetGeneration(); }
bool UsbTransport::generationMatches() const { return usb_generation_ == USB_ResetGeneration(); }
bool UsbTransport::takeReset() {
    if (generationMatches())
        return false;
    usb_generation_ = USB_ResetGeneration();
    input_.reset();
    vendor_.reset();
    vendor_boot_ = false;
    return true;
}
bool UsbTransport::mounted() const { return USBDevice.configured() && !USBDevice.isSuspended(); }
bool UsbTransport::inputReady() { return input_.ready(); }
bool UsbTransport::vendorReady() { return vendor_.ready(); }
uint8_t UsbTransport::inputCompleted() { return input_.completed(); }
bool UsbTransport::vendorCompleted(uint32_t &generation) {
    if (!vendor_.completed())
        return false;
    const bool boot = vendor_boot_;
    vendor_boot_ = false;
    if (boot)
        generation = vendor_generation_;
    return boot;
}
void UsbTransport::serviceRx() { vendor_.serviceRx(); }
void UsbTransport::receive(uint8_t id, uint8_t type, const uint8_t *data, uint16_t length) {
    if (!id && length) {
        id = *data++;
        --length;
    }
    if (id != 12 || length != 63 || (type != HID_REPORT_TYPE_OUTPUT && type != 0)) {
        ATOMIC_BLOCK(ATOMIC_RESTORESTATE) {
            ++rx_errors_;
            rx_flags_ |= 1;
        }
        return;
    }
    ATOMIC_BLOCK(ATOMIC_RESTORESTATE) {
        if (rx_count_ == config::RxDepth) {
            ++rx_errors_;
            rx_flags_ |= 2;
        } else {
            memcpy(rx_[(rx_head_ + rx_count_) % config::RxDepth].bytes, data, 63);
            ++rx_count_;
        }
    }
}
bool UsbTransport::read(wire::Report &report) {
    bool ok = false;
    ATOMIC_BLOCK(ATOMIC_RESTORESTATE) {
        if (rx_count_) {
            report = rx_[rx_head_];
            rx_head_ = (rx_head_ + 1) % config::RxDepth;
            --rx_count_;
            ok = true;
        }
    }
    return ok;
}
void UsbTransport::discardRx() {
    ATOMIC_BLOCK(ATOMIC_RESTORESTATE) {
        rx_count_ = 0;
        rx_head_ = 0;
    }
}
RxFault UsbTransport::takeRxFault() {
    RxFault result{};
    ATOMIC_BLOCK(ATOMIC_RESTORESTATE) {
        result.errors = rx_errors_;
        result.code = (rx_flags_ & 1) ? 1 : (rx_flags_ & 2) ? 8 : 0;
        rx_errors_ = 0;
        rx_flags_ = 0;
    }
    return result;
}
bool UsbTransport::sendInput(const InputReport &r) {
    return input_.sendReport(r.id, r.data, r.length);
}
bool UsbTransport::sendVendor(const wire::Report &r, uint32_t generation) {
    if (!vendor_.sendReport(13, r.bytes, 63))
        return false;
    vendor_boot_ = r.bytes[1] == (Bootloader | 128) && r.bytes[12] == 0;
    vendor_generation_ = generation;
    return true;
}
uint16_t UsbTransport::feature(uint8_t id, uint8_t type, uint8_t *data, uint16_t length) const {
    if (id != 14 || type != HID_REPORT_TYPE_FEATURE || length < 63)
        return 0;
    wire::DeviceInfo info;
    info.board = 3;
    const uint8_t fixed[8] = {'H', 'P', 'K', 'B', 0, 0, 0x24, 3};
    memcpy(info.id, fixed, 8);
    wire::feature(info, data);
    return 63;
}
} // namespace hidfw
