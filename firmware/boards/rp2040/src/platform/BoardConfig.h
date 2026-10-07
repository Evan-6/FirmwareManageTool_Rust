#pragma once
#include <stdint.h>
#ifndef FIRMWARE_ENABLE_NEOPIXEL
#ifdef ARDUINO_SEEED_XIAO_RP2040
#define FIRMWARE_ENABLE_NEOPIXEL 1
#else
#define FIRMWARE_ENABLE_NEOPIXEL 0
#endif
#endif
namespace hidfw::board {
constexpr bool EnableStatusLed = FIRMWARE_ENABLE_NEOPIXEL != 0;
constexpr bool EnableRgbAnimations = true; // false => flat colours, no fades.
#if FIRMWARE_ENABLE_NEOPIXEL
#if !defined(FIRMWARE_NEOPIXEL_POWER_PIN)
#define FIRMWARE_NEOPIXEL_POWER_PIN 11
#endif
#if !defined(FIRMWARE_NEOPIXEL_DATA_PIN)
#define FIRMWARE_NEOPIXEL_DATA_PIN 12
#endif
constexpr uint8_t NeoPixelPowerPin = FIRMWARE_NEOPIXEL_POWER_PIN;
constexpr uint8_t NeoPixelDataPin = FIRMWARE_NEOPIXEL_DATA_PIN;

#endif

// USB identity: kept in sync with boards/leonardo_avr so the host recognises
// either board interchangeably.
#if !defined(FIRMWARE_USB_VID)
#define FIRMWARE_USB_VID 0x03F0
#endif
#if !defined(FIRMWARE_USB_PID)
#define FIRMWARE_USB_PID 0x0024
#endif
#if !defined(FIRMWARE_USB_MANUFACTURER)
#define FIRMWARE_USB_MANUFACTURER "HP"
#endif
#if !defined(FIRMWARE_USB_PRODUCT)
#define FIRMWARE_USB_PRODUCT "HP Keyboard"
#endif

constexpr uint16_t UsbVid = FIRMWARE_USB_VID;
constexpr uint16_t UsbPid = FIRMWARE_USB_PID;
constexpr char UsbManufacturer[] = FIRMWARE_USB_MANUFACTURER;
constexpr char UsbProduct[] = FIRMWARE_USB_PRODUCT;

constexpr unsigned long BootloaderResetDelayMs = 120;

constexpr uint8_t BoardId =
#ifdef ARDUINO_SEEED_XIAO_RP2040
    2;
#else
    1;
#endif
} // namespace hidfw::board
