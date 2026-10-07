#pragma once
namespace binary {
constexpr uint8_t Open = 1, Heartbeat = 2, Status = 3, Barrier = 4, Release = 5, Bootloader = 6,
                  Key = 16, Snapshot = 17, Move = 32, Absolute = 33, Buttons = 34, Wheel = 35,
                  Event = 127;
struct Report {
    uint8_t bytes[63];
};
queue_t rx, tx;
bool boot_requested = false;
std::atomic<bool> overflow{false}, invalid_report{false}, boot_reply_completed{false};
std::atomic<bool> boot_reply_inflight{false};
uint16_t read16(const uint8_t *p) { return uint16_t(p[0]) | (uint16_t(p[1]) << 8); }
uint32_t read32(const uint8_t *p) {
    return uint32_t(p[0]) | (uint32_t(p[1]) << 8) | (uint32_t(p[2]) << 16) | (uint32_t(p[3]) << 24);
}
void write16(uint8_t *p, uint16_t v) {
    p[0] = v;
    p[1] = v >> 8;
}
void write32(uint8_t *p, uint32_t v) {
    for (uint8_t i = 0; i < 4; ++i)
        p[i] = v >> (8 * i);
}
void reply(uint8_t op, uint32_t session, uint32_t seq, const uint8_t *data, uint8_t n) {
    Report r{};
    r.bytes[0] = 3;
    r.bytes[1] = op;
    r.bytes[2] = n;
    write32(r.bytes + 4, session);
    write32(r.bytes + 8, seq);
    memcpy(r.bytes + 12, data, n);
    if (!queue_try_add(&tx, &r)) {
        ++binary_tx_errors;
        overflow.store(true);
    }
}
void result(uint8_t op, uint32_t session, uint32_t seq, uint8_t code = 0) {
    reply(op | 0x80, session, seq, &code, 1);
}
void clearReplies() {
    Report r;
    while (queue_try_remove(&tx, &r)) {
    }
}
void discardReplies(uint32_t session) {
    // TX is only mutated on core 0. Rotate exactly the initial depth, preserving peers' order.
    const uint8_t depth = queue_get_level(&tx);
    Report r;
    for (uint8_t i = 0; i < depth && queue_try_remove(&tx, &r); ++i)
        if (read32(r.bytes + 4) != session)
            queue_try_add(&tx, &r);
}
void event(uint8_t code, uint32_t session, uint32_t seq) {
    // Faults are sticky until a new OPEN; one event per failed session.
    reply(Event, session, seq, &code, 1);
}
uint16_t leaseRemaining(const Session &s, unsigned long now) {
    if (protocol_owner != ProtocolOwner::Binary)
        return 0;
    const unsigned long elapsed = now - s.lease_at;
    return elapsed >= 2000 ? 0 : 2000 - elapsed;
}
uint16_t feature(uint8_t id, hid_report_type_t type, uint8_t *buffer, uint16_t n) {
    if (id != 14 || type != HID_REPORT_TYPE_FEATURE || n < 63)
        return 0;
    memset(buffer, 0, 63);
    memcpy(buffer, "FMT3", 4);
    buffer[4] = 3;
#if defined(ARDUINO_SEEED_XIAO_RP2040)
    buffer[5] = 2;
#else
    buffer[5] = 1;
#endif
    buffer[6] = 3;
    buffer[8] = 3; // Firmware 3.0.3: concurrent v3 clients.
    write16(buffer + 9, 2000);
    write16(buffer + 11, 30000);
    buffer[13] = 1;
    write32(buffer + 14, 23); // NKRO, Consumer, relative mouse, concurrent sessions.
    for (uint16_t u = 4; u < 224; ++u)
        if (key_catalog::keyboardSupported(u))
            buffer[18 + u / 8] |= 1u << (u % 8);
    buffer[46] = 255;
    buffer[47] = 127;
    pico_unique_board_id_t uid;
    pico_get_unique_board_id(&uid);
    memcpy(buffer + 48, uid.id, 8);
    return 63;
}
bool shape(uint8_t op, uint8_t n) {
    switch (op) {
    case Open:
    case Heartbeat:
    case Status:
    case Barrier:
    case Release:
    case Bootloader:
        return n == 0;
    case Key:
        return n == 5;
    case Snapshot:
        return n == 31;
    case Move:
    case Absolute:
    case Wheel:
        return n == 4;
    case Buttons:
        return n == 1;
    default:
        return false;
    }
}
void status(const Session &s, uint32_t seq) {
    uint8_t p[51] = {};
    write32(p + 1, s.received);
    write32(p + 5, s.completed);
    p[9] = output.pending();
    write16(p + 10, leaseRemaining(s, millis()));
    write16(p + 12, binary_rx_errors);
    write16(p + 14, binary_tx_errors);
    write16(p + 16, binary_hid_errors);
    write16(p + 18, binary_failsafe_count);
    memcpy(p + 20, &s.state, 31); // Client-local state: RELEASE must not test another client's holds.
    reply(Status | 0x80, s.id, seq, p, 51);
}
void sessionFault(Session &s, uint8_t code) {
    const uint32_t id = s.id, seq = s.received;
    const uint8_t slot = &s - sessions;
    discardReplies(id);
    if (boot_requested) {
        boot_requested = false;
        boot_reply_inflight = false;
        boot_reply_completed = false;
    }
    s = {};
    ++binary_failsafe_count;
    output.releaseSession(id, 0, slot);
    if (!count())
        protocol_owner = ProtocolOwner::None;
    mouse_report.buttons = output.desired.buttons;
    event(code, id, seq);
    led::signalError();
}
void handle(const Report &report) {
    const uint8_t *p = report.bytes;
    const uint8_t op = p[1], n = p[2];
    const uint32_t session = read32(p + 4), seq = read32(p + 8);
    Session *s = find(session);
    if (s && millis() - s->lease_at >= 2000) {
        sessionFault(*s, 5);
        s = nullptr;
    }
    bool valid = p[0] == 3 && p[3] == 0 && n <= 51 && shape(op, n);
    if (valid)
        for (uint8_t i = 12 + n; i < 63; ++i)
            if (p[i])
                valid = false;
    if (!valid) {
        ++binary_rx_errors;
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
        if (s || boot_requested || bootloader_reset_pending ||
            (protocol_owner == ProtocolOwner::Legacy && (output.hasPressed() || !output.idle())) ||
            (protocol_owner == ProtocolOwner::None && !output.idle())) {
            result(op, session, seq, 2);
            return;
        }
        for (auto &candidate : sessions)
            if (!candidate.id) {
                s = &candidate;
                break;
            }
        if (!s) {
            result(op, session, seq, 2);
            return;
        }
        *s = {};
        s->id = session;
        s->received = 1;
        s->lease_at = millis();
        protocol_owner = ProtocolOwner::Binary;
        binary_released = false;
        if (output.state(aggregate(), seq, count() == 1, session))
            s->pending = {op, seq};
        return;
    }
    if (protocol_owner != ProtocolOwner::Binary || !s)
        return;
    if (seq <= s->received || seq != s->received + 1) {
        ++binary_rx_errors;
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
        s->lease_at = millis();
        return;
    }
    if (boot_requested)
        return; // A successful boot request has committed to an empty input state.
    if (op == Release || op == Bootloader) {
        if (s->pending.op) {
            sessionFault(*s, 2);
            return;
        }
        if (op == Bootloader && count() > 1) {
            result(op, session, seq, 2);
            return;
        }
        if (op == Bootloader)
            boot_requested = true;
        s->state = {};
        s->released = false;
        if (output.releaseSession(session, seq, s - sessions))
            s->pending = {op, seq};
        mouse_report.buttons = output.desired.buttons;
        s->lease_at = millis();
        return;
    }
    if (op == Barrier) {
        if (s->pending.op) {
            sessionFault(*s, 2);
            return;
        }
        if (output.barrier(seq, session))
            s->pending = {op, seq};
        return;
    }
    // Input behind a pending control reply would make its completion ambiguous.
    if (s->pending.op) {
        sessionFault(*s, 2);
        return;
    }
    s->released = false;
    binary_released = false;
    bool ok = false;
    if (op == Key) {
        InputState state = s->state;
        if (data[4] > 1 || !state.set(read16(data), read16(data + 2), data[4] != 0)) {
            sessionFault(*s, 3);
            return;
        }
        s->state = state;
        ok = output.state(aggregate(), seq, false, session);
    } else if (op == Snapshot) {
        InputState state;
        memcpy(&state, data, 31);
        if (!state.valid()) {
            sessionFault(*s, 3);
            return;
        }
        s->state = state;
        ok = output.state(aggregate(), seq, false, session);
    } else if (op == Buttons) {
        if (data[0] & ~31) {
            sessionFault(*s, 3);
            return;
        }
        s->state.buttons = data[0];
        ok = output.state(aggregate(), seq, false, session);
    } else if (op == Absolute) {
        sessionFault(*s, 3);
        return;
    } else {
        const int16_t a = read16(data), b = read16(data + 2);
        if (a < -1024 || a > 1024 || b < -1024 || b > 1024) {
            sessionFault(*s, 3);
            return;
        }
        ok = op == Move ? output.motion(a, b, 0, 0, seq, session)
                        : output.motion(0, 0, a, b, seq, session);
    }
    if (ok) {
        s->lease_at = millis();
        mouse_report.buttons = output.desired.buttons;
    }
}
void serviceRx() {
    if (invalid_report.exchange(false)) {
        protocolFault(1);
        return;
    }
    if (overflow.exchange(false)) {
        protocolFault(8);
        Report r;
        while (queue_try_remove(&rx, &r)) {
        }
        return;
    }
    Report r;
    for (uint8_t budget = 0; budget < 4 && queue_try_remove(&rx, &r); ++budget)
        handle(r);
}
void serviceTx() {
    if (!TinyUSBDevice.mounted() || !usb_vendor.ready())
        return;
    Report r;
    if (!queue_try_peek(&tx, &r))
        return;
    if (usb_vendor.sendReport(13, r.bytes, 63)) {
        boot_reply_inflight = r.bytes[1] == (Bootloader | 0x80) && r.bytes[12] == 0;
        queue_try_remove(&tx, &r);
    }
}
void serviceControl() {
    if (boot_reply_completed.exchange(false))
        scheduleBootloaderReset();
    if (protocol_owner != ProtocolOwner::Binary)
        return;
    binary_released = true;
    for (auto &s : sessions) {
        if (!s.id)
            continue;
        if (millis() - s.lease_at >= 2000) {
            sessionFault(s, 5);
            led::signalFailsafe();
            continue;
        }
        if (s.pending.op && s.completed >= s.pending.seq) {
            if (s.pending.op == Release)
                s.released = true;
            result(s.pending.op, s.id, s.pending.seq);
            s.pending = {};
        }
        if (!s.released || s.pending.op)
            binary_released = false;
        if (output.idle() && !s.pending.op)
            s.completed = s.received;
    }
}
} // namespace binary
void protocolFault(uint8_t code) {
    const auto owner = protocol_owner;
    // Shared USB/queue faults invalidate every client; per-client errors use sessionFault.
    if (owner == ProtocolOwner::Legacy)
        ++failsafe_count;
    binary_released = false;
    binary::boot_requested = false;
    binary::boot_reply_inflight = false;
    binary::boot_reply_completed = false;
    binary::Report stale;
    while (queue_try_remove(&binary::rx, &stale)) {
    }
    binary::overflow.store(false);
    binary::invalid_report.store(false);
    output.release();
    protocol_owner = ProtocolOwner::None;
    mouse_report.buttons = 0;
    led::signalError();
    RxEvent event;
    while (rxPop(event)) {
    }
    resetParser(vendor_parser);
    binary::clearReplies();
    if (owner == ProtocolOwner::Binary)
        for (const auto &s : binary::sessions)
            if (s.id) {
                ++binary_failsafe_count;
                binary::event(code, s.id, s.received);
            }
    binary::resetSessions();
    if (owner == ProtocolOwner::Legacy)
        QUEUE_ERROR("err:hid_send_failed\n");
}
