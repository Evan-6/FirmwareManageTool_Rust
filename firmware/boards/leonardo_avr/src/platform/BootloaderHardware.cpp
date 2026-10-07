#include "Bootloader.h"
#include <Arduino.h>
#include <avr/pgmspace.h>
#include <avr/wdt.h>
namespace hidfw {
namespace avr {
bool hasNewLufaBootloader() { return pgm_read_word(FLASHEND - 1) == NEW_LUFA_SIGNATURE; }

void enterBootloader() {
    uintptr_t magic_key_pos = MAGIC_KEY_POS;

#if MAGIC_KEY_POS != (RAMEND - 1)
    if (hasNewLufaBootloader()) {
        magic_key_pos = (RAMEND - 1);
    }

    if (magic_key_pos != (RAMEND - 1) && *(uint16_t *)magic_key_pos != MAGIC_KEY) {
        *(uint16_t *)(RAMEND - 1) = *(uint16_t *)magic_key_pos;
    }
#endif

    *(uint16_t *)magic_key_pos = MAGIC_KEY;
    wdt_enable(WDTO_120MS);
    while (true) {
    }
}

} // namespace avr
} // namespace hidfw
