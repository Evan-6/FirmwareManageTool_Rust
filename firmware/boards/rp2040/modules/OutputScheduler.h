#pragma once
// All HID output is scheduled on core 0. Only completion notifications cross callbacks.
class OutputScheduler {
  public:
    enum : uint8_t { Keyboard = 1, Consumer = 2, Relative = 4 };
    struct Job {
        InputState state{};
        InputState sources[binary::SessionLimit]{};
        int32_t x = 0, y = 0, wheel = 0, hwheel = 0;
        uint32_t sequence = 0, session = 0;
        unsigned long queued_at = 0;
        uint8_t flags = 0;
        uint8_t kind = 0; // 0 state/barrier, 1 relative
    };
    InputState desired{};
    InputState completed{};
    uint8_t pending() const { return count_; }
    bool idle() const { return count_ == 0 && !in_flight_; }
    bool hasPressed() const { return desired.held(); }
    bool state(const InputState &next, uint32_t seq = 0, bool force = false, uint32_t session = 0) {
        Job j;
        j.state = next;
        j.sequence = seq;
        j.session = session;
        binary::capture(j.sources);
        if (force || next.modifiers != desired.modifiers || memcmp(next.keys, desired.keys, 28))
            j.flags |= Keyboard;
        if (force || next.consumer != desired.consumer)
            j.flags |= Consumer;
        // One relative mouse owns movement, all five buttons and both wheels.
        if (force || next.buttons != desired.buttons)
            j.flags |= Relative;
        if (!add(j))
            return false;
        desired = next;
        return true;
    }
    bool motion(int32_t x, int32_t y, int32_t wheel, int32_t hwheel, uint32_t seq = 0, uint32_t session = 0) {
        // Only combine adjacent pending motion, never the in-flight head or across
        // buttons/barriers.
        if (count_ > 1) {
            Job &last = jobs_[(head_ + count_ - 1) % Depth];
            if (last.kind == 1 && last.session == session && last.state.buttons == desired.buttons) {
                const int64_t nx = int64_t(last.x) + x, ny = int64_t(last.y) + y,
                              nw = int64_t(last.wheel) + wheel, nh = int64_t(last.hwheel) + hwheel;
                if (nx < -8192 || nx > 8192 || ny < -8192 || ny > 8192 || nw < -8192 || nw > 8192 ||
                    nh < -8192 || nh > 8192) {
                    protocolFault(8);
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
        Job j;
        j.state = desired;
        j.kind = 1;
        j.flags = Relative;
        j.x = x;
        j.y = y;
        j.wheel = wheel;
        j.hwheel = hwheel;
        j.sequence = seq;
        j.session = session;
        binary::capture(j.sources);
        return add(j);
    }
    bool barrier(uint32_t seq, uint32_t session = 0) {
        Job j;
        j.state = desired;
        j.sequence = seq;
        j.session = session;
        binary::capture(j.sources);
        return add(j);
    }
    void release(uint32_t seq = 0) {
        if (protocol_owner != ProtocolOwner::None)
            releasing_protocol = protocol_owner;
        // Keep the old in-flight transfer alive but ignore its generation when it completes.
        ++generation_;
        head_ = 0;
        count_ = 0;
        desired = InputState{};
        Job j;
        j.state = desired;
        j.sequence = seq;
        j.flags = Keyboard | Consumer | Relative;
        j.queued_at = millis();
        jobs_[0] = j;
        count_ = 1;
    }
    // Remove only this client's queued work. Historical snapshots of the other
    // clients preserve their short taps and barriers while removing stale holds.
    bool releaseSession(uint32_t session, uint32_t seq, uint8_t slot) {
        releasing_protocol = ProtocolOwner::Binary;
        const bool preserve_inflight = in_flight_ && count_ && jobs_[head_].session != session;
        if (!preserve_inflight)
            ++generation_; // Ignore completion only when its owner is being cancelled.
        const uint8_t old_count = count_;
        uint8_t kept = 0;
        InputState previous = completed;
        for (uint8_t i = 0; i < old_count; ++i) {
            Job j = jobs_[(head_ + i) % Depth];
            if (j.session == session)
                continue;
            const uint8_t original_flags = j.flags;
            j.sources[slot] = {};
            j.state = binary::merge(j.sources);
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
        desired = previous;
        return state(binary::aggregate(), seq, true, session);
    }
    void complete(uint8_t report_id) { completion_.store(report_id, std::memory_order_release); }
    void failed() { failed_.store(true, std::memory_order_release); }
    void flush(unsigned long now, bool mounted) {
        if (!mounted) {
            if (mounted_ || protocol_owner != ProtocolOwner::None)
                protocolFault(7);
            mounted_ = false;
            in_flight_ = false;
            completion_.store(0);
            return;
        }
        if (!mounted_) {
            mounted_ = true;
            release();
        }
        if (failed_.exchange(false)) {
            ++binary_hid_errors;
            protocolFault(6);
            return;
        }
        const uint8_t done = completion_.exchange(0, std::memory_order_acq_rel);
        if (done && in_flight_) {
            in_flight_ = false;
            if (inflight_generation_ == generation_ && count_) {
                Job &j = jobs_[head_];
                if (done == KeyboardReportId)
                    j.flags &= ~Keyboard;
                else if (done == ConsumerReportId)
                    j.flags &= ~Consumer;
                else if (done == MouseReportId) {
                    j.x -= sent_x_;
                    j.y -= sent_y_;
                    j.wheel -= sent_wheel_;
                    j.hwheel -= sent_hwheel_;
                    if (!j.x && !j.y && !j.wheel && !j.hwheel)
                        j.flags &= ~Relative;
                }
            }
        }
        if (count_ && now - jobs_[head_].queued_at > 50 && protocol_owner != ProtocolOwner::None) {
            protocolFault(8);
            return;
        }
        if (in_flight_)
            return;
        while (count_ && jobs_[head_].flags == 0) {
            Job &j = jobs_[head_];
            completed = j.state;
            if (j.sequence)
                binary::completed(j.session, j.sequence);
            head_ = (head_ + 1) % Depth;
            --count_;
        }
        if (!count_ || !usb_hid.ready() || now - last_send_at_ < 1)
            return;
        Job &j = jobs_[head_];
        uint8_t data[29] = {};
        uint8_t id = 0, n = 0;
        if (j.flags & Keyboard) {
            id = KeyboardReportId;
            n = 29;
            data[0] = j.state.modifiers;
            memcpy(data + 1, j.state.keys, 28);
        } else if (j.flags & Consumer) {
            id = ConsumerReportId;
            n = 1;
            data[0] = j.state.consumer;
        } else if (j.flags & Relative) {
            id = MouseReportId;
            n = 5;
            data[0] = j.state.buttons;
            sent_x_ = clamp(j.x);
            sent_y_ = clamp(j.y);
            sent_wheel_ = clamp(j.wheel);
            sent_hwheel_ = clamp(j.hwheel);
            data[1] = sent_x_;
            data[2] = sent_y_;
            data[3] = sent_wheel_;
            data[4] = sent_hwheel_;
        }
        last_send_at_ = now;
        in_flight_ = true;
        inflight_generation_ = generation_;
        if (!usb_hid.sendReport(id, data, n)) {
            in_flight_ = false;
            ++binary_hid_errors;
            protocolFault(6);
        }
    }

  private:
    static constexpr uint8_t Depth = 32;
    Job jobs_[Depth]{};
    uint8_t head_ = 0, count_ = 0;
    unsigned long last_send_at_ = 0;
    uint32_t generation_ = 0, inflight_generation_ = 0;
    bool in_flight_ = false, mounted_ = false;
    int8_t sent_x_ = 0, sent_y_ = 0, sent_wheel_ = 0, sent_hwheel_ = 0;
    std::atomic<uint8_t> completion_{0};
    std::atomic<bool> failed_{false};
    static int8_t clamp(int32_t n) { return n < -127 ? -127 : n > 127 ? 127 : n; }
    bool add(Job j) {
        if (count_ == Depth) {
            protocolFault(8);
            return false;
        }
        j.queued_at = millis();
        jobs_[(head_ + count_) % Depth] = j;
        ++count_;
        return true;
    }
};
OutputScheduler output;
