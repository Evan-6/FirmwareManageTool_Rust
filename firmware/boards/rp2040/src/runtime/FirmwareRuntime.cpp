#include "FirmwareRuntime.h"
#include <Arduino.h>
#include <pico/unique_id.h>
namespace hidfw {
void FirmwareRuntime::begin() {
    wire::DeviceInfo info;
    info.board = board::BoardId;
    pico_unique_board_id_t uid;
    pico_get_unique_board_id(&uid);
    memcpy(info.id, uid.id, 8);
    usb_.begin(info);
}
void FirmwareRuntime::discardFaultedRx() {
    if (rx_generation_ != engine_.rxGeneration()) {
        rx_generation_ = engine_.rxGeneration();
        usb_.discardRx();
        // Discard fault flags but preserve every callback's accumulated error count.
        const auto errors = usb_.takeRxFault();
        engine_.receiveFault(0, errors & 0xffff, millis());
    }
}
void FirmwareRuntime::loop() {
#ifdef TINYUSB_NEED_POLLING_TASK
    TinyUSBDevice.task();
#endif
    const uint32_t now = millis();
    if (usb_.takeReset())
        engine_.receiveFault(7, 0, now);
    const auto fault = usb_.takeRxFault();
    engine_.receiveFault((fault & (1u << 16))   ? 1
                         : (fault & (1u << 17)) ? 8
                                                : 0,
                         fault & 0xffff, now);
    discardFaultedRx();
    wire::Report packet;
    for (uint8_t budget = 0; budget < 4 && usb_.read(packet); ++budget) {
        engine_.handle(packet, now);
        discardFaultedRx();
    }
    InputReport input;
    if (engine_.advance(now, usb_.mounted(), usb_.takeInputComplete(), usb_.takeInputFailure(),
                        usb_.inputReady(), input))
        if (!usb_.sendInput(input))
            engine_.inputRejected(now);
    discardFaultedRx();
    uint32_t generation;
    if (usb_.takeVendorComplete(generation))
        engine_.vendorCompleted(generation);
    engine_.serviceControl(now);
    discardFaultedRx();
    if (usb_.mounted() && usb_.vendorReady())
        if (const auto *reply = engine_.vendorReply())
            if (usb_.sendVendor(*reply, engine_.bootGeneration()))
                engine_.vendorSubmitted();
    const auto notices = engine_.takeNotices();
    if (notices & InputEngine::Error)
        indicator_.signalError(now);
    if (notices & InputEngine::Failsafe)
        indicator_.signalFailsafe(now);
    if (notices & InputEngine::BootConfirmed)
        indicator_.signalBootloader();
    indicator_.update(now, usb_.mounted(), engine_.desiredState().held());
    boot_.update(now, engine_.bootConfirmed(), engine_.idle(), usb_.vendorReady(),
                 engine_.txEmpty());
}
void FirmwareRuntime::beginLed() { renderer_.begin(millis()); }
void FirmwareRuntime::renderLed() {
    renderer_.render(millis(), indicator_.state());
    delay(1);
}
} // namespace hidfw
