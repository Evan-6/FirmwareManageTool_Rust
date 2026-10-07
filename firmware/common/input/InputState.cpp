#include "InputState.h"
namespace hidfw {
bool InputState::set(uint16_t page, uint16_t usage, bool down) {
    uint8_t *byte = nullptr;
    uint8_t bit = 0;
    if (page == 7 && key_catalog::keyboardSupported(usage)) {
        if (usage >= 0xe0 && usage <= 0xe7) {
            byte = &modifiers;
            bit = 1u << (usage - 0xe0);
        } else {
            byte = &keys[usage / 8];
            bit = 1u << (usage % 8);
        }
    } else if (page == 12) {
        for (uint8_t i = 0; i < 7; ++i)
            if (key_catalog::ConsumerUsages[i] == usage) {
                byte = &consumer;
                bit = 1u << i;
            }
    }
    if (!byte)
        return false;
    if (down)
        *byte |= bit;
    else
        *byte &= static_cast<uint8_t>(~bit);
    return true;
}
bool InputState::valid() const {
    if ((consumer & 0x80) || (buttons & ~31))
        return false;
    for (uint16_t u = 0; u < 224; ++u)
        if ((keys[u / 8] & (1u << (u % 8))) && !key_catalog::keyboardSupported(u))
            return false;
    return true;
}
bool InputState::held() const {
    if (modifiers || consumer || buttons)
        return true;
    for (uint8_t b : keys)
        if (b)
            return true;
    return false;
}
InputState mergeSources(const InputState *states) {
    InputState merged;
    for (uint8_t i = 0; i < SessionLimit; ++i) {
        merged.modifiers |= states[i].modifiers;
        merged.consumer |= states[i].consumer;
        merged.buttons |= states[i].buttons;
        for (uint8_t k = 0; k < 28; ++k)
            merged.keys[k] |= states[i].keys[k];
    }
    return merged;
}
void SourceView::capture(InputState *destination) const {
    for (uint8_t i = 0; i < SessionLimit; ++i)
        destination[i] = *states[i];
}
InputState SourceView::aggregate() const {
    InputState merged;
    for (uint8_t i = 0; i < SessionLimit; ++i) {
        const InputState &state = *states[i];
        merged.modifiers |= state.modifiers;
        merged.consumer |= state.consumer;
        merged.buttons |= state.buttons;
        for (uint8_t k = 0; k < 28; ++k)
            merged.keys[k] |= state.keys[k];
    }
    return merged;
}
} // namespace hidfw
