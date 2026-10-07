#include "InputEngine.h"
namespace hidfw {
using namespace wire;
void InputEngine::reply(uint8_t op, uint32_t session, uint32_t seq, const uint8_t *data,
                        uint8_t n) {
    if (tx_count_ == 32) {
        ++counters_.tx;
        overflow_ = true;
        return;
    }
    tx_[(tx_head_ + tx_count_) % 32] = wire::reply(op, session, seq, data, n);
    ++tx_count_;
}
void InputEngine::result(uint8_t op, uint32_t session, uint32_t seq, uint8_t code) {
    reply(op | 128, session, seq, &code, 1);
}
void InputEngine::event(uint8_t code, uint32_t session, uint32_t seq) {
    reply(Event, session, seq, &code, 1);
}
void InputEngine::discardReplies(uint32_t session) {
    const auto depth = tx_count_;
    for (uint8_t i = 0; i < depth; ++i) {
        const auto r = tx_[tx_head_];
        tx_head_ = (tx_head_ + 1) % 32;
        --tx_count_;
        if (read32(r.bytes + 4) != session) {
            tx_[(tx_head_ + tx_count_) % 32] = r;
            ++tx_count_;
        }
    }
}
uint16_t InputEngine::leaseRemaining(const Session &s) const {
    if (!sessions_.count())
        return 0;
    const uint32_t elapsed = now_ - s.lease_at;
    return elapsed >= 2000 ? 0 : 2000 - elapsed;
}
void InputEngine::clearBoot() {
    boot_requested_ = boot_inflight_ = boot_completed_ = boot_confirmed_ = false;
    ++boot_generation_;
}
void InputEngine::sessionFault(Session &s, uint8_t code) {
    const uint32_t id = s.id, seq = s.received;
    const auto slot = sessions_.slot(s);
    const bool remove = s.state.held() || output_.pending(id);
    discardReplies(id);
    if (boot_requested_)
        clearBoot();
    s = {};
    ++counters_.failsafe;
    if (remove)
        cancelOwner(id, 0, slot);
    event(code, id, seq);
    notices_ |= Error;
}
void InputEngine::fault(uint8_t code) {
    clearBoot();
    ++rx_generation_;
    overflow_ = false;
    output_.release(now_);
    notices_ |= Error;
    tx_head_ = tx_count_ = 0;
    for (uint8_t i = 0; i < SessionLimit; ++i) {
        const auto &s = sessions_.entries()[i];
        if (s.id) {
            ++counters_.failsafe;
            event(code, s.id, s.received);
        }
    }
    sessions_.reset();
}
void InputEngine::receiveFault(uint8_t code, uint16_t errors, uint32_t now) {
    now_ = now;
    counters_.rx += errors;
    if (code) {
        fault(code);
        if (code == 7)
            output_.disconnect();
    } else if (overflow_)
        fault(8);
}
bool InputEngine::checkScheduled(bool ok) {
    if (auto code = output_.takeFault())
        fault(code);
    return ok;
}
bool InputEngine::queueState(uint32_t seq, bool force, uint32_t session) {
    Sources sources;
    sessions_.capture(sources);
    return checkScheduled(output_.state(sessions_.aggregate(), sources, now_, seq, force, session));
}
bool InputEngine::queueMotion(int32_t x, int32_t y, int32_t w, int32_t h, uint32_t seq,
                              uint32_t session) {
    Sources sources;
    sessions_.capture(sources);
    return checkScheduled(output_.motion(x, y, w, h, sources, now_, seq, session));
}
bool InputEngine::queueBarrier(uint32_t seq, uint32_t session) {
    Sources sources;
    sessions_.capture(sources);
    return checkScheduled(output_.barrier(sources, now_, seq, session));
}
bool InputEngine::cancelOwner(uint32_t session, uint32_t seq, uint8_t slot) {
    Sources sources;
    sessions_.capture(sources);
    return checkScheduled(output_.releaseSession(session, seq, slot, sources, now_));
}
void InputEngine::status(const Session &s, uint32_t seq) {
    uint8_t p[51] = {};
    write32(p + 1, s.received);
    write32(p + 5, s.completed);
    p[9] = output_.pending(s.id);
    write16(p + 10, leaseRemaining(s));
    write16(p + 12, counters_.rx);
    write16(p + 14, counters_.tx);
    write16(p + 16, counters_.hid);
    write16(p + 18, counters_.failsafe);
    memcpy(p + 20, &s.state,
           31); // Client-local state: RELEASE must not test another client's holds.
    reply(Status | 0x80, s.id, seq, p, 51);
}
void InputEngine::handle(const wire::Report &report, uint32_t now) {
    now_ = now;
    const uint8_t *p = report.bytes;
    const uint8_t op = p[1];
    const uint32_t session = read32(p + 4), seq = read32(p + 8);
    Session *s = sessions_.find(session);
    if (s && now_ - s->lease_at >= 2000) {
        sessionFault(*s, 5);
        s = nullptr;
    }
    const bool valid = wire::valid(report);
    if (!valid) {
        ++counters_.rx;
        if (s)
            sessionFault(*s, 1);
        return;
    }
    if (op == Open) {
        if (!session || seq != 1) {
            result(op, session, seq, 1);
            return;
        }
        // Never let OPEN with a colliding ID reset a live client's sequence or keys.
        if (s || boot_requested_ || boot_confirmed_ || (!sessions_.count() && !output_.idle())) {
            result(op, session, seq, 2);
            return;
        }
        s = sessions_.allocate();
        if (!s) {
            result(op, session, seq, 2);
            return;
        }
        *s = {};
        s->id = session;
        s->received = 1;
        s->lease_at = now_;

        if (queueState(seq, sessions_.count() == 1, session))
            s->pending = {op, seq};
        return;
    }
    if (!sessions_.count() || !s)
        return;
    if (seq <= s->received || seq != s->received + 1) {
        ++counters_.rx;
        sessionFault(*s, seq <= s->received ? 9 : 10);
        return;
    }
    s->received = seq;
    const uint8_t *data = p + 12;
    if (op == Status) {
        status(*s, seq);
        return;
    }
    if (op == Heartbeat) {
        s->lease_at = now_;
        return;
    }
    if (boot_requested_)
        return; // A successful boot request has committed to an empty input state.
    if (op == Release || op == Bootloader) {
        if (s->pending.op) {
            sessionFault(*s, 2);
            return;
        }
        if (op == Bootloader && sessions_.count() > 1) {
            result(op, session, seq, 2);
            return;
        }
        // An empty owner with no queued input is already physically released.
        // Do not consume AVR output slots (or disturb peer jobs) just to confirm it.
        if (op == Release && !s->state.held() && !output_.pending(session)) {
            s->released = true;
            s->completed = seq;
            s->lease_at = now_;
            result(op, session, seq);
            return;
        }
        if (op == Bootloader)
            boot_requested_ = true;
        s->state = {};
        s->released = false;
        if (cancelOwner(session, seq, sessions_.slot(*s)))
            s->pending = {op, seq};
        s->lease_at = now_;
        return;
    }
    if (op == Barrier) {
        if (s->pending.op) {
            sessionFault(*s, 2);
            return;
        }
        if (queueBarrier(seq, session))
            s->pending = {op, seq};
        return;
    }
    // Input behind a pending control reply would make its completion ambiguous.
    if (s->pending.op) {
        sessionFault(*s, 2);
        return;
    }
    s->released = false;
    bool ok = false;
    if (op == Key) {
        InputState state = s->state;
        if (data[4] > 1 || !state.set(read16(data), read16(data + 2), data[4] != 0)) {
            sessionFault(*s, 3);
            return;
        }
        s->state = state;
        ok = queueState(seq, false, session);
    } else if (op == Snapshot) {
        InputState state;
        memcpy(&state, data, 31);
        if (!state.valid()) {
            sessionFault(*s, 3);
            return;
        }
        s->state = state;
        ok = queueState(seq, false, session);
    } else if (op == Buttons) {
        if (data[0] & ~31) {
            sessionFault(*s, 3);
            return;
        }
        s->state.buttons = data[0];
        ok = queueState(seq, false, session);
    } else if (op == Absolute) {
        sessionFault(*s, 3);
        return;
    } else {
        const int16_t a = read16(data), b = read16(data + 2);
        if (a < -1024 || a > 1024 || b < -1024 || b > 1024) {
            sessionFault(*s, 3);
            return;
        }
        ok = op == Move ? queueMotion(a, b, 0, 0, seq, session)
                        : queueMotion(0, 0, a, b, seq, session);
    }
    if (ok) {
        s->lease_at = now_;
    }
}

bool InputEngine::advance(uint32_t now, bool mounted, uint8_t done, bool failed, bool ready,
                          InputReport &report) {
    now_ = now;
    if (!mounted) {
        if (mounted_ || sessions_.count())
            fault(7);
        mounted_ = false;
        output_.disconnect();
        return false;
    }
    if (!mounted_) {
        mounted_ = true;
        output_.release(now);
    }
    if (failed) {
        output_.rejected();
        ++counters_.hid;
        fault(6);
        return false;
    }
    output_.complete(done);
    if (sessions_.count() && output_.stale(now)) {
        fault(8);
        return false;
    }
    Completion completed;
    while (output_.popCompleted(completed))
        if (completed.sequence)
            sessions_.completed(completed.session, completed.sequence);
    return output_.prepare(now, ready, report);
}
void InputEngine::inputRejected(uint32_t now) {
    now_ = now;
    output_.rejected();
    ++counters_.hid;
    fault(6);
}
const wire::Report *InputEngine::vendorReply() const {
    return tx_count_ ? &tx_[tx_head_] : nullptr;
}
void InputEngine::vendorSubmitted() {
    if (!tx_count_)
        return;
    const auto &r = tx_[tx_head_];
    if (r.bytes[1] == (Bootloader | 128) && r.bytes[12] == 0)
        boot_inflight_ = true;
    tx_head_ = (tx_head_ + 1) % 32;
    --tx_count_;
}
void InputEngine::vendorCompleted(uint32_t generation) {
    if (generation == boot_generation_ && boot_requested_ && boot_inflight_) {
        boot_inflight_ = false;
        boot_completed_ = true;
    }
}
void InputEngine::serviceControl(uint32_t now) {
    now_ = now;
    if (boot_completed_) {
        boot_completed_ = false;
        boot_confirmed_ = true;
        notices_ |= BootConfirmed;
    }
    if (!sessions_.count())
        return;
    for (uint8_t i = 0; i < SessionLimit; ++i) {
        auto &s = sessions_.entries()[i];
        if (!s.id)
            continue;
        if (uint32_t(now - s.lease_at) >= 2000) {
            sessionFault(s, 5);
            notices_ |= Failsafe;
            continue;
        }
        if (s.pending.op && s.completed >= s.pending.seq) {
            if (s.pending.op == Release)
                s.released = true;
            result(s.pending.op, s.id, s.pending.seq);
            s.pending = {};
        }
        if (output_.idle() && !s.pending.op)
            s.completed = s.received;
    }
}
} // namespace hidfw
