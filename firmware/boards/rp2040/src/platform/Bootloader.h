#pragma once
#include <stdint.h>
namespace hidfw {
class Bootloader {
  public:
    void update(uint32_t now, bool confirmed, bool output_idle, bool vendor_ready, bool tx_empty);
    bool pending() const { return pending_; }

  private:
    uint32_t at_ = 0;
    bool pending_ = false;
};
} // namespace hidfw
