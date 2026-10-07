#pragma once
#include "../input/BootloaderGate.h"
namespace hidfw {
class Bootloader {
  public:
    void update(uint32_t now, bool confirmed, bool output_idle, bool vendor_ready, bool tx_empty);
    bool pending() const { return gate_.pending(); }

  private:
    BootloaderGate gate_{};
};
namespace avr {
void enterBootloader();
}
} // namespace hidfw
