// Generated from firmware/common/input/InputConfig.h; do not edit this copy.
#pragma once
#include <stdint.h>
namespace hidfw {
namespace config {
constexpr uint8_t Sessions = 8;
constexpr uint16_t BootloaderDelayMs = 120;
#if defined(ARDUINO_ARCH_AVR)
constexpr uint8_t OutputDepth = 2, RxDepth = 1, TxDepth = 8;
#else
constexpr uint8_t OutputDepth = 32, RxDepth = 32, TxDepth = 32;
#endif
static_assert(TxDepth >= Sessions, "A global fault must fit every session event");
} // namespace config
} // namespace hidfw
