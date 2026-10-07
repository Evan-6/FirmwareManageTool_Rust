#include "StatusLedRenderer.h"
#include <Arduino.h>
namespace hidfw {
using namespace board;
#if FIRMWARE_ENABLE_NEOPIXEL
namespace {
// --- renderer tunables --------------------------------------------------------
constexpr uint8_t MaxBrightness = 160;
constexpr uint16_t FrameIntervalMs = 10;
constexpr uint8_t FadeStep = 18;
constexpr uint16_t IdleBreathPeriodMs = 4000;
constexpr uint8_t IdleBreathMin = 2;
constexpr uint8_t IdleBreathMax = 24;
constexpr uint16_t FailsafeBlinkMs = 400;
constexpr uint16_t BootBlinkMs = 60;

constexpr Rgb ColourOff = {0, 0, 0};
constexpr Rgb ColourHeld = {0, 160, 255};
constexpr Rgb ColourIdle = {0, 40, 255};
constexpr Rgb ColourError = {255, 20, 0};
constexpr Rgb ColourFailsafe = {255, 120, 0};
constexpr Rgb ColourBoot = {200, 0, 255};

uint8_t scale8(uint8_t value, uint8_t scale) {
    return static_cast<uint8_t>((static_cast<uint16_t>(value) * scale) >> 8);
}

Rgb scaleRgb(const Rgb &colour, uint8_t scale) {
    return Rgb{scale8(colour.r, scale), scale8(colour.g, scale), scale8(colour.b, scale)};
}

// Gamma-ish triangle breathing: 0 -> 255 -> 0 across `period`, squared so the
// low end lingers (the eye reads that as a smooth breath rather than a ramp).
uint8_t breath(unsigned long now, uint16_t period) {
    const uint16_t pos = static_cast<uint16_t>(now % period);
    const uint16_t half = static_cast<uint16_t>(period / 2);
    const uint16_t tri =
        pos < half ? static_cast<uint16_t>((static_cast<uint32_t>(pos) * 255U) / half)
                   : static_cast<uint16_t>((static_cast<uint32_t>(period - pos) * 255U) / half);
    return static_cast<uint8_t>((tri * tri) >> 8);
}

bool blinkOn(unsigned long now, uint16_t period) { return (now % period) < (period / 2); }

Rgb targetColour(LedState state, unsigned long now, bool &instant) {
    instant = false;

    switch (state) {
    case LedState::Off:
        return ColourOff;
    case LedState::Error:
        instant = true;
        return ColourError;
    case LedState::Failsafe:
        return blinkOn(now, FailsafeBlinkMs) ? ColourFailsafe : ColourOff;
    case LedState::Held:
        return ColourHeld;
    case LedState::Bootloader:
        instant = true;
        return blinkOn(now, BootBlinkMs) ? ColourBoot : ColourOff;
    case LedState::Idle:
        if (!EnableRgbAnimations) {
            return ColourOff;
        }
        const uint8_t wave = breath(now, IdleBreathPeriodMs);
        const uint8_t level = static_cast<uint8_t>(
            IdleBreathMin + ((static_cast<uint16_t>(wave) * (IdleBreathMax - IdleBreathMin)) >> 8));
        return scaleRgb(ColourIdle, level);
    }

    return ColourOff;
}

uint8_t approach(uint8_t from, uint8_t to) {
    if (from == to) {
        return to;
    }
    if (from < to) {
        const uint8_t delta = static_cast<uint8_t>(to - from);
        return delta <= FadeStep ? to : static_cast<uint8_t>(from + FadeStep);
    }
    const uint8_t delta = static_cast<uint8_t>(from - to);
    return delta <= FadeStep ? to : static_cast<uint8_t>(from - FadeStep);
}

} // namespace
void StatusLedRenderer::render(uint32_t now, LedState state) {
    if (!EnableStatusLed) {
        return;
    }
    if (now - last_frame_at_ < FrameIntervalMs) {
        return;
    }
    last_frame_at_ = now;

    bool instant = false;
    const Rgb target = targetColour(state, now, instant);

    if (instant || !EnableRgbAnimations) {
        current_ = target;
    } else {
        current_.r = approach(current_.r, target.r);
        current_.g = approach(current_.g, target.g);
        current_.b = approach(current_.b, target.b);
    }

    const Rgb out = scaleRgb(current_, MaxBrightness);

    // Only bit-bang the strip when the pixel actually changes. show() runs with
    // interrupts disabled, so skipping no-op frames keeps core 1 responsive.
    if (out == shown_) {
        return;
    }
    shown_ = out;
    pixels_.setPixelColor(0, pixels_.Color(out.r, out.g, out.b));
    pixels_.show();
}

void StatusLedRenderer::begin(uint32_t now) {
    if (!EnableStatusLed) {
        return;
    }
    pinMode(NeoPixelPowerPin, OUTPUT);
    digitalWrite(NeoPixelPowerPin, HIGH);
    pixels_.begin();
    pixels_.setBrightness(255); // brightness handled in software, keep the lib linear
    pixels_.setPixelColor(0, pixels_.Color(0, 0, 0));
    pixels_.show();
    current_ = ColourOff;
    shown_ = ColourOff;
    last_frame_at_ = now;
}

#else
void StatusLedRenderer::begin(uint32_t) {}
void StatusLedRenderer::render(uint32_t, LedState) {}
#endif
} // namespace hidfw
