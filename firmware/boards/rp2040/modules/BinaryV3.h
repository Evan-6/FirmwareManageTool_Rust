#pragma once
namespace binary {
constexpr uint8_t Open = 1, Heartbeat = 2, Status = 3, Barrier = 4, Release = 5, Bootloader = 6,
                  Key = 16, Snapshot = 17, Move = 32, Absolute = 33, Buttons = 34, Wheel = 35,
                  Event = 127;
struct Report {
    uint8_t bytes[63];
};
queue_t rx, tx;
std::atomic<bool> overflow{false}, invalid_report{false}, boot_reply_completed{false};
std::atomic<bool> boot_reply_inflight{false};
struct Pending {
    uint8_t op = 0;
    uint32_t session = 0, seq = 0;
    unsigned long at = 0;
} pending;
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
void event(uint8_t code, uint32_t session, uint32_t seq) {
    // Faults are sticky until a new OPEN; one event per failed session.
    clearReplies();
    reply(Event, session, seq, &code, 1);
}
uint16_t leaseRemaining(unsigned long now) {
    if (protocol_owner != ProtocolOwner::Binary)
        return 0;
    const unsigned long elapsed = now - binary_lease_at;
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
    buffer[8] = 1; // Firmware 3.0.1: single collection owns mouse buttons.
    write16(buffer + 9, 2000);
    write16(buffer + 11, 30000);
    buffer[13] = 1;
    write32(buffer + 14, 15);
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
void status(uint32_t session, uint32_t seq) {
    uint8_t p[51] = {};
    write32(p + 1, received_sequence);
    write32(p + 5, completed_sequence);
    p[9] = output.pending();
    write16(p + 10, leaseRemaining(millis()));
    write16(p + 12, binary_rx_errors);
    write16(p + 14, binary_tx_errors);
    write16(p + 16, binary_hid_errors);
    write16(p + 18, binary_failsafe_count);
    memcpy(p + 20, &output.desired, 31);
    reply(Status | 0x80, session, seq, p, 51);
}
void handle(const Report &report) {
    const uint8_t *p = report.bytes;
    const uint8_t op = p[1], n = p[2];
    const uint32_t session = read32(p + 4), seq = read32(p + 8);
    bool valid = p[0] == 3 && p[3] == 0 && n <= 51 && shape(op, n);
    if (valid)
        for (uint8_t i = 12 + n; i < 63; ++i)
            if (p[i])
                valid = false;
    if (!valid) {
        ++binary_rx_errors;
        if (protocol_owner == ProtocolOwner::Binary && session == active_session)
            protocolFault(1);
        return;
    }
    if (op == Open) {
        if (!session || seq != 1) {
            result(op, session, seq, 1);
            return;
        }
        const bool handoff = (protocol_owner == ProtocolOwner::Legacy && !output.hasPressed()) ||
                             (protocol_owner == ProtocolOwner::Binary && binary_released);
        if ((protocol_owner != ProtocolOwner::None && !handoff) || !output.idle() || pending.op) {
            result(op, session, seq, 2);
            return;
        }
        clearReplies();
        binary_released = false;
        active_session = session;
        received_sequence = 1;
        completed_sequence = 0;
        protocol_owner = ProtocolOwner::Binary;
        binary_lease_at = millis();
        output.release(seq);
        pending = {op, session, seq, millis()};
        return;
    }
    if (protocol_owner != ProtocolOwner::Binary || session != active_session) {
        return;
    }
    if (seq <= received_sequence) {
        ++binary_rx_errors;
        protocolFault(9);
        return;
    }
    if (seq != received_sequence + 1) {
        ++binary_rx_errors;
        protocolFault(10);
        return;
    }
    received_sequence = seq;
    const uint8_t *data = p + 12;
    if (op == Status) {
        status(session, seq);
        return;
    }
    if (op == Heartbeat) {
        binary_lease_at = millis();
        return;
    }
    if (op == Release || op == Bootloader) {
        if (pending.op) {
            protocolFault(2);
            return;
        }
        output.release(seq);
        mouse_report.buttons = 0;
        pending = {op, session, seq, millis()};
        binary_lease_at = millis();
        return;
    }
    if (op == Barrier) {
        if (pending.op) {
            protocolFault(2);
            return;
        }
        if (output.barrier(seq))
            pending = {op, session, seq, millis()};
        return;
    }
    binary_released = false;
    bool ok = false;
    if (op == Key) {
        InputState state = output.desired;
        if (data[4] > 1 || !state.set(read16(data), read16(data + 2), data[4] != 0)) {
            protocolFault(3);
            return;
        }
        ok = output.state(state, seq);
    } else if (op == Snapshot) {
        InputState state;
        memcpy(&state, data, 31);
        if (!state.valid()) {
            protocolFault(3);
            return;
        }
        ok = output.state(state, seq);
        mouse_report.buttons = state.buttons;
    } else if (op == Buttons) {
        if (data[0] & ~31) {
            protocolFault(3);
            return;
        }
        InputState state = output.desired;
        state.buttons = data[0];
        ok = output.state(state, seq);
        mouse_report.buttons = data[0];
    } else if (op == Absolute)
        ok = output.absolute(read16(data), read16(data + 2), seq);
    else {
        const int16_t a = read16(data), b = read16(data + 2);
        if (a < -1024 || a > 1024 || b < -1024 || b > 1024) {
            protocolFault(3);
            return;
        }
        ok = op == Move ? output.motion(a, b, 0, 0, seq) : output.motion(0, 0, a, b, seq);
    }
    if (ok)
        binary_lease_at = millis();
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
        boot_reply_inflight = r.bytes[1] == (Bootloader | 0x80);
        queue_try_remove(&tx, &r);
    }
}
void serviceControl() {
    if (boot_reply_completed.exchange(false))
        scheduleBootloaderReset();
    if (protocol_owner != ProtocolOwner::Binary)
        return;
    if (millis() - binary_lease_at >= 2000) {
        protocolFault(5);
        led::signalFailsafe();
        return;
    }
    if (pending.op && completed_sequence >= pending.seq) {
        if (pending.op == Release)
            binary_released = true;
        result(pending.op, pending.session, pending.seq);
        pending = {};
    }
    if (output.idle() && !pending.op)
        completed_sequence = received_sequence;
}
} // namespace binary
void protocolFault(uint8_t code) {
    const auto owner = protocol_owner;
    if (owner == ProtocolOwner::Binary)
        ++binary_failsafe_count;
    else if (owner == ProtocolOwner::Legacy)
        ++failsafe_count;
    const uint32_t session = active_session, seq = received_sequence;
    binary::pending = {};
    binary_released = false;
    binary::Report stale;
    while (queue_try_remove(&binary::rx, &stale)) {
    }
    binary::overflow.store(false);
    binary::invalid_report.store(false);
    output.release();
    protocol_owner = ProtocolOwner::None;
    active_session = 0;
    mouse_report.buttons = 0;
    led::signalError();
    RxEvent event;
    while (rxPop(event)) {
    }
    resetParser(vendor_parser);
    if (owner == ProtocolOwner::Binary)
        binary::event(code, session, seq);
    else if (owner == ProtocolOwner::Legacy)
        QUEUE_ERROR("err:hid_send_failed\n");
}
