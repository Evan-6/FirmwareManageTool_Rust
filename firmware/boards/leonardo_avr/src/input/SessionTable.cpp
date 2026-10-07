// Generated from firmware/common/input/SessionTable.cpp; do not edit this copy.
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
SourceView SessionTable::sources() const {
    SourceView view;
    for (uint8_t i = 0; i < SessionLimit; ++i)
        view.states[i] = &sessions_[i].state;
    return view;
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
