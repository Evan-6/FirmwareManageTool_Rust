#pragma once
#include "../input/InputEngine.h"
#include "../platform/UsbTransport.h"
#include "../platform/Bootloader.h"
namespace hidfw {
class FirmwareRuntime {
  public:
    void begin();
    void loop();
    const InputEngine &engine() const { return engine_; }
    UsbTransport &transport() { return usb_; }
    const Bootloader &bootloader() const { return boot_; }

  private:
    InputEngine engine_{};
    UsbTransport usb_{};
    Bootloader boot_{};
    uint32_t rx_generation_ = 0;
    void discardFaultedRx(uint32_t now);
};
} // namespace hidfw
