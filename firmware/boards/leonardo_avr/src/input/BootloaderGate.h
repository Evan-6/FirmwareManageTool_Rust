// Generated from firmware/common/input/BootloaderGate.h; do not edit this copy.
#pragma once
#include <stdint.h>
namespace hidfw {
class BootloaderGate {
  public:
    bool advance(uint32_t now, bool confirmed, bool output_idle, bool vendor_ready, bool tx_empty);
    bool pending() const { return pending_; }

  private:
    bool pending_ = false;
    uint32_t at_ = 0;
};
} // namespace hidfw
