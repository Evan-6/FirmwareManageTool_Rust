#pragma once
#include <stdint.h>
namespace hidfw {
namespace config {
constexpr uint8_t Sessions = 8;
constexpr uint16_t BootloaderDelayMs = 120;
constexpr uint8_t OutputDepth = 32, RxDepth = 32, TxDepth = 32;
static_assert(TxDepth >= Sessions, "A global fault must fit every session event");
} // namespace config
} // namespace hidfw
