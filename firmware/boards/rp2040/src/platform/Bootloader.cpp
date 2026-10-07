#include "Bootloader.h"
#include "BoardConfig.h"
#include <Arduino.h>
namespace hidfw {
void Bootloader::update(uint32_t now, bool confirmed, bool output_idle, bool vendor_ready,
                        bool tx_empty) {
    if (!confirmed) {
        pending_ = false;
        return;
    }
    if (!pending_) {
        pending_ = true;
        at_ = now;
    }
    if (uint32_t(now - at_) >= board::BootloaderResetDelayMs && output_idle && vendor_ready &&
        tx_empty)
        rp2040.rebootToBootloader();
}
} // namespace hidfw
