#include "Bootloader.h"
namespace hidfw {
void Bootloader::update(uint32_t now, bool confirmed, bool output_idle, bool vendor_ready,
                        bool tx_empty) {
    if (gate_.advance(now, confirmed, output_idle, vendor_ready, tx_empty))
        avr::enterBootloader();
}
} // namespace hidfw
