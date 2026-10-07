#pragma once
bool hasNewLufaBootloader() { return pgm_read_word(FLASHEND - 1) == NEW_LUFA_SIGNATURE; }

void enterBootloaderNow() {
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

void scheduleBootloaderReset() {
    bootloader_reset_pending = true;
    bootloader_reset_requested_at = millis();
}
void serviceBootloaderReset(unsigned long now) {
    if (bootloader_reset_pending && now - bootloader_reset_requested_at >= 120 && output.idle() &&
        usb_vendor.ready() && queue_get_level(&binary::tx) == 0)
        enterBootloaderNow();
}
