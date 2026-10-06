// RP2040 internal module; included once by Firmware.cpp.
#if FIRMWARE_ENABLE_NEOPIXEL
struct Rgb {
    uint8_t r;
    uint8_t g;
    uint8_t b;
};

constexpr bool operator==(const Rgb &a, const Rgb &b) {
    return a.r == b.r && a.g == b.g && a.b == b.b;
}
#endif

namespace led {

enum class State : uint8_t { Off, Idle, Held, Error, Failsafe, Bootloader };

// This is the only state shared between the cores. Core 0 owns all priority and
// timeout decisions; core 1 loads this value once per render frame.
std::atomic<State> published_state{State::Off};
unsigned long error_signalled_at = 0;
unsigned long failsafe_signalled_at = 0;
bool error_active = false;
bool failsafe_active = false;
bool bootloader_signalled = false;

constexpr uint16_t ErrorFlashMs = 140;
constexpr uint16_t FailsafeHoldMs = 3000;

inline void signalError() {
    error_signalled_at = millis();
    error_active = true;
}

inline void signalFailsafe() {
    failsafe_signalled_at = millis();
    failsafe_active = true;
}

inline void signalBootloader() { bootloader_signalled = true; }

void update(unsigned long now, bool mounted, bool has_pressed_keys) {
    State state = State::Off;

    if (bootloader_signalled) {
        state = State::Bootloader;
    } else if (!mounted) {
        state = State::Off;
    } else if (error_active && now - error_signalled_at < ErrorFlashMs) {
        state = State::Error;
    } else if (failsafe_active && now - failsafe_signalled_at < FailsafeHoldMs) {
        state = State::Failsafe;
    } else if (has_pressed_keys) {
        state = State::Held;
    } else {
        state = State::Idle;
    }

    if (error_active && now - error_signalled_at >= ErrorFlashMs) {
        error_active = false;
    }
    if (failsafe_active && now - failsafe_signalled_at >= FailsafeHoldMs) {
        failsafe_active = false;
    }

    published_state.store(state, std::memory_order_release);
}

#if FIRMWARE_ENABLE_NEOPIXEL

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

unsigned long last_frame_at = 0;
Rgb current{0, 0, 0};
Rgb shown{255, 255, 255};

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

Rgb targetColour(State state, unsigned long now, bool &instant) {
    instant = false;

    switch (state) {
    case State::Off:
        return ColourOff;
    case State::Error:
        instant = true;
        return ColourError;
    case State::Failsafe:
        return blinkOn(now, FailsafeBlinkMs) ? ColourFailsafe : ColourOff;
    case State::Held:
        return ColourHeld;
    case State::Bootloader:
        instant = true;
        return blinkOn(now, BootBlinkMs) ? ColourBoot : ColourOff;
    case State::Idle:
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

void render(unsigned long now) {
    if (!EnableStatusLed) {
        return;
    }
    if (now - last_frame_at < FrameIntervalMs) {
        return;
    }
    last_frame_at = now;

    const State state = published_state.load(std::memory_order_acquire);
    bool instant = false;
    const Rgb target = targetColour(state, now, instant);

    if (instant || !EnableRgbAnimations) {
        current = target;
    } else {
        current.r = approach(current.r, target.r);
        current.g = approach(current.g, target.g);
        current.b = approach(current.b, target.b);
    }

    const Rgb out = scaleRgb(current, MaxBrightness);

    // Only bit-bang the strip when the pixel actually changes. show() runs with
    // interrupts disabled, so skipping no-op frames keeps core 1 responsive.
    if (out == shown) {
        return;
    }
    shown = out;
    pixels.setPixelColor(0, pixels.Color(out.r, out.g, out.b));
    pixels.show();
}

void begin() {
    if (!EnableStatusLed) {
        return;
    }
    pinMode(NeoPixelPowerPin, OUTPUT);
    digitalWrite(NeoPixelPowerPin, HIGH);
    pixels.begin();
    pixels.setBrightness(255); // brightness handled in software, keep the lib linear
    pixels.setPixelColor(0, pixels.Color(0, 0, 0));
    pixels.show();
    current = ColourOff;
    shown = ColourOff;
    last_frame_at = millis();
}

#else

inline void render(unsigned long) {}
inline void begin() {}

#endif

} // namespace led
