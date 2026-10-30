#pragma once
#include <stdint.h>
#include <string.h>
#include "KeyCatalog.h"
#include "InputConfig.h"
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
constexpr uint8_t SessionLimit = config::Sessions;
using Sources = InputState[SessionLimit];
InputState mergeSources(const InputState *sources);
// Borrowed only during a core-0 scheduling call. Jobs store copied states,
// never these pointers, so historical peer input survives future changes.
struct SourceView {
    const InputState *states[SessionLimit]{};
    void capture(InputState *destination) const;
    InputState aggregate() const;
};
} // namespace hidfw
