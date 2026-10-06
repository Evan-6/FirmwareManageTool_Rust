#include "Firmware.h"
#include "KeyCatalog.h"
#include <Adafruit_TinyUSB.h>
#include <Arduino.h>
#include <atomic>
#include <pico/unique_id.h>
#include <pico/util/queue.h>
#include <stdlib.h>
#include <string.h>
#if !defined(FIRMWARE_ENABLE_NEOPIXEL)
#if defined(ARDUINO_SEEED_XIAO_RP2040)
#define FIRMWARE_ENABLE_NEOPIXEL 1
#else
#define FIRMWARE_ENABLE_NEOPIXEL 0
#endif
#endif
#if FIRMWARE_ENABLE_NEOPIXEL
#include <Adafruit_NeoPixel.h>
#endif
#if !defined(USE_TINYUSB)
#error "Select usbstack=tinyusb"
#endif
namespace {
// Internal modules share this single state instance; the include order is intentional.
#include "modules/Config.h"
#include "modules/InputTypes.h"
#include "modules/InputState.h"
#include "modules/UsbTransport.h"
#include "modules/OutputScheduler.h"
#include "modules/LegacyKeyboard.h"
#include "modules/MouseState.h"
#include "modules/StatusLed.h"
#include "modules/LegacyTx.h"
#include "modules/LegacyKeys.h"
#include "modules/LegacyV2.h"
#include "modules/BinaryV3.h"
#include "modules/LegacyRx.h"
#include "modules/Lifecycle.h"
} // namespace
extern "C" void tud_hid_report_complete_cb(uint8_t instance, uint8_t const *report, uint16_t len) {
    if (!len)
        return;
    if (instance == 0)
        output.complete(report[0]);
    else if (instance == 1 && binary::boot_reply_inflight) {
        binary::boot_reply_inflight = false;
        binary::boot_reply_completed.store(true);
    }
}
extern "C" void tud_hid_report_failed_cb(uint8_t instance, hid_report_type_t, uint8_t const *,
                                         uint16_t) {
    if (instance == 0)
        output.failed();
}
namespace Firmware {
void setup() {
    if (!TinyUSBDevice.isInitialized())
        TinyUSBDevice.begin(0);
    Serial.end();
    TinyUSBDevice.setID(UsbVid, UsbPid);
    TinyUSBDevice.setDeviceVersion(0x0302);
    static char serial[17];
    pico_get_unique_board_id_string(serial, sizeof(serial));
    TinyUSBDevice.setSerialDescriptor(serial);
    TinyUSBDevice.setManufacturerDescriptor(UsbManufacturer);
    TinyUSBDevice.setProductDescriptor(UsbProduct);
    queue_init(&vendor_rx, sizeof(RxEvent), RxQueueDepth);
    queue_init(&binary::rx, sizeof(binary::Report), 32);
    queue_init(&binary::tx, sizeof(binary::Report), 8);
    usb_vendor.setReportCallback(binary::feature, vendorSetReport);
    usb_hid.begin();
    usb_vendor.begin();
    keyboard.begin();
    clearTxQueue(vendor_tx);
    if (TinyUSBDevice.mounted()) {
        TinyUSBDevice.detach();
        delay(10);
        TinyUSBDevice.attach();
    }
    last_lease_renewed_at = millis();
    vendor_parser.last_rx_byte_at = millis();
}
void loop() {
#ifdef TINYUSB_NEED_POLLING_TASK
    TinyUSBDevice.task();
#endif
    const unsigned long now = millis();
    serviceVendorRx(now);
    binary::serviceRx();
    serviceLineTimeout(vendor_parser, now);
    serviceFailsafe(now);
    output.flush(millis(), TinyUSBDevice.mounted());
    binary::serviceControl();
    binary::serviceTx();
    serviceVendorTx();
    led::update(millis(), TinyUSBDevice.mounted(), output.hasPressed());
    serviceBootloaderReset(millis());
}
void setup1() { led::begin(); }
void loop1() {
    led::render(millis());
    delay(1);
}
} // namespace Firmware
