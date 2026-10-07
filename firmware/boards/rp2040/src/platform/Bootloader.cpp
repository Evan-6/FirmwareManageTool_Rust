#include "Bootloader.h"
#include <Arduino.h>
namespace hidfw {
void Bootloader::update(uint32_t now, bool confirmed, bool output_idle, bool vendor_ready,
                        bool tx_empty) {
    if (gate_.advance(now, confirmed, output_idle, vendor_ready, tx_empty))
        rp2040.rebootToBootloader();
}
} // namespace hidfw
