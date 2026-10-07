#pragma once
#include <stdint.h>
#include <string.h>
#include "../../KeyCatalog.h"
namespace hidfw {
struct InputState {
    uint8_t modifiers = 0;
    uint8_t keys[28] = {};
    uint8_t consumer = 0;
    uint8_t buttons = 0;
    bool set(uint16_t page, uint16_t usage, bool down);
    bool valid() const;
    bool held() const;
};
static_assert(sizeof(InputState) == 31, "Input snapshot must have a stable byte layout");
constexpr uint8_t SessionLimit = 8;
using Sources = InputState[SessionLimit];
InputState mergeSources(const InputState *sources);
} // namespace hidfw
