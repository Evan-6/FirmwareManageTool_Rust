#pragma once
// All HID output is scheduled on core 0. Only completion notifications cross callbacks.
class OutputScheduler {
  public:
    enum : uint8_t { Keyboard = 1, Consumer = 2, Relative = 4, Absolute = 8 };
    struct Job {
        InputState state{};
        int32_t x = 0, y = 0, wheel = 0, hwheel = 0;
        uint16_t abs_x = 0, abs_y = 0;
        uint32_t sequence = 0;
        unsigned long queued_at = 0;
        uint8_t flags = 0;
        uint8_t kind = 0; // 0 state/barrier, 1 relative, 2 absolute
    };
    InputState desired{};
    InputState completed{};
    uint8_t pending() const { return count_; }
    bool idle() const { return count_ == 0 && !in_flight_; }
    bool hasPressed() const { return desired.held(); }
    bool state(const InputState &next, uint32_t seq = 0, bool force = false) {
        Job j;
        j.state = next;
        j.sequence = seq;
        if (force || next.modifiers != desired.modifiers || memcmp(next.keys, desired.keys, 28))
            j.flags |= Keyboard;
        if (force || next.consumer != desired.consumer)
            j.flags |= Consumer;
        // Windows exposes each top-level mouse collection as a separate device.
        // Keep all button transitions in the relative collection, even while
        // moving absolutely; mirroring buttons would generate duplicate clicks.
        if (force || next.buttons != desired.buttons)
            j.flags |= Relative;
        if (!add(j))
            return false;
        desired = next;
        return true;
    }
    bool motion(int32_t x, int32_t y, int32_t wheel, int32_t hwheel, uint32_t seq = 0) {
        // Only combine adjacent pending motion, never the in-flight head or across
        // buttons/barriers.
        if (count_ > 1) {
            Job &last = jobs_[(head_ + count_ - 1) % Depth];
            if (last.kind == 1 && last.state.buttons == desired.buttons) {
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
        return add(j);
    }
    bool absolute(uint16_t x, uint16_t y, uint32_t seq = 0) {
        if (count_ > 1) {
            Job &last = jobs_[(head_ + count_ - 1) % Depth];
            if (last.kind == 2 && last.state.buttons == desired.buttons) {
                last.abs_x = x;
                last.abs_y = y;
                last.sequence = seq;
                return true;
            }
        }
        Job j;
        j.state = desired;
        j.kind = 2;
        j.flags = Absolute;
        j.abs_x = x;
        j.abs_y = y;
        j.sequence = seq;
        return add(j);
    }
    bool barrier(uint32_t seq) {
        Job j;
        j.state = desired;
        j.sequence = seq;
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
        j.flags = Keyboard | Consumer | Relative | Absolute;
        j.queued_at = millis();
        jobs_[0] = j;
        count_ = 1;
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
                } else if (done == AbsMouseReportId)
                    j.flags &= ~Absolute;
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
                completed_sequence = j.sequence;
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
        } else if (j.flags & Absolute) {
            if (j.kind == 2) {
                abs_x_ = j.abs_x;
                abs_y_ = j.abs_y;
                abs_known_ = true;
            }
            if (!abs_known_) {
                j.flags &= ~Absolute;
                return;
            }
            id = AbsMouseReportId;
            n = 5;
            // The absolute collection owns coordinates only. Sending held buttons
            // here would create a second press on the first absolute drag motion.
            data[0] = 0;
            data[1] = abs_x_;
            data[2] = abs_x_ >> 8;
            data[3] = abs_y_;
            data[4] = abs_y_ >> 8;
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
    bool in_flight_ = false, mounted_ = false, abs_known_ = false;
    uint16_t abs_x_ = 0, abs_y_ = 0;
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
