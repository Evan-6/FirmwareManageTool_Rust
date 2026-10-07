#pragma once
#include <stdint.h>
#include <string.h>
#include <vector>
#include <deque>
extern unsigned long test_time;
inline unsigned long millis() { return test_time; }
#define USBCON 1
#define CDC_DISABLED 1
#define MAGIC_KEY_POS 0x0800
#define MAGIC_KEY 0x7777
#define NEW_LUFA_SIGNATURE 0xDCFB
#define RAMEND 0x0AFF
#define FLASHEND 0x7FFF
struct Device {
    bool configured_ = true, suspended_ = false;
    bool configured() { return configured_; }
    bool isSuspended() { return suspended_; }
};
inline Device USBDevice;
