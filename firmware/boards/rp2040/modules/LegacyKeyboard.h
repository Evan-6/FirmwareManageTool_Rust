#pragma once
// A six-key adapter preserves protocol=2 limits; HID output itself is NKRO.
class KeyboardEngine {
  public:
    void begin() {}
    KeyboardReport report() const {
        KeyboardReport r{};
        r.modifiers = output.desired.modifiers;
        uint8_t n = 0;
        for (uint16_t u = 4; u < 224 && n < 6; ++u)
            if (output.desired.keys[u / 8] & (1u << (u % 8)))
                r.keys[n++] = u;
        return r;
    }
    bool hasPressedKeys() const { return output.hasPressed(); }
    uint16_t sendFailures() const { return binary_hid_errors; }
    ApplyResult press(const KeySpec &k) {
        KeyboardReport r = report();
        if (k.modifier_mask) {
            if (r.modifiers & k.modifier_mask)
                return ApplyResult::AlreadyDesired;
            r.modifiers |= k.modifier_mask;
            return applySnapshot(r);
        }
        for (uint8_t u : r.keys)
            if (u == k.usage)
                return ApplyResult::AlreadyDesired;
        for (uint8_t &u : r.keys)
            if (!u) {
                u = k.usage;
                return applySnapshot(r);
            }
        return ApplyResult::ReportFull;
    }
    ApplyResult release(const KeySpec &k) {
        KeyboardReport r = report();
        if (k.modifier_mask)
            r.modifiers &= ~k.modifier_mask;
        else
            for (uint8_t &u : r.keys)
                if (u == k.usage)
                    u = 0;
        return applySnapshot(r);
    }
    ApplyResult applySnapshot(const KeyboardReport &r) {
        InputState next = output.desired;
        next.modifiers = r.modifiers;
        memset(next.keys, 0, 28);
        for (uint8_t u : r.keys)
            if (u)
                next.keys[u / 8] |= 1u << (u % 8);
        if (memcmp(&next, &output.desired, sizeof(next)) == 0)
            return ApplyResult::AlreadyDesired;
        return output.state(next) ? ApplyResult::Applied : ApplyResult::TransportFailed;
    }
    ApplyResult releaseAll() {
        output.release();
        return ApplyResult::Applied;
    }
};
KeyboardEngine keyboard;
