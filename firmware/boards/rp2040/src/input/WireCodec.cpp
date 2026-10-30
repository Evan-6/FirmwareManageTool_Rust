#include "WireCodec.h"
namespace hidfw::wire {
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

bool valid(const Report &report) {
    const uint8_t *p = report.bytes;
    const uint8_t n = p[2];
    if (p[0] != 3 || p[3] != 0 || n > 51 || !shape(p[1], n))
        return false;
    for (uint8_t i = 12 + n; i < 63; ++i)
        if (p[i])
            return false;
    return true;
}
Report reply(uint8_t op, uint32_t session, uint32_t seq, const uint8_t *data, uint8_t n) {
    Report r{};
    r.bytes[0] = 3;
    r.bytes[1] = op;
    r.bytes[2] = n;
    write32(r.bytes + 4, session);
    write32(r.bytes + 8, seq);
    memcpy(r.bytes + 12, data, n);
    return r;
}
void feature(const DeviceInfo &info, uint8_t *buffer) {
    memset(buffer, 0, 63);
    memcpy(buffer, "FMT3", 4);
    buffer[4] = 3;
    buffer[5] = info.board;
    buffer[6] = 3;
    buffer[7] = 1;
    write16(buffer + 9, 2000);
    buffer[13] = 1;
    write32(buffer + 14, 23);
    for (uint16_t u = 4; u < 224; ++u)
        if (key_catalog::keyboardSupported(u))
            buffer[18 + u / 8] |= 1u << (u % 8);
    buffer[46] = 255;
    buffer[47] = 127;
    memcpy(buffer + 48, info.id, 8);
}
} // namespace hidfw::wire
