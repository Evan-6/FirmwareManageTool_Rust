#include "SessionTable.h"
namespace hidfw {
Session *SessionTable::find(uint32_t id) {
    if (id)
        for (auto &s : sessions_)
            if (s.id == id)
                return &s;
    return nullptr;
}
const Session *SessionTable::find(uint32_t id) const {
    if (id)
        for (const auto &s : sessions_)
            if (s.id == id)
                return &s;
    return nullptr;
}
uint8_t SessionTable::count() const {
    uint8_t n = 0;
    for (const auto &s : sessions_)
        n += s.id != 0;
    return n;
}
Session *SessionTable::allocate() {
    for (auto &s : sessions_)
        if (!s.id)
            return &s;
    return nullptr;
}
void SessionTable::capture(InputState *states) const {
    for (uint8_t i = 0; i < SessionLimit; ++i)
        states[i] = sessions_[i].state;
}
InputState SessionTable::aggregate() const {
    InputState merged;
    for (const auto &s : sessions_) {
        merged.modifiers |= s.state.modifiers;
        merged.consumer |= s.state.consumer;
        merged.buttons |= s.state.buttons;
        for (uint8_t k = 0; k < 28; ++k)
            merged.keys[k] |= s.state.keys[k];
    }
    return merged;
}
void SessionTable::reset() {
    for (auto &s : sessions_)
        s = {};
}
void SessionTable::completed(uint32_t id, uint32_t seq) {
    if (auto *s = find(id))
        s->completed = seq;
}
} // namespace hidfw
