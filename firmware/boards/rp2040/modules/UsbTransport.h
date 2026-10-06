// RP2040 internal module; included once by Firmware.cpp.
// Keyboard, Consumer and mouse share ONE HID interface (report ids 2/3/4/5). This is not
// cosmetic: the RP2040 TinyUSB port hard-caps CFG_TUD_HID at 2 interfaces, and
// the second slot must stay free for the Vendor HID command channel. With three
// separate instances the last begin() (vendor) silently fails and the command
// channel never enumerates. A combined interface cannot claim a boot protocol,
// which only matters to BIOS/boot-protocol-only hosts.
const uint8_t HidReportDescriptor[] = {
    // 8 modifier bits, then 224 usage bits (reserved usages are always zero).
    0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x85, KeyboardReportId, 0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7,
    0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x08, 0x81, 0x02, 0x19, 0x00, 0x29, 0xDF, 0x75, 0x01,
    0x95, 0xE0, 0x81, 0x02,
    // Host keyboard LEDs, retained for lock-key compatibility.
    0x05, 0x08, 0x19, 0x01, 0x29, 0x05, 0x75, 0x01, 0x95, 0x05, 0x91, 0x02, 0x75, 0x03, 0x95, 0x01,
    0x91, 0x01, 0xC0,
    // Seven Consumer controls and one constant padding bit.
    0x05, 0x0C, 0x09, 0x01, 0xA1, 0x01, 0x85, ConsumerReportId, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01,
    0x95, 0x07, 0x09, 0xE2, 0x09, 0xE9, 0x09, 0xEA, 0x09, 0xCD, 0x09, 0xB5, 0x09, 0xB6, 0x09, 0xB7,
    0x81, 0x02, 0x95, 0x01, 0x81, 0x01, 0xC0,
    TUD_HID_REPORT_DESC_MOUSE(HID_REPORT_ID(MouseReportId)),

    // Absolute pointer report (report id AbsMouseReportId): 5 buttons + 16-bit X/Y with
    // an Absolute (not Relative) Input flag. This is the standard "USB tablet" HID trick
    // (also used by VirtualBox/VMware absolute-mouse devices): Windows' HID mouse class
    // driver maps the 0-65535 logical range onto the primary display, so the report
    // itself carries the on-screen position instead of a delta.
    // Keep the descriptor/layout compatible, but always transmit zero buttons in
    // this collection. The relative collection owns buttons for both motion modes.
    0x05, 0x01,                         // Usage Page (Generic Desktop)
    0x09, 0x02,                         // Usage (Mouse)
    0xA1, 0x01,                         // Collection (Application)
    0x85, AbsMouseReportId, 0x09, 0x01, //   Usage (Pointer)
    0xA1, 0x00,                         //   Collection (Physical)
    0x05, 0x09,                         //     Usage Page (Button)
    0x19, 0x01,                         //     Usage Minimum (Button 1)
    0x29, 0x05,                         //     Usage Maximum (Button 5)
    0x15, 0x00,                         //     Logical Minimum (0)
    0x25, 0x01,                         //     Logical Maximum (1)
    0x95, 0x05,                         //     Report Count (5)
    0x75, 0x01,                         //     Report Size (1)
    0x81, 0x02,                         //     Input (Data, Variable, Absolute)
    0x95, 0x01,                         //     Report Count (1)
    0x75, 0x03,                         //     Report Size (3)
    0x81, 0x01,                         //     Input (Constant) - padding to a full byte
    0x05, 0x01,                         //     Usage Page (Generic Desktop)
    0x09, 0x30,                         //     Usage (X)
    0x09, 0x31,                         //     Usage (Y)
    0x17, 0x00, 0x00, 0x00, 0x00,       // Logical Minimum (0)
    0x27, 0xFF, 0xFF, 0x00, 0x00,       // Logical Maximum (65535)
    0x75, 0x10,                         //     Report Size (16)
    0x95, 0x02,                         //     Report Count (2)
    0x81, 0x02,                         //     Input (Data, Variable, Absolute)
    0xC0,                               //   End Collection
    0xC0                                // End Collection
};

// Vendor-defined command channel (usage page 0xFF60), mirrors the AVR build.
const uint8_t VendorHidReportDescriptor[] = {
    0x06, 0x60, 0xFF, 0x09, 0x61, 0xA1, 0x01, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x95, 63,
    0x85, 10,   0x09, 0x62, 0x91, 0x02, 0x85, 11,   0x09, 0x63, 0x81, 0x02, 0x85, 12,   0x09, 0x64,
    0x91, 0x02, 0x85, 13,   0x09, 0x65, 0x81, 0x02, 0x85, 14,   0x09, 0x66, 0xB1, 0x02, 0xC0};

Adafruit_USBD_HID usb_hid(HidReportDescriptor, sizeof(HidReportDescriptor), HID_ITF_PROTOCOL_NONE,
                          1, false);

Adafruit_USBD_HID usb_vendor(VendorHidReportDescriptor, sizeof(VendorHidReportDescriptor),
                             HID_ITF_PROTOCOL_NONE, 1,
                             true); // has_out_endpoint

uint8_t popcount8(uint8_t value) {
    value = static_cast<uint8_t>(value - ((value >> 1) & 0x55));
    value = static_cast<uint8_t>((value & 0x33) + ((value >> 2) & 0x33));
    return static_cast<uint8_t>((value + (value >> 4)) & 0x0F);
}
