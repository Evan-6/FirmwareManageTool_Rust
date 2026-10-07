#pragma once
#include "../input/OutputScheduler.h"
#include <Arduino.h>
#include <HID.h>
namespace hidfw {
class UsbTransport;
class ReportHid : public PluggableUSBModule {
  public:
    ReportHid(bool vendor, UsbTransport &owner);
    void reset();
    bool ready();
    bool sendReport(uint8_t id, const uint8_t *data, uint8_t length);
    uint8_t completed();
    void serviceRx();

  protected:
    int getInterface(uint8_t *count) override;
    int getDescriptor(USBSetup &setup) override;
    bool setup(USBSetup &setup) override;

  private:
    uint8_t ep_type_[2];
    UsbTransport &owner_;
    bool vendor_, pending_ = false;
    uint8_t pending_id_ = 0;
};
struct RxFault {
    uint16_t errors;
    uint8_t code;
};
class UsbTransport {
  public:
    UsbTransport();
    UsbTransport(const UsbTransport &) = delete;
    UsbTransport &operator=(const UsbTransport &) = delete;
    void begin();
    bool takeReset();
    bool generationMatches() const;
    bool mounted() const;
    bool inputReady();
    bool vendorReady();
    uint8_t inputCompleted();
    bool vendorCompleted(uint32_t &generation);
    void serviceRx();
    bool read(wire::Report &report);
    void discardRx();
    RxFault takeRxFault();
    bool sendInput(const InputReport &report);
    bool sendVendor(const wire::Report &report, uint32_t generation);
    void receive(uint8_t id, uint8_t type, const uint8_t *data, uint16_t length);
    uint16_t feature(uint8_t id, uint8_t type, uint8_t *data, uint16_t length) const;
    ReportHid &inputHid() { return input_; }
    ReportHid &vendorHid() { return vendor_; }

  private:
    ReportHid input_, vendor_;
    wire::Report rx_[config::RxDepth]{};
    uint8_t rx_head_ = 0;
    volatile uint8_t rx_count_ = 0, rx_flags_ = 0;
    volatile uint16_t rx_errors_ = 0;
    uint8_t usb_generation_ = 0;
    bool vendor_boot_ = false;
    uint32_t vendor_generation_ = 0;
};
} // namespace hidfw
