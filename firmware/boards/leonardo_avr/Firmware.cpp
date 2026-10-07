#include "Firmware.h"
#include "KeyCatalog.h"
#include <Arduino.h>
#include <HID.h>
#include <avr/pgmspace.h>
#include <avr/wdt.h>
#include <util/atomic.h>
#include <string.h>
#if !defined(USBCON) || !defined(CDC_DISABLED)
#error "Leonardo v3 requires the bundled native USB core with CDC_DISABLED."
#endif
namespace {
#include "modules/Platform.h"
#include "modules/V3InputState.h"
#include "modules/UsbTransport.h"
#include "modules/V3OutputScheduler.h"
#include "modules/V3BinaryV3.h"
#include "modules/BinaryRx.h"
#include "modules/Lifecycle.h"
uint16_t vendorFeature(uint8_t id, hid_report_type_t type, uint8_t *data, uint16_t length) {
    return binary::feature(id, type, data, length);
}
} // namespace
namespace Firmware {
void setup() { usb_generation = USB_ResetGeneration(); }
void loop() {
    const uint8_t generation = USB_ResetGeneration();
    if (generation != usb_generation) {
        usb_generation = generation;
        usb_hid.reset();
        usb_vendor.reset();
        output.flush(millis(), false); // Discard lost packets before detecting any completion.
    }
    const uint8_t completed = usb_hid.completed();
    if (completed)
        output.complete(completed);
    if (usb_vendor.completed() && binary::boot_reply_inflight) {
        binary::boot_reply_inflight = false;
        binary::boot_reply_completed.store(true);
    }
    usb_vendor.serviceRx();
    binary::serviceRx();
    output.flush(millis(), TinyUSBDevice.mounted());
    binary::serviceControl();
    binary::serviceTx();
    serviceBootloaderReset(millis());
}
} // namespace Firmware
