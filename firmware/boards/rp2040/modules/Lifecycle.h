void scheduleBootloaderReset() {
    bootloader_reset_requested_at = millis();
    bootloader_reset_pending = true;
    led::signalBootloader();
}
void serviceBootloaderReset(unsigned long now) {
    if (bootloader_reset_pending && now - bootloader_reset_requested_at >= BootloaderResetDelayMs &&
        output.idle() && usb_vendor.ready() && queue_get_level(&binary::tx) == 0) {
        rp2040.rebootToBootloader();
    }
}
