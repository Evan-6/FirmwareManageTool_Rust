// RP2040 internal module; included once by Firmware.cpp.
struct MouseReport {
    uint8_t buttons;
    int8_t x;
    int8_t y;
    int8_t wheel;
    int8_t hwheel;
};

MouseReport mouse_report{};
uint16_t mouse_send_failures = 0;

// Absolute pointer report (report id AbsMouseReportId). The button byte stays
// zero: all button transitions belong to the relative mouse collection, so
// absolute motion during a drag cannot generate an extra press/release.
struct __attribute__((packed)) AbsMouseReport {
    uint8_t buttons;
    uint16_t x;
    uint16_t y;
};

uint16_t abs_mouse_x = 0;
uint16_t abs_mouse_y = 0;
bool abs_mouse_known = false;

// Per-command delta bound: keeps the chunked send loop below to a handful of
// reports so a hostile/buggy host cannot stall core 0's protocol loop.
constexpr int32_t MaxMouseDeltaPerCommand = 1024;

// Legacy commands enqueue the same output engine as v3; no endpoint waits.
bool sendMouseDelta(int32_t x, int32_t y, int32_t wheel, int32_t hwheel) {
    InputState next = output.desired;
    next.buttons = mouse_report.buttons;
    if (next.buttons != output.desired.buttons && !output.state(next))
        return false;
    return output.motion(x, y, wheel, hwheel);
}
bool sendAbsMouse(uint16_t x, uint16_t y) {
    abs_mouse_x = x;
    abs_mouse_y = y;
    abs_mouse_known = true;
    return output.absolute(x, y);
}
bool updateMouseButtons(uint16_t flags, uint32_t data) {
    if (flags & 0x0002)
        mouse_report.buttons |= 1;
    if (flags & 0x0004)
        mouse_report.buttons &= ~1;
    if (flags & 0x0008)
        mouse_report.buttons |= 2;
    if (flags & 0x0010)
        mouse_report.buttons &= ~2;
    if (flags & 0x0020)
        mouse_report.buttons |= 4;
    if (flags & 0x0040)
        mouse_report.buttons &= ~4;
    if (flags & 0x0080)
        mouse_report.buttons |= (data == 1 ? 8 : 16);
    if (flags & 0x0100)
        mouse_report.buttons &= ~(data == 1 ? 8 : 16);
    InputState next = output.desired;
    next.buttons = mouse_report.buttons;
    return output.state(next);
}
bool releaseAllMouseButtons() {
    mouse_report.buttons = 0;
    return true;
}
