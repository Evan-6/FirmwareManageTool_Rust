#pragma once
#include <stdint.h>
#include <string.h>
struct InputState {
    uint8_t modifiers = 0;
    uint8_t keys[28] = {};
    uint8_t consumer = 0;
    uint8_t buttons = 0;
    bool set(uint16_t page, uint16_t usage, bool down) {
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
    bool valid() const {
        if ((consumer & 0x80) || (buttons & ~31))
            return false;
        for (uint16_t u = 0; u < 224; ++u)
            if ((keys[u / 8] & (1u << (u % 8))) && !key_catalog::keyboardSupported(u))
                return false;
        return true;
    }
    bool held() const {
        if (modifiers || consumer || buttons)
            return true;
        for (uint8_t b : keys)
            if (b)
                return true;
        return false;
    }
};
static_assert(sizeof(InputState) == 31, "Input snapshot must have a stable byte layout");
enum class ProtocolOwner : uint8_t { None, Binary };
ProtocolOwner protocol_owner = ProtocolOwner::None;
namespace binary {
constexpr uint8_t SessionLimit = 8;
struct Pending {
    uint8_t op = 0;
    uint32_t seq = 0;
};
struct Session {
    uint32_t id = 0, received = 0, completed = 0;
    unsigned long lease_at = 0;
    InputState state{};
    Pending pending{};
    bool released = false;
};
Session sessions[SessionLimit];
Session *find(uint32_t id) {
    if (id)
        for (auto &s : sessions)
            if (s.id == id)
                return &s;
    return nullptr;
}
uint8_t count() {
    uint8_t n = 0;
    for (const auto &s : sessions)
        n += s.id != 0;
    return n;
}
InputState merge(const InputState *states) {
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
void capture(InputState *states) {
    for (uint8_t i = 0; i < SessionLimit; ++i)
        states[i] = sessions[i].state;
}
InputState aggregate() {
    InputState merged;
    for (const auto &s : sessions) {
        merged.modifiers |= s.state.modifiers;
        merged.consumer |= s.state.consumer;
        merged.buttons |= s.state.buttons;
        for (uint8_t k = 0; k < 28; ++k)
            merged.keys[k] |= s.state.keys[k];
    }
    return merged;
}
void resetSessions() {
    for (auto &s : sessions)
        s = {};
}
void completed(uint32_t id, uint32_t seq) {
    if (auto *s = find(id))
        s->completed = seq;
}
} // namespace binary
uint16_t binary_rx_errors = 0, binary_tx_errors = 0, binary_hid_errors = 0,
         binary_failsafe_count = 0;
void protocolFault(uint8_t code);
