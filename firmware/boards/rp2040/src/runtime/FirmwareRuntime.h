#pragma once
#include "../input/InputEngine.h"
#include "../platform/UsbTransport.h"
#include "../platform/Bootloader.h"
#include "../platform/StatusLedRenderer.h"
namespace hidfw {
class FirmwareRuntime {
  public:
    void begin();
    void loop();
    void beginLed();
    void renderLed();
    const InputEngine &engine() const { return engine_; }
    UsbTransport &transport() { return usb_; }
    const Bootloader &bootloader() const { return boot_; }

  private:
    InputEngine engine_{};
    UsbTransport usb_{};
    Bootloader boot_{};
    StatusIndicator indicator_{};
    StatusLedRenderer renderer_{};
    uint32_t rx_generation_ = 0;
    void discardFaultedRx();
};
} // namespace hidfw
