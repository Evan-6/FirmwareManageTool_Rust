#include "V3Descriptors.h"
Adafruit_USBD_HID usb_hid(HidReportDescriptor, sizeof(HidReportDescriptor), HID_ITF_PROTOCOL_NONE,
                          1, false);
Adafruit_USBD_HID usb_vendor(VendorHidReportDescriptor, sizeof(VendorHidReportDescriptor),
                             HID_ITF_PROTOCOL_NONE, 1, true);
