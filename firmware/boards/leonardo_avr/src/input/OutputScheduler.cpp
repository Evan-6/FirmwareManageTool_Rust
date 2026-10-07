// Generated from firmware/common/input/OutputScheduler.cpp; do not edit this copy.
#include "OutputScheduler.h"
namespace hidfw {
uint8_t OutputScheduler::pending(uint32_t session) const {
    uint8_t n = 0;
    for (uint8_t i = 0; i < count_; ++i)
        n += jobs_[(head_ + i) % Depth].session == session;
    return n;
}
bool OutputScheduler::state(const InputState &next, const SourceView &sources, uint32_t now,
                            uint32_t seq, bool force, uint32_t session) {
    if (!room())
        return false;
    Job &j = jobs_[(head_ + count_) % Depth];
    j = {};
    j.state = next;
    j.sequence = seq;
    j.session = session;
    sources.capture(j.sources);
    if (force || next.modifiers != desired_.modifiers || memcmp(next.keys, desired_.keys, 28))
        j.flags |= Keyboard;
    if (force || next.consumer != desired_.consumer)
        j.flags |= Consumer;
    // One relative mouse owns movement, all five buttons and both wheels.
    if (force || next.buttons != desired_.buttons)
        j.flags |= Relative;
    add(now);
    desired_ = next;
    return true;
}
bool OutputScheduler::motion(int32_t x, int32_t y, int32_t wheel, int32_t hwheel,
                             const SourceView &sources, uint32_t now, uint32_t seq,
                             uint32_t session) {
    // Only combine adjacent pending motion, never the in-flight head or across
    // buttons/barriers.
    if (count_ > 1) {
        Job &last = jobs_[(head_ + count_ - 1) % Depth];
        if (last.kind == 1 && last.session == session && last.state.buttons == desired_.buttons) {
            const int64_t nx = int64_t(last.x) + x, ny = int64_t(last.y) + y,
                          nw = int64_t(last.wheel) + wheel, nh = int64_t(last.hwheel) + hwheel;
            if (nx < -8192 || nx > 8192 || ny < -8192 || ny > 8192 || nw < -8192 || nw > 8192 ||
                nh < -8192 || nh > 8192) {
                fault_ = 8;
                return false;
            }
            last.x = nx;
            last.y = ny;
            last.wheel = nw;
            last.hwheel = nh;
            last.sequence = seq;
            return true;
        }
    }
    if (!room())
        return false;
    Job &j = jobs_[(head_ + count_) % Depth];
    j = {};
    j.state = desired_;
    j.kind = 1;
    j.flags = Relative;
    j.x = x;
    j.y = y;
    j.wheel = wheel;
    j.hwheel = hwheel;
    j.sequence = seq;
    j.session = session;
    sources.capture(j.sources);
    add(now);
    return true;
}
bool OutputScheduler::barrier(const SourceView &sources, uint32_t now, uint32_t seq,
                              uint32_t session) {
    // The last job already has exactly this frontier. Reuse its completion
    // for the same owner without another output slot.
    if (count_) {
        Job &last = jobs_[(head_ + count_ - 1) % Depth];
        if (last.session == session) {
            last.sequence = seq;
            return true;
        }
    }
    if (!room())
        return false;
    Job &j = jobs_[(head_ + count_) % Depth];
    j = {};
    j.state = desired_;
    j.sequence = seq;
    j.session = session;
    sources.capture(j.sources);
    add(now);
    return true;
}
void OutputScheduler::release(uint32_t now) {
    ++generation_;
    head_ = 0;
    count_ = 0;
    desired_ = InputState{};
    Job &j = jobs_[0];
    j = {};
    j.state = desired_;

    j.flags = Keyboard | Consumer | Relative;
    j.queued_at = now;
    count_ = 1;
}
bool OutputScheduler::releaseSession(uint32_t session, uint32_t seq, uint8_t slot,
                                     const SourceView &sources, uint32_t now) {
    const bool preserve_inflight = in_flight_ && count_ && jobs_[head_].session != session;
    if (!preserve_inflight)
        ++generation_; // Ignore completion only when its owner is being cancelled.
    const uint8_t old_count = count_;
    uint8_t kept = 0;
    InputState previous = completed_;
    for (uint8_t i = 0; i < old_count; ++i) {
        Job &j = jobs_[(head_ + i) % Depth];
        if (j.session == session)
            continue;
        const uint8_t original_flags = j.flags;
        j.sources[slot] = {};
        j.state = mergeSources(j.sources);
        j.flags = 0;
        if (!kept || j.state.modifiers != previous.modifiers ||
            memcmp(j.state.keys, previous.keys, 28))
            j.flags |= Keyboard;
        if (!kept || j.state.consumer != previous.consumer)
            j.flags |= Consumer;
        if (!kept || j.kind == 1 || j.state.buttons != previous.buttons)
            j.flags |= Relative;
        if (i == 0 && preserve_inflight)
            j.flags = original_flags; // Its pending completion must subtract motion exactly once.
        jobs_[(head_ + kept) % Depth] = j;
        previous = j.state;
        ++kept;
    }
    count_ = kept;
    desired_ = previous;
    return state(sources.aggregate(), sources, now, seq, true, session);
}
bool OutputScheduler::room() {
    if (count_ < Depth)
        return true;
    fault_ = 8;
    return false;
}
void OutputScheduler::add(uint32_t now) {
    jobs_[(head_ + count_) % Depth].queued_at = now;
    ++count_;
}
int8_t OutputScheduler::clamp(int32_t n) { return n < -127 ? -127 : n > 127 ? 127 : n; }
void OutputScheduler::disconnect() { in_flight_ = false; }
void OutputScheduler::complete(uint8_t done) {
    if (!done || !in_flight_)
        return;
    in_flight_ = false;
    if (inflight_generation_ != generation_ || !count_)
        return;
    Job &j = jobs_[head_];
    if (done == wire::KeyboardReportId)
        j.flags &= ~Keyboard;
    else if (done == wire::ConsumerReportId)
        j.flags &= ~Consumer;
    else if (done == wire::MouseReportId) {
        j.x -= sent_x_;
        j.y -= sent_y_;
        j.wheel -= sent_wheel_;
        j.hwheel -= sent_hwheel_;
        if (!j.x && !j.y && !j.wheel && !j.hwheel)
            j.flags &= ~Relative;
    }
}
bool OutputScheduler::stale(uint32_t now) const {
    return count_ && uint32_t(now - jobs_[head_].queued_at) > 50;
}
bool OutputScheduler::popCompleted(Completion &result) {
    if (in_flight_ || !count_ || jobs_[head_].flags)
        return false;
    const Job &j = jobs_[head_];
    completed_ = j.state;
    result = {j.session, j.sequence};
    head_ = (head_ + 1) % Depth;
    --count_;
    return true;
}
bool OutputScheduler::prepare(uint32_t now, bool ready, InputReport &r) {
    if (in_flight_ || !count_ || !ready || uint32_t(now - last_send_at_) < 1)
        return false;
    Job &j = jobs_[head_];
    r = {};
    if (j.flags & Keyboard) {
        r.id = wire::KeyboardReportId;
        r.length = 29;
        r.data[0] = j.state.modifiers;
        memcpy(r.data + 1, j.state.keys, 28);
    } else if (j.flags & Consumer) {
        r.id = wire::ConsumerReportId;
        r.length = 1;
        r.data[0] = j.state.consumer;
    } else if (j.flags & Relative) {
        r.id = wire::MouseReportId;
        r.length = 5;
        r.data[0] = j.state.buttons;
        sent_x_ = clamp(j.x);
        sent_y_ = clamp(j.y);
        sent_wheel_ = clamp(j.wheel);
        sent_hwheel_ = clamp(j.hwheel);
        r.data[1] = sent_x_;
        r.data[2] = sent_y_;
        r.data[3] = sent_wheel_;
        r.data[4] = sent_hwheel_;
    }
    last_send_at_ = now;
    in_flight_ = true;
    inflight_generation_ = generation_;
    return true;
}
} // namespace hidfw
