#pragma once
#include "InputState.h"
namespace hidfw {
struct Pending {
    uint8_t op = 0;
    uint32_t seq = 0;
};
struct Session {
    uint32_t id = 0, received = 0, completed = 0, lease_at = 0;
    InputState state{};
    Pending pending{};
    bool released = false;
};
class SessionTable {
  public:
    Session *find(uint32_t id);
    const Session *find(uint32_t id) const;
    uint8_t count() const;
    SourceView sources() const;
    void reset();
    void completed(uint32_t id, uint32_t seq);
    Session *allocate();
    Session *entries() { return sessions_; }
    uint8_t slot(const Session &s) const { return &s - sessions_; }

  private:
    Session sessions_[SessionLimit]{};
};
} // namespace hidfw
