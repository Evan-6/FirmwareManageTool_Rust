#pragma once
#include "InputState.h"
#include "WireCodec.h"
namespace hidfw {
struct InputReport {
    uint8_t id = 0, length = 0, data[29]{};
};
struct Completion {
    uint32_t session = 0, sequence = 0;
};
class OutputScheduler {
  public:
    uint8_t pending() const { return count_; }
    uint8_t pending(uint32_t session) const;
    bool idle() const { return !count_ && !in_flight_; }
    bool state(const InputState &next, const SourceView &sources, uint32_t now, uint32_t seq = 0,
               bool force = false, uint32_t session = 0);
    bool motion(int32_t x, int32_t y, int32_t wheel, int32_t hwheel, const SourceView &sources,
                uint32_t now, uint32_t seq = 0, uint32_t session = 0);
    bool barrier(const SourceView &sources, uint32_t now, uint32_t seq, uint32_t session);
    void release(uint32_t now);
    bool releaseSession(uint32_t session, uint32_t seq, uint8_t slot, const SourceView &sources,
                        uint32_t now);
    void complete(uint8_t id);
    void disconnect();
    bool stale(uint32_t now) const;
    bool popCompleted(Completion &completion);
    bool prepare(uint32_t now, bool ready, InputReport &report);
    void rejected() { in_flight_ = false; }
    uint8_t takeFault() {
        const auto code = fault_;
        fault_ = 0;
        return code;
    }
    const InputState &desired() const { return desired_; }
    const InputState &completed() const { return completed_; }

  private:
    enum : uint8_t { Keyboard = 1, Consumer = 2, Relative = 4, Depth = config::OutputDepth };
    struct Job {
        InputState state{};
        Sources sources{};
        int32_t x = 0, y = 0, wheel = 0, hwheel = 0;
        uint32_t sequence = 0, session = 0, queued_at = 0;
        uint8_t flags = 0, kind = 0;
    };
    Job jobs_[Depth]{};
    InputState desired_{}, completed_{};
    uint8_t head_ = 0, count_ = 0, fault_ = 0;
    uint32_t last_send_at_ = 0, generation_ = 0, inflight_generation_ = 0;
    bool in_flight_ = false;
    int8_t sent_x_ = 0, sent_y_ = 0, sent_wheel_ = 0, sent_hwheel_ = 0;
    static int8_t clamp(int32_t n);
    bool room();
    void add(uint32_t now);
};
} // namespace hidfw
