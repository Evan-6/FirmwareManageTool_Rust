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
enum class ProtocolOwner : uint8_t { None, Legacy, Binary };
ProtocolOwner protocol_owner = ProtocolOwner::None;
ProtocolOwner releasing_protocol = ProtocolOwner::None;
uint32_t active_session = 0, received_sequence = 0, completed_sequence = 0;
uint16_t binary_rx_errors = 0, binary_tx_errors = 0, binary_hid_errors = 0,
         binary_failsafe_count = 0;
unsigned long binary_lease_at = 0;
bool binary_released = false;
void protocolFault(uint8_t code);
