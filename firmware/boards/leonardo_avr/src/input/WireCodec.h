// Generated from firmware/common/input/WireCodec.h; do not edit this copy.
#pragma once
#include "InputState.h"
namespace hidfw::wire {
constexpr uint8_t Open = 1, Heartbeat = 2, Status = 3, Barrier = 4, Release = 5, Bootloader = 6,
                  Key = 16, Snapshot = 17, Move = 32, Absolute = 33, Buttons = 34, Wheel = 35,
                  Event = 127;
constexpr uint8_t KeyboardReportId = 2, ConsumerReportId = 5, MouseReportId = 3;
struct Report {
    uint8_t bytes[63]{};
};
struct DeviceInfo {
    uint8_t board = 1;
    uint8_t id[8]{};
};
uint16_t read16(const uint8_t *p);
uint32_t read32(const uint8_t *p);
void write16(uint8_t *p, uint16_t v);
void write32(uint8_t *p, uint32_t v);
bool valid(const Report &report);
Report reply(uint8_t op, uint32_t session, uint32_t seq, const uint8_t *data, uint8_t n);
void feature(const DeviceInfo &info, uint8_t *buffer);
} // namespace hidfw::wire
