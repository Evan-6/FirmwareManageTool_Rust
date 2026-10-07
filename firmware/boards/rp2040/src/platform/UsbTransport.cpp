#include "UsbTransport.h"
#include "BoardConfig.h"
#include "UsbDescriptors.h"
#include <Arduino.h>
#if !defined(USE_TINYUSB)
#error "Select usbstack=tinyusb"
#endif
namespace hidfw {
UsbTransport::UsbTransport()
    : input_(board::HidReportDescriptor, board::HidReportDescriptorLength, HID_ITF_PROTOCOL_NONE, 1,
             false),
      vendor_(board::VendorHidReportDescriptor, board::VendorHidReportDescriptorLength,
              HID_ITF_PROTOCOL_NONE, 1, true) {}
UsbTransport::~UsbTransport() {
    unbindTransport(this);
    if (initialized_)
        queue_free(&rx_);
}
void UsbTransport::begin(const wire::DeviceInfo &info) {
    if (initialized_)
        return;
    wire::feature(info, feature_);
    queue_init(&rx_, sizeof(wire::Report), 32);
    initialized_ = true;
    bindTransport(this);
    if (!TinyUSBDevice.isInitialized())
        TinyUSBDevice.begin(0);
    Serial.end();
    TinyUSBDevice.setID(board::UsbVid, board::UsbPid);
    TinyUSBDevice.setDeviceVersion(0x0310);
    constexpr char hex[] = "0123456789ABCDEF";
    for (uint8_t i = 0; i < 8; ++i) {
        serial_[2 * i] = hex[info.id[i] >> 4];
        serial_[2 * i + 1] = hex[info.id[i] & 15];
    }
    serial_[16] = 0;
    TinyUSBDevice.setSerialDescriptor(serial_);
    TinyUSBDevice.setManufacturerDescriptor(board::UsbManufacturer);
    TinyUSBDevice.setProductDescriptor(board::UsbProduct);
    // Callback functions are registered by bindTransport; the C++ wrappers are
    // defined in UsbCallbacks.cpp to keep callbacks separate from device ownership.
    vendor_.setReportCallback(vendorFeature, vendorReceive);
    input_.begin();
    vendor_.begin();
    if (TinyUSBDevice.mounted()) {
        TinyUSBDevice.detach();
        delay(10);
        TinyUSBDevice.attach();
    }
}
bool UsbTransport::mounted() const { return TinyUSBDevice.mounted(); }
bool UsbTransport::inputReady() { return input_.ready(); }
bool UsbTransport::vendorReady() { return vendor_.ready(); }
bool UsbTransport::sendInput(const InputReport &r) {
    if (reset_pending_.load())
        return false;
    input_epoch_ = usb_epoch_.load();
    input_pending_.store(r.id);
    if (input_.sendReport(r.id, r.data, r.length))
        return true;
    input_pending_.store(0);
    return false;
}
bool UsbTransport::sendVendor(const wire::Report &r, uint32_t generation) {
    if (reset_pending_.load())
        return false;
    vendor_epoch_ = usb_epoch_.load();
    vendor_generation_ = generation;
    vendor_session_ = wire::read32(r.bytes + 4);
    vendor_sequence_ = wire::read32(r.bytes + 8);
    vendor_boot_.store(r.bytes[1] == (wire::Bootloader | 128) && r.bytes[12] == 0);
    if (vendor_.sendReport(13, r.bytes, 63))
        return true;
    vendor_boot_.store(false);
    return false;
}
bool UsbTransport::read(wire::Report &r) { return queue_try_remove(&rx_, &r); }
void UsbTransport::discardRx() {
    while (queue_try_remove(&rx_, nullptr)) {
    }
}
void UsbTransport::receiveError(uint32_t flag) {
    auto value = rx_fault_.load();
    while (!rx_fault_.compare_exchange_weak(value,
                                            ((value + 1) & 0xffff) | (value & 0xffff0000) | flag)) {
    }
}
void UsbTransport::receive(uint8_t id, hid_report_type_t type, const uint8_t *data,
                           uint16_t length) {
    if (!initialized_)
        return;
    if (!id && length) {
        id = *data++;
        --length;
    }
    if (id != 12 || length != 63 ||
        (type != HID_REPORT_TYPE_OUTPUT && type != HID_REPORT_TYPE_INVALID)) {
        receiveError(1u << 16);
        return;
    }
    if (!queue_try_add(&rx_, data))
        receiveError(1u << 17);
}
uint16_t UsbTransport::feature(uint8_t id, hid_report_type_t type, uint8_t *data,
                               uint16_t length) const {
    if (id != 14 || type != HID_REPORT_TYPE_FEATURE || length < 63)
        return 0;
    memcpy(data, feature_, 63);
    return 63;
}
void UsbTransport::completed(uint8_t instance, const uint8_t *report, uint16_t length) {
    if (!length)
        return;
    if (instance == 0 && input_epoch_ == usb_epoch_.load()) {
        auto expected = report[0];
        if (input_pending_.compare_exchange_strong(expected, 0))
            input_complete_.store(report[0]);
    } else if (instance == 1 && length == 64 && vendor_epoch_ == usb_epoch_.load() &&
               report[0] == 13 && report[1] == 3 && report[2] == (wire::Bootloader | 128) &&
               report[13] == 0 && wire::read32(report + 5) == vendor_session_ &&
               wire::read32(report + 9) == vendor_sequence_ && vendor_boot_.exchange(false)) {
        completed_generation_.store(vendor_generation_.load());
        vendor_complete_.store(true);
    }
}
void UsbTransport::failed(uint8_t instance) {
    if (instance == 0) {
        input_pending_.store(0);
        input_failed_.store(true);
    }
}
bool UsbTransport::takeVendorComplete(uint32_t &generation) {
    if (!vendor_complete_.exchange(false))
        return false;
    generation = completed_generation_.load();
    return true;
}
void UsbTransport::reset() {
    usb_epoch_.fetch_add(1);
    input_pending_.store(0);
    input_complete_.store(0);
    vendor_boot_.store(false);
    vendor_complete_.store(false);
    reset_pending_.store(true);
}
} // namespace hidfw
