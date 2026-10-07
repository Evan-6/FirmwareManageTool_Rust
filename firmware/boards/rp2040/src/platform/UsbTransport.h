#pragma once
#include "../input/OutputScheduler.h"
#include <Adafruit_TinyUSB.h>
#include <atomic>
#include <pico/util/queue.h>
namespace hidfw {
class UsbTransport {
  public:
    UsbTransport();
    ~UsbTransport();
    UsbTransport(const UsbTransport &) = delete;
    UsbTransport &operator=(const UsbTransport &) = delete;
    void begin(const wire::DeviceInfo &info);
    bool mounted() const;
    bool inputReady();
    bool vendorReady();
    bool sendInput(const InputReport &report);
    bool sendVendor(const wire::Report &report, uint32_t boot_generation);
    bool read(wire::Report &report);
    void discardRx();
    uint32_t takeRxFault() { return rx_fault_.exchange(0); }
    uint8_t takeInputComplete() { return input_complete_.exchange(0); }
    bool takeInputFailure() { return input_failed_.exchange(false); }
    bool takeVendorComplete(uint32_t &generation);
    bool takeReset() { return reset_pending_.exchange(false); }
    void receive(uint8_t id, hid_report_type_t type, const uint8_t *data, uint16_t length);
    uint16_t feature(uint8_t id, hid_report_type_t type, uint8_t *data, uint16_t length) const;
    void completed(uint8_t instance, const uint8_t *report, uint16_t length);
    void failed(uint8_t instance);
    void reset();

  private:
    Adafruit_USBD_HID input_, vendor_;
    queue_t rx_{};
    uint8_t feature_[63]{};
    char serial_[17]{};
    bool initialized_ = false;
    std::atomic<uint32_t> rx_fault_{0}, usb_epoch_{0}, completed_generation_{0};
    std::atomic<uint8_t> input_pending_{0}, input_complete_{0};
    std::atomic<bool> input_failed_{false}, vendor_boot_{false}, vendor_complete_{false},
        reset_pending_{false};
    std::atomic<uint32_t> input_epoch_{0}, vendor_epoch_{0}, vendor_generation_{0};
    std::atomic<uint32_t> vendor_session_{0}, vendor_sequence_{0};
    void receiveError(uint32_t flag);
};
void bindTransport(UsbTransport *transport);
void unbindTransport(UsbTransport *transport);
uint16_t vendorFeature(uint8_t id, hid_report_type_t type, uint8_t *data, uint16_t length);
void vendorReceive(uint8_t id, hid_report_type_t type, const uint8_t *data, uint16_t length);
} // namespace hidfw
