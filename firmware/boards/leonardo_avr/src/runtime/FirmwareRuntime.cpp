#include "FirmwareRuntime.h"
namespace hidfw {
void FirmwareRuntime::begin() { usb_.begin(); }
void FirmwareRuntime::discardFaultedRx(uint32_t now) {
    if (rx_generation_ != engine_.rxGeneration()) {
        rx_generation_ = engine_.rxGeneration();
        usb_.discardRx();
        engine_.receiveFault(0, usb_.takeRxFault().errors, now);
    }
}
void FirmwareRuntime::loop() {
    const uint32_t now = millis();
    if (usb_.takeReset()) {
        engine_.receiveFault(7, 0, now);
        // Drop the old callback mailbox before polling newly arrived OUT data.
        discardFaultedRx(now);
    }
    const uint8_t completed = usb_.inputCompleted();
    uint32_t generation;
    if (usb_.vendorCompleted(generation))
        engine_.vendorCompleted(generation);
    usb_.serviceRx();
    const RxFault fault = usb_.takeRxFault();
    engine_.receiveFault(fault.code, fault.errors, now);
    discardFaultedRx(now);
    wire::Report packet;
    for (uint8_t budget = 0; budget < 4 && usb_.read(packet); ++budget) {
        engine_.handle(packet, now);
        discardFaultedRx(now);
    }
    InputReport input;
    if (engine_.advance(now, usb_.mounted(), completed, false, usb_.inputReady(), input))
        if (!usb_.sendInput(input))
            engine_.inputRejected(now);
    discardFaultedRx(now);
    engine_.serviceControl(now);
    discardFaultedRx(now);
    if (usb_.mounted() && usb_.vendorReady())
        if (const auto *reply = engine_.vendorReply())
            if (usb_.sendVendor(*reply, engine_.bootGeneration()))
                engine_.vendorSubmitted();
    engine_.takeNotices(); // Leonardo has no status LED renderer.
    boot_.update(now, engine_.bootConfirmed(), engine_.idle(), usb_.vendorReady(),
                 engine_.txEmpty());
}
} // namespace hidfw
