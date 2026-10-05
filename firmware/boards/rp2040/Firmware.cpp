#include "Firmware.h"

#include <Arduino.h>
#include <Adafruit_TinyUSB.h>
#include <atomic>
#include <stdlib.h>
#include <string.h>
#include <pico/util/queue.h>

#if !defined(FIRMWARE_ENABLE_NEOPIXEL)
#if defined(ARDUINO_SEEED_XIAO_RP2040)
#define FIRMWARE_ENABLE_NEOPIXEL 1
#else
#define FIRMWARE_ENABLE_NEOPIXEL 0
#endif
#endif

#if FIRMWARE_ENABLE_NEOPIXEL
#include <Adafruit_NeoPixel.h>
#endif

#if !defined(USE_TINYUSB)
#error "RP2040 firmware requires the Adafruit TinyUSB stack. Set Tools > USB Stack to 'Adafruit TinyUSB' (arduino-cli: usbstack=tinyusb)."
#endif

// =============================================================================
// RP2040 port of the MSCV keyboard firmware.
//
// The command protocol and key-token parser remain compatible with
// boards/leonardo_avr/Firmware.cpp. The RP2040 keyboard engine is scan-paced and
// HID goes through Adafruit TinyUSB; there is no flash/RAM split, so
// PROGMEM/PSTR are dropped.
//
// Like the AVR build (which compiles with -DCDC_DISABLED), this firmware does
// NOT expose a USB CDC serial port: the only command channel is Vendor HID
// (usage page 0xFF60). Sunshine uses this same Vendor HID command channel.
//
// Threading model
// ---------------
//   core 0 (loop)  : TinyUSB, Vendor HID protocol, desired keyboard state,
//                    paced keyboard reports, failsafe and bootloader reset.
//   core 1 (loop1) : NeoPixel renderer only. It reads one atomic LED state and
//                    never touches TinyUSB, the parser or keyboard state.
//
// Keyboard output follows an 8 ms scan cadence and always sends a complete
// boot-keyboard snapshot. Intermediate command states that appear and disappear
// inside one scan period are coalesced, like a physical keyboard matrix scan;
// they are never queued for delayed macro-like playback.
//
// Wire protocol is the single source of truth in ../../PROTOCOL.md.
// =============================================================================

namespace {

// -----------------------------------------------------------------------------
// Configuration
// -----------------------------------------------------------------------------

constexpr unsigned long FailsafeReleaseMs = 30000UL;
constexpr unsigned long LineIdleTimeoutMs = 1000UL;
constexpr unsigned long KeyboardScanIntervalMs = 1UL;

constexpr uint8_t RxBudgetPerLoop = 64;
constexpr uint8_t LineBufferSize = 128;
constexpr uint8_t TxBufferSize = 128; // Must be a power of two and <= 256.
constexpr uint16_t RxQueueDepth = 128;

constexpr bool EnableKeyCommandAcks = true;
constexpr bool EnableStatusLed = FIRMWARE_ENABLE_NEOPIXEL != 0;
constexpr bool EnableRgbAnimations = true; // false => flat colours, no fades.
#
#if FIRMWARE_ENABLE_NEOPIXEL
#if !defined(FIRMWARE_NEOPIXEL_POWER_PIN)
#define FIRMWARE_NEOPIXEL_POWER_PIN 11
#endif
#if !defined(FIRMWARE_NEOPIXEL_DATA_PIN)
#define FIRMWARE_NEOPIXEL_DATA_PIN 12
#endif
constexpr uint8_t NeoPixelPowerPin = FIRMWARE_NEOPIXEL_POWER_PIN;
constexpr uint8_t NeoPixelDataPin = FIRMWARE_NEOPIXEL_DATA_PIN;

Adafruit_NeoPixel pixels(1, NeoPixelDataPin, NEO_GRB + NEO_KHZ800);
#endif

constexpr uint8_t KeyboardReportId = 2;
constexpr uint8_t MouseReportId = 3;
constexpr uint8_t AbsMouseReportId = 4;
constexpr uint8_t MaxNonModifierKeys = 6;
constexpr uint8_t VendorCommandReportId = 10;   // OUT: host -> device
constexpr uint8_t VendorResponseReportId = 11;  // IN:  device -> host
constexpr uint8_t VendorHidPayloadSize = 63;    // 64-byte endpoint minus report id.
constexpr unsigned long BootloaderResetDelayMs = 120UL;

// USB identity: kept in sync with boards/leonardo_avr so the host recognises
// either board interchangeably.
#if !defined(FIRMWARE_USB_VID)
#define FIRMWARE_USB_VID 0x03F0
#endif
#if !defined(FIRMWARE_USB_PID)
#define FIRMWARE_USB_PID 0x0024
#endif
#if !defined(FIRMWARE_USB_MANUFACTURER)
#define FIRMWARE_USB_MANUFACTURER "HP"
#endif
#if !defined(FIRMWARE_USB_PRODUCT)
#define FIRMWARE_USB_PRODUCT "HP Keyboard"
#endif

constexpr uint16_t UsbVid = FIRMWARE_USB_VID;
constexpr uint16_t UsbPid = FIRMWARE_USB_PID;
constexpr char UsbManufacturer[] = FIRMWARE_USB_MANUFACTURER;
constexpr char UsbProduct[] = FIRMWARE_USB_PRODUCT;

static_assert((TxBufferSize & (TxBufferSize - 1)) == 0,
              "TxBufferSize must be a power of two");

// -----------------------------------------------------------------------------
// USB HID keyboard implementation
// -----------------------------------------------------------------------------

struct KeyboardReport {
    uint8_t modifiers;
    uint8_t reserved;
    uint8_t keys[MaxNonModifierKeys];
};

struct KeySpec {
    uint8_t usage;
    uint8_t modifier_mask;
};

enum class ApplyResult : uint8_t {
    Applied,
    AlreadyDesired,
    ReportFull
};

constexpr uint8_t ModLeftCtrl   = 1u << 0;
constexpr uint8_t ModLeftShift  = 1u << 1;
constexpr uint8_t ModLeftAlt    = 1u << 2;
constexpr uint8_t ModLeftGui    = 1u << 3;
constexpr uint8_t ModRightCtrl  = 1u << 4;
constexpr uint8_t ModRightShift = 1u << 5;
constexpr uint8_t ModRightAlt   = 1u << 6;
constexpr uint8_t ModRightGui   = 1u << 7;

constexpr KeySpec usageKey(uint8_t usage)
{
    return KeySpec{usage, 0};
}

constexpr KeySpec modifierKey(uint8_t mask)
{
    return KeySpec{0, mask};
}

// Keyboard and mouse share ONE HID interface (report ids 2 and 3). This is not
// cosmetic: the RP2040 TinyUSB port hard-caps CFG_TUD_HID at 2 interfaces, and
// the second slot must stay free for the Vendor HID command channel. With three
// separate instances the last begin() (vendor) silently fails and the command
// channel never enumerates. A combined interface cannot claim a boot protocol,
// which only matters to BIOS/boot-protocol-only hosts.
const uint8_t HidReportDescriptor[] = {
    TUD_HID_REPORT_DESC_KEYBOARD(HID_REPORT_ID(KeyboardReportId)),
    TUD_HID_REPORT_DESC_MOUSE(HID_REPORT_ID(MouseReportId)),

    // Absolute pointer report (report id AbsMouseReportId): 5 buttons + 16-bit X/Y with
    // an Absolute (not Relative) Input flag. This is the standard "USB tablet" HID trick
    // (also used by VirtualBox/VMware absolute-mouse devices): Windows' HID mouse class
    // driver maps the 0-65535 logical range onto the primary display, so the report
    // itself carries the on-screen position instead of a delta.
    0x05, 0x01,       // Usage Page (Generic Desktop)
    0x09, 0x02,       // Usage (Mouse)
    0xA1, 0x01,       // Collection (Application)
    0x85, AbsMouseReportId,
    0x09, 0x01,       //   Usage (Pointer)
    0xA1, 0x00,       //   Collection (Physical)
    0x05, 0x09,       //     Usage Page (Button)
    0x19, 0x01,       //     Usage Minimum (Button 1)
    0x29, 0x05,       //     Usage Maximum (Button 5)
    0x15, 0x00,       //     Logical Minimum (0)
    0x25, 0x01,       //     Logical Maximum (1)
    0x95, 0x05,       //     Report Count (5)
    0x75, 0x01,       //     Report Size (1)
    0x81, 0x02,       //     Input (Data, Variable, Absolute)
    0x95, 0x01,       //     Report Count (1)
    0x75, 0x03,       //     Report Size (3)
    0x81, 0x01,       //     Input (Constant) - padding to a full byte
    0x05, 0x01,       //     Usage Page (Generic Desktop)
    0x09, 0x30,       //     Usage (X)
    0x09, 0x31,       //     Usage (Y)
    0x27, 0x00, 0x00, 0x00, 0x00,  // Logical Minimum (0)
    0x27, 0xFF, 0xFF, 0x00, 0x00,  // Logical Maximum (65535)
    0x75, 0x10,       //     Report Size (16)
    0x95, 0x02,       //     Report Count (2)
    0x81, 0x02,       //     Input (Data, Variable, Absolute)
    0xC0,             //   End Collection
    0xC0              // End Collection
};

// Vendor-defined command channel (usage page 0xFF60), mirrors the AVR build.
const uint8_t VendorHidReportDescriptor[] = {
    0x06, 0x60, 0xFF, // Usage Page (Vendor Defined 0xFF60)
    0x09, 0x61,       // Usage (Vendor 0x61)
    0xA1, 0x01,       // Collection (Application)

    0x85, VendorCommandReportId,
    0x09, 0x62,
    0x15, 0x00,
    0x26, 0xFF, 0x00,
    0x75, 0x08,
    0x95, VendorHidPayloadSize,
    0x91, 0x02,       // Output (Data, Variable, Absolute)

    0x85, VendorResponseReportId,
    0x09, 0x63,
    0x15, 0x00,
    0x26, 0xFF, 0x00,
    0x75, 0x08,
    0x95, VendorHidPayloadSize,
    0x81, 0x02,       // Input (Data, Variable, Absolute)

    0xC0
};

Adafruit_USBD_HID usb_hid(HidReportDescriptor,
                          sizeof(HidReportDescriptor),
                          HID_ITF_PROTOCOL_NONE,
                          1,
                          false);

Adafruit_USBD_HID usb_vendor(VendorHidReportDescriptor,
                             sizeof(VendorHidReportDescriptor),
                             HID_ITF_PROTOCOL_NONE,
                             1,
                             true); // has_out_endpoint

uint8_t popcount8(uint8_t value)
{
    value = static_cast<uint8_t>(value - ((value >> 1) & 0x55));
    value = static_cast<uint8_t>((value & 0x33) + ((value >> 2) & 0x33));
    return static_cast<uint8_t>((value + (value >> 4)) & 0x0F);
}

class KeyboardEngine {
public:
    void begin()
    {
        memset(&desired_report_, 0, sizeof(desired_report_));
        memset(&submitted_report_, 0, sizeof(submitted_report_));
        host_state_known_ = false;
        mounted_last_flush_ = false;
        send_failures_ = 0;
        failure_signalled_for_state_ = false;
        transport_error_pending_ = false;
        last_send_attempt_at_ = millis() - KeyboardScanIntervalMs;
    }

    KeyboardReport report() const
    {
        return desired_report_;
    }

    bool hasPressedKeys() const
    {
        return computeAnyPressed(desired_report_);
    }

    uint16_t sendFailures() const
    {
        return send_failures_;
    }

    ApplyResult press(const KeySpec& key)
    {
        KeyboardReport desired = desired_report_;

        if (key.modifier_mask != 0) {
            if ((desired.modifiers & key.modifier_mask) != 0) {
                return ApplyResult::AlreadyDesired;
            }

            desired.modifiers |= key.modifier_mask;
            return apply(desired);
        }

        for (uint8_t usage : desired.keys) {
            if (usage == key.usage) {
                return ApplyResult::AlreadyDesired;
            }
        }

        for (uint8_t i = 0; i < MaxNonModifierKeys; ++i) {
            if (desired.keys[i] == 0) {
                desired.keys[i] = key.usage;
                return apply(desired);
            }
        }

        return ApplyResult::ReportFull;
    }

    ApplyResult release(const KeySpec& key)
    {
        KeyboardReport desired = desired_report_;

        if (key.modifier_mask != 0) {
            if ((desired.modifiers & key.modifier_mask) == 0) {
                return ApplyResult::AlreadyDesired;
            }

            desired.modifiers &= static_cast<uint8_t>(~key.modifier_mask);
            return apply(desired);
        }

        uint8_t index = MaxNonModifierKeys;
        for (uint8_t i = 0; i < MaxNonModifierKeys; ++i) {
            if (desired.keys[i] == key.usage) {
                index = i;
                break;
            }
        }

        if (index == MaxNonModifierKeys) {
            return ApplyResult::AlreadyDesired;
        }

        // Compact the six-key array to keep status output deterministic.
        for (uint8_t i = index; i + 1 < MaxNonModifierKeys; ++i) {
            desired.keys[i] = desired.keys[i + 1];
        }
        desired.keys[MaxNonModifierKeys - 1] = 0;

        return apply(desired);
    }

    ApplyResult applySnapshot(const KeyboardReport& desired)
    {
        return apply(desired);
    }

    ApplyResult releaseAll()
    {
        const KeyboardReport empty_report{};
        return apply(empty_report);
    }

    // Model a physical keyboard scan: at most one complete snapshot is submitted
    // per scan period. Only the newest desired state is retained, so a command
    // flood cannot become a delayed stream of synthetic keystrokes.
    void flush(unsigned long now, bool mounted)
    {
        if (!mounted) {
            host_state_known_ = false;
            mounted_last_flush_ = false;
            return;
        }

        if (!mounted_last_flush_) {
            // A newly enumerated host has no knowledge of reports submitted on
            // the previous USB connection, even if desired_report_ is unchanged.
            host_state_known_ = false;
            failure_signalled_for_state_ = false;
            mounted_last_flush_ = true;
        }

        if (host_state_known_ &&
            memcmp(&desired_report_, &submitted_report_,
                   sizeof(desired_report_)) == 0) {
            return;
        }

        if (now - last_send_attempt_at_ < KeyboardScanIntervalMs ||
            !usb_hid.ready()) {
            return;
        }

        last_send_attempt_at_ = now;
        if (usb_hid.sendReport(
                KeyboardReportId, &desired_report_, sizeof(desired_report_))) {
            submitted_report_ = desired_report_;
            host_state_known_ = true;
            return;
        }

        ++send_failures_;
        if (!failure_signalled_for_state_) {
            failure_signalled_for_state_ = true;
            transport_error_pending_ = true;
        }
    }

    bool consumeTransportError()
    {
        const bool pending = transport_error_pending_;
        transport_error_pending_ = false;
        return pending;
    }

private:
    ApplyResult apply(const KeyboardReport& desired)
    {
        if (memcmp(&desired_report_, &desired, sizeof(desired_report_)) == 0) {
            return ApplyResult::AlreadyDesired;
        }

        desired_report_ = desired;
        failure_signalled_for_state_ = false;
        return ApplyResult::Applied;
    }

    static bool computeAnyPressed(const KeyboardReport& report)
    {
        if (report.modifiers != 0) {
            return true;
        }
        for (uint8_t usage : report.keys) {
            if (usage != 0) {
                return true;
            }
        }
        return false;
    }

    KeyboardReport desired_report_{};
    KeyboardReport submitted_report_{};
    unsigned long last_send_attempt_at_ = 0;
    uint16_t send_failures_ = 0;
    bool host_state_known_ = false;
    bool mounted_last_flush_ = false;
    bool failure_signalled_for_state_ = false;
    bool transport_error_pending_ = false;
};

KeyboardEngine keyboard;

// Standard relative mouse report (report id 3 on the shared HID interface).
struct MouseReport {
    uint8_t buttons;
    int8_t x;
    int8_t y;
    int8_t wheel;
    int8_t hwheel;
};

MouseReport mouse_report{};
uint16_t mouse_send_failures = 0;

// Absolute pointer report (report id AbsMouseReportId). Buttons mirror
// mouse_report.buttons so a button pressed/released via mouse_button stays
// consistent across both HID collections (see updateMouseButtons()).
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

// The keyboard and mouse share one IN endpoint; a report is "in flight" for up
// to one interval after sendReport(). Bounded wait so core 0 never wedges.
bool waitSharedHidReady()
{
    const unsigned long start = millis();
    while (!usb_hid.ready()) {
        if (millis() - start >= 5UL) {
            return false;
        }
        yield();
    }
    return true;
}

bool sendMouseDelta(int32_t x, int32_t y, int32_t wheel, int32_t hwheel)
{
    if (!TinyUSBDevice.mounted()) {
        ++mouse_send_failures;
        return false;
    }

    do {
        if (!waitSharedHidReady()) {
            ++mouse_send_failures;
            return false;
        }

        MouseReport report = mouse_report;
        report.x = static_cast<int8_t>(constrain(x, -127, 127));
        report.y = static_cast<int8_t>(constrain(y, -127, 127));
        report.wheel = static_cast<int8_t>(constrain(wheel, -127, 127));
        report.hwheel = static_cast<int8_t>(constrain(hwheel, -127, 127));
        if (!usb_hid.sendReport(MouseReportId, &report, sizeof(report))) {
            ++mouse_send_failures;
            return false;
        }
        x -= report.x;
        y -= report.y;
        wheel -= report.wheel;
        hwheel -= report.hwheel;
    } while (x != 0 || y != 0 || wheel != 0 || hwheel != 0);

    return true;
}

bool sendAbsMouse(uint16_t x, uint16_t y)
{
    if (!TinyUSBDevice.mounted()) {
        ++mouse_send_failures;
        return false;
    }

    if (!waitSharedHidReady()) {
        ++mouse_send_failures;
        return false;
    }

    AbsMouseReport report{};
    report.buttons = static_cast<uint8_t>(mouse_report.buttons & 0x1F);
    report.x = x;
    report.y = y;
    if (usb_hid.sendReport(AbsMouseReportId, &report, sizeof(report))) {
        abs_mouse_x = x;
        abs_mouse_y = y;
        abs_mouse_known = true;
        return true;
    }

    ++mouse_send_failures;
    return false;
}

// Re-emits the last absolute report (if any) so a button press/release made via
// mouse_button, release-all, or the failsafe stays visible to the absolute pointer
// HID collection too -- it is a separate top-level collection from the relative
// mouse report, so the OS tracks its button state independently.
bool resyncAbsMouseButtons()
{
    if (abs_mouse_known) {
        return sendAbsMouse(abs_mouse_x, abs_mouse_y);
    }
    return true;
}

bool updateMouseButtons(uint16_t flags, uint32_t data)
{
    constexpr uint16_t MouseLeftDown = 0x0002;
    constexpr uint16_t MouseLeftUp = 0x0004;
    constexpr uint16_t MouseRightDown = 0x0008;
    constexpr uint16_t MouseRightUp = 0x0010;
    constexpr uint16_t MouseMiddleDown = 0x0020;
    constexpr uint16_t MouseMiddleUp = 0x0040;
    constexpr uint16_t MouseXDown = 0x0080;
    constexpr uint16_t MouseXUp = 0x0100;

    if (flags & MouseLeftDown) mouse_report.buttons |= 0x01;
    if (flags & MouseLeftUp) mouse_report.buttons &= static_cast<uint8_t>(~0x01);
    if (flags & MouseRightDown) mouse_report.buttons |= 0x02;
    if (flags & MouseRightUp) mouse_report.buttons &= static_cast<uint8_t>(~0x02);
    if (flags & MouseMiddleDown) mouse_report.buttons |= 0x04;
    if (flags & MouseMiddleUp) mouse_report.buttons &= static_cast<uint8_t>(~0x04);
    if (flags & MouseXDown) mouse_report.buttons |= (data == 1 ? 0x08 : 0x10);
    if (flags & MouseXUp) mouse_report.buttons &= static_cast<uint8_t>(~(data == 1 ? 0x08 : 0x10));

    const bool relative_ok = sendMouseDelta(0, 0, 0, 0);
    const bool absolute_ok = resyncAbsMouseButtons();
    return relative_ok && absolute_ok;
}

bool releaseAllMouseButtons()
{
    mouse_report.buttons = 0;
    const bool relative_ok = sendMouseDelta(0, 0, 0, 0);
    const bool absolute_ok = resyncAbsMouseButtons();
    return relative_ok && absolute_ok;
}

// -----------------------------------------------------------------------------
// RGB status LED: intent (set by the command path) + renderer (core 1, fixed rate)
// -----------------------------------------------------------------------------

#if FIRMWARE_ENABLE_NEOPIXEL
struct Rgb {
    uint8_t r;
    uint8_t g;
    uint8_t b;
};

constexpr bool operator==(const Rgb& a, const Rgb& b)
{
    return a.r == b.r && a.g == b.g && a.b == b.b;
}
#endif

namespace led {

enum class State : uint8_t {
    Off,
    Idle,
    Held,
    Error,
    Failsafe,
    Bootloader
};

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

inline void signalError()
{
    error_signalled_at = millis();
    error_active = true;
}

inline void signalFailsafe()
{
    failsafe_signalled_at = millis();
    failsafe_active = true;
}

inline void signalBootloader()
{
    bootloader_signalled = true;
}

void update(unsigned long now, bool mounted, bool has_pressed_keys)
{
    State state = State::Off;

    if (bootloader_signalled) {
        state = State::Bootloader;
    }
    else if (!mounted) {
        state = State::Off;
    }
    else if (error_active && now - error_signalled_at < ErrorFlashMs) {
        state = State::Error;
    }
    else if (failsafe_active &&
             now - failsafe_signalled_at < FailsafeHoldMs) {
        state = State::Failsafe;
    }
    else if (has_pressed_keys) {
        state = State::Held;
    }
    else {
        state = State::Idle;
    }

    if (error_active && now - error_signalled_at >= ErrorFlashMs) {
        error_active = false;
    }
    if (failsafe_active &&
        now - failsafe_signalled_at >= FailsafeHoldMs) {
        failsafe_active = false;
    }

    published_state.store(state, std::memory_order_release);
}

#if FIRMWARE_ENABLE_NEOPIXEL

// --- renderer tunables --------------------------------------------------------
constexpr uint8_t  MaxBrightness      = 160;
constexpr uint16_t FrameIntervalMs    = 10;
constexpr uint8_t  FadeStep           = 18;
constexpr uint16_t IdleBreathPeriodMs = 4000;
constexpr uint8_t  IdleBreathMin      = 2;
constexpr uint8_t  IdleBreathMax      = 24;
constexpr uint16_t FailsafeBlinkMs    = 400;
constexpr uint16_t BootBlinkMs        = 60;

constexpr Rgb ColourOff      = {0, 0, 0};
constexpr Rgb ColourHeld     = {0, 160, 255};
constexpr Rgb ColourIdle     = {0, 40, 255};
constexpr Rgb ColourError    = {255, 20, 0};
constexpr Rgb ColourFailsafe = {255, 120, 0};
constexpr Rgb ColourBoot     = {200, 0, 255};

unsigned long last_frame_at = 0;
Rgb current{0, 0, 0};
Rgb shown{255, 255, 255};

uint8_t scale8(uint8_t value, uint8_t scale)
{
    return static_cast<uint8_t>((static_cast<uint16_t>(value) * scale) >> 8);
}

Rgb scaleRgb(const Rgb& colour, uint8_t scale)
{
    return Rgb{scale8(colour.r, scale), scale8(colour.g, scale), scale8(colour.b, scale)};
}

// Gamma-ish triangle breathing: 0 -> 255 -> 0 across `period`, squared so the
// low end lingers (the eye reads that as a smooth breath rather than a ramp).
uint8_t breath(unsigned long now, uint16_t period)
{
    const uint16_t pos = static_cast<uint16_t>(now % period);
    const uint16_t half = static_cast<uint16_t>(period / 2);
    const uint16_t tri = pos < half
        ? static_cast<uint16_t>((static_cast<uint32_t>(pos) * 255U) / half)
        : static_cast<uint16_t>((static_cast<uint32_t>(period - pos) * 255U) / half);
    return static_cast<uint8_t>((tri * tri) >> 8);
}

bool blinkOn(unsigned long now, uint16_t period)
{
    return (now % period) < (period / 2);
}

Rgb targetColour(State state, unsigned long now, bool& instant)
{
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
                IdleBreathMin +
                ((static_cast<uint16_t>(wave) *
                  (IdleBreathMax - IdleBreathMin)) >> 8));
            return scaleRgb(ColourIdle, level);
    }

    return ColourOff;
}

uint8_t approach(uint8_t from, uint8_t to)
{
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

void render(unsigned long now)
{
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
    }
    else {
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

void begin()
{
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

// -----------------------------------------------------------------------------
// Non-blocking response queue (Vendor HID is the only command channel)
// -----------------------------------------------------------------------------

struct TxQueue {
    uint8_t buffer[TxBufferSize];
    uint8_t head = 0;
    uint8_t tail = 0;
    uint16_t dropped_messages = 0;
};

constexpr uint8_t TxMask = TxBufferSize - 1;

TxQueue vendor_tx;

// TinyUSB's callback and the consumer both belong to core 0. The SDK queue keeps
// the callback short and preserves typed events without exposing parser work to
// the USB callback context.
enum class RxEventType : uint8_t {
    Byte,
    BadReportId
};

struct RxEvent {
    RxEventType type;
    uint8_t value;
};

queue_t vendor_rx;
std::atomic<uint32_t> rx_overflow_count{0};
std::atomic<bool> rx_overflow_pending{false};

void rxPush(RxEventType type, uint8_t value = 0)
{
    const RxEvent event{type, value};
    if (!queue_try_add(&vendor_rx, &event)) {
        rx_overflow_count.fetch_add(1, std::memory_order_relaxed);
        rx_overflow_pending.store(true, std::memory_order_release);
    }
}

bool rxPop(RxEvent& event)
{
    return queue_try_remove(&vendor_rx, &event);
}

uint8_t txFreeSpace(const TxQueue& queue)
{
    return static_cast<uint8_t>((queue.tail - queue.head - 1) & TxMask);
}

uint8_t txQueued(const TxQueue& queue)
{
    return static_cast<uint8_t>((queue.head - queue.tail) & TxMask);
}

void clearTxQueue(TxQueue& queue)
{
    queue.head = 0;
    queue.tail = 0;
}

bool queueRam(const char* data, uint8_t length)
{
    if (length > txFreeSpace(vendor_tx)) {
        ++vendor_tx.dropped_messages;
        return false;
    }

    for (uint8_t i = 0; i < length; ++i) {
        vendor_tx.buffer[vendor_tx.head] = static_cast<uint8_t>(data[i]);
        vendor_tx.head = static_cast<uint8_t>((vendor_tx.head + 1) & TxMask);
    }

    return true;
}

bool queueText(const char* text)
{
    const size_t length = strlen(text);
    if (length > 255) {
        ++vendor_tx.dropped_messages;
        return false;
    }

    return queueRam(text, static_cast<uint8_t>(length));
}

uint32_t txDroppedMessages()
{
    return vendor_tx.dropped_messages;
}

#define QUEUE_TEXT(text) queueText(text)

// Every err:* reply also arms the red flash, so failures are visible on the
// device without a host-side console.
#define QUEUE_ERROR(text)   \
    do {                    \
        led::signalError(); \
        queueText(text);    \
    } while (0)

void serviceVendorTx()
{
    if (!TinyUSBDevice.mounted() || !usb_vendor.ready()) {
        return;
    }

    if (vendor_tx.head == vendor_tx.tail) {
        return;
    }

    uint8_t payload[VendorHidPayloadSize] = {};
    uint8_t next_tail = vendor_tx.tail;
    uint8_t amount = txQueued(vendor_tx);
    if (amount > VendorHidPayloadSize) {
        amount = VendorHidPayloadSize;
    }

    for (uint8_t i = 0; i < amount; ++i) {
        payload[i] = vendor_tx.buffer[next_tail];
        next_tail = static_cast<uint8_t>((next_tail + 1) & TxMask);
    }

    if (usb_vendor.sendReport(VendorResponseReportId, payload, sizeof(payload))) {
        vendor_tx.tail = next_tail;
    }
}

// -----------------------------------------------------------------------------
// Fast key-token parser
// -----------------------------------------------------------------------------

constexpr uint32_t fnv1aConst(const char* text,
                              uint32_t hash = 2166136261UL)
{
    return *text == '\0'
        ? hash
        : fnv1aConst(
              text + 1,
              (hash ^ static_cast<uint8_t>(*text)) * 16777619UL);
}

uint32_t fnv1a(const char* text)
{
    uint32_t hash = 2166136261UL;

    while (*text != '\0') {
        hash ^= static_cast<uint8_t>(*text++);
        hash *= 16777619UL;
    }

    return hash;
}

bool parseFunctionKey(const char* token, KeySpec& result)
{
    if (token[0] != 'f' || token[1] < '1' || token[1] > '9') {
        return false;
    }

    uint16_t number = 0;
    const char* cursor = token + 1;

    while (*cursor >= '0' && *cursor <= '9') {
        number = static_cast<uint16_t>(number * 10 + (*cursor - '0'));
        if (number > 24) {
            return false;
        }
        ++cursor;
    }

    if (*cursor != '\0' || number < 1 || number > 24) {
        return false;
    }

    const uint8_t usage = number <= 12
        ? static_cast<uint8_t>(0x3A + number - 1)
        : static_cast<uint8_t>(0x68 + number - 13);

    result = usageKey(usage);
    return true;
}

bool parseKeyToken(const char* token, KeySpec& result)
{
    if (token[0] == '\0') {
        return false;
    }

    if (token[1] == '\0') {
        const char key = token[0];

        if (key >= 'a' && key <= 'z') {
            result = usageKey(static_cast<uint8_t>(0x04 + key - 'a'));
            return true;
        }

        if (key >= '1' && key <= '9') {
            result = usageKey(static_cast<uint8_t>(0x1E + key - '1'));
            return true;
        }

        if (key == '0') {
            result = usageKey(0x27);
            return true;
        }

        return false;
    }

    if (parseFunctionKey(token, result)) {
        return true;
    }

    const uint32_t hash = fnv1a(token);

#define KEY_CASE(name, value) \
    case fnv1aConst(name):     \
        if (strcmp(token, name) == 0) { \
            result = value;    \
            return true;       \
        }                      \
        break

    switch (hash) {
        KEY_CASE("left", usageKey(0x50));
        KEY_CASE("right", usageKey(0x4F));
        KEY_CASE("up", usageKey(0x52));
        KEY_CASE("down", usageKey(0x51));

        KEY_CASE("alt", modifierKey(ModLeftAlt));
        KEY_CASE("left_alt", modifierKey(ModLeftAlt));
        KEY_CASE("right_alt", modifierKey(ModRightAlt));

        KEY_CASE("ctrl", modifierKey(ModLeftCtrl));
        KEY_CASE("control", modifierKey(ModLeftCtrl));
        KEY_CASE("left_ctrl", modifierKey(ModLeftCtrl));
        KEY_CASE("right_ctrl", modifierKey(ModRightCtrl));

        KEY_CASE("shift", modifierKey(ModLeftShift));
        KEY_CASE("left_shift", modifierKey(ModLeftShift));
        KEY_CASE("right_shift", modifierKey(ModRightShift));

        KEY_CASE("win", modifierKey(ModLeftGui));
        KEY_CASE("gui", modifierKey(ModLeftGui));
        KEY_CASE("left_gui", modifierKey(ModLeftGui));
        KEY_CASE("right_gui", modifierKey(ModRightGui));

        KEY_CASE("enter", usageKey(0x28));
        KEY_CASE("return", usageKey(0x28));
        KEY_CASE("esc", usageKey(0x29));
        KEY_CASE("escape", usageKey(0x29));
        KEY_CASE("backspace", usageKey(0x2A));
        KEY_CASE("tab", usageKey(0x2B));
        KEY_CASE("space", usageKey(0x2C));
        KEY_CASE("caps_lock", usageKey(0x39));
        KEY_CASE("capslock", usageKey(0x39));

        KEY_CASE("print_screen", usageKey(0x46));
        KEY_CASE("printscreen", usageKey(0x46));
        KEY_CASE("scroll_lock", usageKey(0x47));
        KEY_CASE("scrolllock", usageKey(0x47));
        KEY_CASE("pause", usageKey(0x48));
        KEY_CASE("insert", usageKey(0x49));
        KEY_CASE("home", usageKey(0x4A));
        KEY_CASE("pageup", usageKey(0x4B));
        KEY_CASE("page_up", usageKey(0x4B));
        KEY_CASE("delete", usageKey(0x4C));
        KEY_CASE("end", usageKey(0x4D));
        KEY_CASE("pagedown", usageKey(0x4E));
        KEY_CASE("page_down", usageKey(0x4E));
        KEY_CASE("menu", usageKey(0x65));
        KEY_CASE("application", usageKey(0x65));

        KEY_CASE("minus", usageKey(0x2D));
        KEY_CASE("equal", usageKey(0x2E));
        KEY_CASE("left_bracket", usageKey(0x2F));
        KEY_CASE("right_bracket", usageKey(0x30));
        KEY_CASE("backslash", usageKey(0x31));
        KEY_CASE("semicolon", usageKey(0x33));
        KEY_CASE("quote", usageKey(0x34));
        KEY_CASE("apostrophe", usageKey(0x34));
        KEY_CASE("grave", usageKey(0x35));
        KEY_CASE("backtick", usageKey(0x35));
        KEY_CASE("comma", usageKey(0x36));
        KEY_CASE("period", usageKey(0x37));
        KEY_CASE("dot", usageKey(0x37));
        KEY_CASE("slash", usageKey(0x38));

        KEY_CASE("num_lock", usageKey(0x53));
        KEY_CASE("numlock", usageKey(0x53));
        KEY_CASE("kp_slash", usageKey(0x54));
        KEY_CASE("kp_asterisk", usageKey(0x55));
        KEY_CASE("kp_minus", usageKey(0x56));
        KEY_CASE("kp_plus", usageKey(0x57));
        KEY_CASE("kp_enter", usageKey(0x58));
        KEY_CASE("kp_1", usageKey(0x59));
        KEY_CASE("kp_2", usageKey(0x5A));
        KEY_CASE("kp_3", usageKey(0x5B));
        KEY_CASE("kp_4", usageKey(0x5C));
        KEY_CASE("kp_5", usageKey(0x5D));
        KEY_CASE("kp_6", usageKey(0x5E));
        KEY_CASE("kp_7", usageKey(0x5F));
        KEY_CASE("kp_8", usageKey(0x60));
        KEY_CASE("kp_9", usageKey(0x61));
        KEY_CASE("kp_0", usageKey(0x62));
        KEY_CASE("kp_dot", usageKey(0x63));
    }

#undef KEY_CASE

    return false;
}

// -----------------------------------------------------------------------------
// Command parser and status formatting
// -----------------------------------------------------------------------------

struct CommandParser {
    char line_buffer[LineBufferSize];
    uint8_t line_length = 0;
    bool discarding_line = false;
    bool swallow_lf = false;
    unsigned long last_rx_byte_at = 0;
};

CommandParser vendor_parser;
unsigned long last_lease_renewed_at = 0;

uint16_t line_too_long_count = 0;
uint16_t invalid_byte_count = 0;
uint16_t line_timeout_count = 0;
uint16_t bad_report_id_count = 0;
uint16_t failsafe_count = 0;

bool bootloader_reset_pending = false;
unsigned long bootloader_reset_requested_at = 0;

void resetParser(CommandParser& parser)
{
    parser.line_length = 0;
    parser.discarding_line = false;
    parser.swallow_lf = false;
}

char* trimAndLower(char* text)
{
    while (*text == ' ') {
        ++text;
    }

    char* cursor = text;
    char* trimmed_end = text;

    while (*cursor != '\0') {
        if (*cursor >= 'A' && *cursor <= 'Z') {
            *cursor = static_cast<char>(*cursor - 'A' + 'a');
        }

        if (*cursor != ' ') {
            trimmed_end = cursor + 1;
        }

        ++cursor;
    }

    *trimmed_end = '\0';
    return text;
}

void renewLease()
{
    last_lease_renewed_at = millis();
}

class TextBuilder {
public:
    TextBuilder()
        : length_(0)
    {
    }

    void append(char value)
    {
        if (length_ < sizeof(buffer_)) {
            buffer_[length_++] = value;
        }
    }

    void append(const char* text)
    {
        while (*text != '\0') {
            append(*text++);
        }
    }

    void appendUnsigned(uint32_t value)
    {
        char temporary[10];
        uint8_t count = 0;

        do {
            temporary[count++] = static_cast<char>('0' + value % 10);
            value /= 10;
        } while (value != 0 && count < sizeof(temporary));

        while (count > 0) {
            append(temporary[--count]);
        }
    }

    void appendHexByte(uint8_t value)
    {
        static const char Hex[] = "0123456789ABCDEF";
        append(Hex[value >> 4]);
        append(Hex[value & 0x0F]);
    }

    const char* data() const
    {
        return buffer_;
    }

    uint8_t length() const
    {
        return length_;
    }

private:
    char buffer_[112];
    uint8_t length_;
};

unsigned long leaseRemaining(unsigned long now)
{
    if (!keyboard.hasPressedKeys() && mouse_report.buttons == 0) {
        return 0;
    }

    const unsigned long elapsed = now - last_lease_renewed_at;
    return elapsed >= FailsafeReleaseMs
        ? 0
        : FailsafeReleaseMs - elapsed;
}

void queueHello()
{
    TextBuilder output;
    output.append("ok:hello,protocol=2,fw=2.0,lease_ms=");
    output.appendUnsigned(FailsafeReleaseMs);
    output.append(",max_nonmod=6,mouse=1\n");
    queueRam(output.data(), output.length());
}

void queueStatus()
{
    const KeyboardReport report = keyboard.report();
    uint8_t nonModifierCount = 0;
    for (uint8_t usage : report.keys) {
        if (usage != 0) {
            ++nonModifierCount;
        }
    }
    const uint8_t pressedCount = static_cast<uint8_t>(
        nonModifierCount + popcount8(report.modifiers));

    TextBuilder output;

    output.append("ok:status,p=");
    output.appendUnsigned(pressedCount);
    output.append(",n=");
    output.appendUnsigned(nonModifierCount);
    output.append(",m=");
    output.appendHexByte(report.modifiers);
    output.append(",k=");

    bool wrote_key = false;
    for (uint8_t usage : report.keys) {
        if (usage == 0) {
            continue;
        }

        if (wrote_key) {
            output.append('/');
        }

        output.appendHexByte(usage);
        wrote_key = true;
    }

    if (!wrote_key) {
        output.append('-');
    }

    output.append(",lease=");
    output.appendUnsigned(leaseRemaining(millis()));
    output.append(",fs=");
    output.appendUnsigned(failsafe_count);
    output.append(",rx=");
    output.appendUnsigned(
        static_cast<uint32_t>(line_too_long_count) +
        invalid_byte_count +
        line_timeout_count +
        bad_report_id_count +
        rx_overflow_count.load(std::memory_order_relaxed));
    output.append(",tx=");
    output.appendUnsigned(txDroppedMessages());
    output.append(",hid=");
    output.appendUnsigned(
        static_cast<uint32_t>(keyboard.sendFailures()) + mouse_send_failures);
    output.append(",mb=");
    output.appendHexByte(mouse_report.buttons);
    output.append('\n');

    queueRam(output.data(), output.length());
}

bool addKeyToSnapshot(KeyboardReport& report, const KeySpec& key)
{
    if (key.modifier_mask != 0) {
        report.modifiers |= key.modifier_mask;
        return true;
    }

    for (uint8_t usage : report.keys) {
        if (usage == key.usage) {
            return true;
        }
    }

    for (uint8_t i = 0; i < MaxNonModifierKeys; ++i) {
        if (report.keys[i] == 0) {
            report.keys[i] = key.usage;
            return true;
        }
    }

    return false;
}

void respondToKeyResult(ApplyResult result, bool is_down)
{
    switch (result) {
        case ApplyResult::Applied:
            renewLease();
            if (EnableKeyCommandAcks) {
                is_down
                    ? QUEUE_TEXT("ok:down\n")
                    : QUEUE_TEXT("ok:up\n");
            }
            return;

        case ApplyResult::AlreadyDesired:
            renewLease();
            if (EnableKeyCommandAcks) {
                is_down
                    ? QUEUE_TEXT("ok:already_down\n")
                    : QUEUE_TEXT("ok:already_up\n");
            }
            return;

        case ApplyResult::ReportFull:
            QUEUE_ERROR("err:hid_report_full\n");
            return;

    }
}

void handleSnapshotCommand(char* key_list)
{
    // Parse separately so unknown tokens and malformed lists get useful errors.
    KeyboardReport desired{};
    char* cursor = trimAndLower(key_list);

    if (*cursor != '\0') {
        while (true) {
            char* separator = strchr(cursor, ',');
            if (separator != nullptr) {
                *separator = '\0';
            }

            char* token = trimAndLower(cursor);
            if (*token == '\0') {
                QUEUE_ERROR("err:bad_sync\n");
                return;
            }

            KeySpec key{};
            if (!parseKeyToken(token, key)) {
                QUEUE_ERROR("err:unknown_key\n");
                return;
            }

            if (!addKeyToSnapshot(desired, key)) {
                QUEUE_ERROR("err:hid_report_full\n");
                return;
            }

            if (separator == nullptr) {
                break;
            }

            cursor = separator + 1;
        }
    }

    const ApplyResult result = keyboard.applySnapshot(desired);

    renewLease();

    if (EnableKeyCommandAcks) {
        result == ApplyResult::Applied
            ? QUEUE_TEXT("ok:sync\n")
            : QUEUE_TEXT("ok:sync_unchanged\n");
    }
}

void handleReleaseAll()
{
    const ApplyResult result = keyboard.releaseAll();
    const bool mouse_ok = releaseAllMouseButtons();

    (void)result;
    if (mouse_ok) {
        QUEUE_TEXT("ok:release_all\n");
    }
    else {
        QUEUE_ERROR("err:release_all_send_failed\n");
    }
}

bool parseSignedPair(const char* text, int32_t& first, int32_t& second)
{
    char* end = nullptr;
    const long parsed_first = strtol(text, &end, 10);
    if (end == text || *end != ',') {
        return false;
    }

    const char* second_text = end + 1;
    const long parsed_second = strtol(second_text, &end, 10);
    if (end == second_text || *end != '\0') {
        return false;
    }

    first = static_cast<int32_t>(parsed_first);
    second = static_cast<int32_t>(parsed_second);
    return true;
}

bool handleMouseCommand(char* line)
{
    int32_t first = 0;
    int32_t second = 0;

    if (strncmp(line, "mouse_move:", 11) == 0) {
        if (!parseSignedPair(line + 11, first, second) ||
            first < -MaxMouseDeltaPerCommand || first > MaxMouseDeltaPerCommand ||
            second < -MaxMouseDeltaPerCommand || second > MaxMouseDeltaPerCommand) {
            QUEUE_ERROR("err:bad_mouse\n");
            return true;
        }
        if (!sendMouseDelta(first, second, 0, 0)) {
            QUEUE_ERROR("err:hid_send_failed\n");
            return true;
        }
        // A relative move makes any previously known absolute position stale;
        // without this, the next mouse_button would resync the abs pointer
        // collection to that stale (x, y) and warp the cursor away from
        // wherever the relative move just placed it. See resyncAbsMouseButtons().
        abs_mouse_known = false;
        renewLease();
        if (EnableKeyCommandAcks) {
            QUEUE_TEXT("ok:mouse_move\n");
        }
        return true;
    }

    if (strncmp(line, "mouse_abs:", 10) == 0) {
        if (!parseSignedPair(line + 10, first, second) ||
            first < 0 || first > 65535 || second < 0 || second > 65535) {
            QUEUE_ERROR("err:bad_mouse\n");
            return true;
        }
        if (!sendAbsMouse(static_cast<uint16_t>(first),
                          static_cast<uint16_t>(second))) {
            QUEUE_ERROR("err:hid_send_failed\n");
            return true;
        }
        renewLease();
        if (EnableKeyCommandAcks) {
            QUEUE_TEXT("ok:mouse_abs\n");
        }
        return true;
    }

    if (strncmp(line, "mouse_button:", 13) == 0) {
        char* separator = strchr(line + 13, ',');
        if (separator == nullptr) {
            QUEUE_ERROR("err:bad_mouse\n");
            return true;
        }
        *separator = '\0';
        char* end = nullptr;
        const long button = strtol(line + 13, &end, 10);
        const bool down = strcmp(separator + 1, "down") == 0;
        const bool up = strcmp(separator + 1, "up") == 0;
        if (end == line + 13 || *end != '\0' ||
            button < 1 || button > 5 || (!down && !up)) {
            QUEUE_ERROR("err:bad_mouse\n");
            return true;
        }

        uint16_t flags = 0;
        uint32_t data = 0;
        switch (button) {
            case 1: flags = down ? 0x0002 : 0x0004; break;
            case 2: flags = down ? 0x0020 : 0x0040; break;
            case 3: flags = down ? 0x0008 : 0x0010; break;
            case 4: flags = down ? 0x0080 : 0x0100; data = 1; break;
            case 5: flags = down ? 0x0080 : 0x0100; data = 2; break;
        }
        if (!updateMouseButtons(flags, data)) {
            QUEUE_ERROR("err:hid_send_failed\n");
            return true;
        }
        renewLease();
        if (EnableKeyCommandAcks) {
            QUEUE_TEXT("ok:mouse_button\n");
        }
        return true;
    }

    if (strncmp(line, "mouse_wheel:", 12) == 0 ||
        strncmp(line, "mouse_hwheel:", 13) == 0) {
        const bool horizontal = line[6] == 'h';
        char* end = nullptr;
        const long amount = strtol(line + (horizontal ? 13 : 12), &end, 10);
        if (end == line + (horizontal ? 13 : 12) || *end != '\0' ||
            amount < -MaxMouseDeltaPerCommand || amount > MaxMouseDeltaPerCommand) {
            QUEUE_ERROR("err:bad_mouse\n");
            return true;
        }
        if (!sendMouseDelta(0, 0,
                            horizontal ? 0 : amount,
                            horizontal ? amount : 0)) {
            QUEUE_ERROR("err:hid_send_failed\n");
            return true;
        }
        renewLease();
        if (EnableKeyCommandAcks) {
            horizontal
                ? QUEUE_TEXT("ok:mouse_hwheel\n")
                : QUEUE_TEXT("ok:mouse_wheel\n");
        }
        return true;
    }

    return false;
}

void enterBootloaderNow()
{
    // Reboot into the RP2040 ROM bootloader (BOOTSEL / USB mass-storage).
    rp2040.rebootToBootloader();
    while (true) {
    }
}

void scheduleBootloaderReset()
{
    bootloader_reset_requested_at = millis();
    bootloader_reset_pending = true;
    led::signalBootloader();
}

void serviceBootloaderReset(unsigned long now)
{
    if (bootloader_reset_pending &&
        now - bootloader_reset_requested_at >= BootloaderResetDelayMs) {
        enterBootloaderNow();
    }
}

void handleCommand(char* raw_line)
{
    char* line = trimAndLower(raw_line);

    if (*line == '\0') {
        return;
    }

    if (strcmp(line, "ping") == 0) {
        renewLease();
        QUEUE_TEXT("pong\n");
        return;
    }

    if (strcmp(line, "hello") == 0) {
        queueHello();
        return;
    }

    if (strcmp(line, "status") == 0) {
        queueStatus();
        return;
    }

    if (strcmp(line, "enter_bootloader") == 0 ||
        strcmp(line, "bootloader") == 0) {
        keyboard.releaseAll();
        releaseAllMouseButtons();
        QUEUE_TEXT("ok:enter_bootloader\n");
        scheduleBootloaderReset();
        return;
    }

    if (strcmp(line, "r:all") == 0 ||
        strcmp(line, "release:all") == 0 ||
        strcmp(line, "allup") == 0 ||
        strcmp(line, "reset") == 0) {
        handleReleaseAll();
        return;
    }

    if (strncmp(line, "sync:", 5) == 0) {
        handleSnapshotCommand(line + 5);
        return;
    }

    // Short alias: =w,left_shift is the same as sync:w,left_shift.
    if (line[0] == '=') {
        handleSnapshotCommand(line + 1);
        return;
    }

    if (handleMouseCommand(line)) {
        return;
    }

    bool is_down = false;
    char* key_token = nullptr;

    if ((line[0] == 'd' || line[0] == 'u') && line[1] == ':') {
        is_down = line[0] == 'd';
        key_token = line + 2;
    }
    else if (line[0] == '+' || line[0] == '-') {
        is_down = line[0] == '+';
        key_token = line + 1;
    }
    else {
        QUEUE_ERROR("err:bad_command\n");
        return;
    }

    key_token = trimAndLower(key_token);

    KeySpec key{};
    if (!parseKeyToken(key_token, key)) {
        QUEUE_ERROR("err:unknown_key\n");
        return;
    }

    const ApplyResult result = is_down
        ? keyboard.press(key)
        : keyboard.release(key);

    respondToKeyResult(result, is_down);
}

void finishLine(CommandParser& parser)
{
    if (parser.discarding_line) {
        parser.line_length = 0;
        parser.discarding_line = false;
        return;
    }

    parser.line_buffer[parser.line_length] = '\0';
    handleCommand(parser.line_buffer);
    parser.line_length = 0;
}

void appendCommandByte(CommandParser& parser, uint8_t value)
{
    if (value == '\r') {
        finishLine(parser);
        parser.swallow_lf = true;
        return;
    }

    if (value == '\n') {
        if (parser.swallow_lf) {
            parser.swallow_lf = false;
            return;
        }

        finishLine(parser);
        return;
    }

    parser.swallow_lf = false;

    if (parser.discarding_line) {
        return;
    }

    if (value < 0x20 || value > 0x7E) {
        parser.line_length = 0;
        parser.discarding_line = true;
        ++invalid_byte_count;
        QUEUE_ERROR("err:invalid_byte\n");
        return;
    }

    if (parser.line_length >= LineBufferSize - 1) {
        parser.line_length = 0;
        parser.discarding_line = true;
        ++line_too_long_count;
        QUEUE_ERROR("err:line_too_long\n");
        return;
    }

    parser.line_buffer[parser.line_length++] = static_cast<char>(value);
}

// Vendor-HID OUT reports arrive asynchronously through this TinyUSB callback.
// Keep it minimal: validate the report id and hand the payload bytes to the RX
// queue. All parsing and keyboard output happen later in core 0's loop().
void vendorSetReport(uint8_t report_id,
                     hid_report_type_t report_type,
                     uint8_t const* buffer,
                     uint16_t bufsize)
{
    (void)report_type;

    const uint8_t* data = buffer;
    uint16_t length = bufsize;
    uint8_t id = report_id;

    // OUT-endpoint data arrives with report_id == 0 and the id as the first byte.
    if (id == 0 && length > 0) {
        id = data[0];
        ++data;
        --length;
    }

    if (id != VendorCommandReportId) {
        rxPush(RxEventType::BadReportId);
        return;
    }

    for (uint16_t i = 0; i < length; ++i) {
        const uint8_t value = data[i];
        if (value == 0) {
            break;
        }
        rxPush(RxEventType::Byte, value);
    }
}

// Drain the Vendor-HID RX queue and run commands on core 0.
// Returns true if any byte was consumed.
bool serviceVendorRx(unsigned long now)
{
    uint8_t processed = 0;
    RxEvent event{};
    bool did_work = rx_overflow_pending.exchange(
        false, std::memory_order_acq_rel);

    if (did_work) {
        // Once any byte is lost, no fragment from the affected line is safe to
        // execute. Bound cleanup to one queue depth; discarding_line preserves
        // the poisoned framing across later loops until a newline is observed.
        uint16_t discarded = 0;
        while (discarded < RxQueueDepth && rxPop(event)) {
            if (event.type == RxEventType::BadReportId) {
                ++bad_report_id_count;
            }
            ++discarded;
        }
        resetParser(vendor_parser);
        vendor_parser.discarding_line = true;
        vendor_parser.last_rx_byte_at = now;
        led::signalError();
        return true;
    }

    while (processed < RxBudgetPerLoop && rxPop(event)) {
        if (event.type == RxEventType::BadReportId) {
            ++bad_report_id_count;
            QUEUE_ERROR("err:bad_report_id\n");
        }
        else {
            appendCommandByte(vendor_parser, event.value);
        }
        ++processed;
    }

    if (processed != 0) {
        vendor_parser.last_rx_byte_at = now;
        did_work = true;
    }

    return did_work;
}

bool serviceKeyboardTransportError()
{
    if (!keyboard.consumeTransportError()) {
        return false;
    }
    QUEUE_ERROR("err:hid_send_failed\n");
    return true;
}

void serviceLineTimeout(CommandParser& parser, unsigned long now)
{
    if ((parser.line_length > 0 || parser.discarding_line) &&
        now - parser.last_rx_byte_at >= LineIdleTimeoutMs) {
        resetParser(parser);
        ++line_timeout_count;
        QUEUE_ERROR("err:line_timeout\n");
    }
}

void serviceFailsafe(unsigned long now)
{
    if ((keyboard.hasPressedKeys() || mouse_report.buttons != 0) &&
        now - last_lease_renewed_at >= FailsafeReleaseMs) {
        keyboard.releaseAll();
        releaseAllMouseButtons();
        ++failsafe_count;
        led::signalFailsafe();
        QUEUE_TEXT("warn:failsafe_release_all\n");
    }
}

} // namespace

namespace Firmware {

void setup()
{
    // Some arduino-pico configurations do not initialise TinyUSB before setup().
    if (!TinyUSBDevice.isInitialized()) {
        TinyUSBDevice.begin(0);
    }

    // Force disable CDC/COM port for Adafruit TinyUSB on RP2040.
    Serial.end();

    // Identify as the same device as the AVR build so the host treats them alike.
    TinyUSBDevice.setID(UsbVid, UsbPid);
    TinyUSBDevice.setManufacturerDescriptor(UsbManufacturer);
    TinyUSBDevice.setProductDescriptor(UsbProduct);

    queue_init(&vendor_rx, sizeof(RxEvent), RxQueueDepth);
    rx_overflow_count.store(0, std::memory_order_relaxed);
    rx_overflow_pending.store(false, std::memory_order_relaxed);

    usb_vendor.setReportCallback(nullptr, vendorSetReport);
    usb_hid.begin();
    usb_vendor.begin();

    keyboard.begin();
    clearTxQueue(vendor_tx);

    // If the core attached USB before setup() ran, the host has already
    // enumerated the default descriptor (with CDC and without the interfaces
    // added above). Re-attach so it re-enumerates the final layout.
    if (TinyUSBDevice.mounted()) {
        TinyUSBDevice.detach();
        delay(10);
        TinyUSBDevice.attach();
    }

    const unsigned long now = millis();
    vendor_parser.last_rx_byte_at = now;
    last_lease_renewed_at = now;
}

// ---- core 0: USB, protocol and keyboard state -------------------------------
void loop()
{
#ifdef TINYUSB_NEED_POLLING_TASK
    // Some platforms require the device task to be serviced manually.
    TinyUSBDevice.task();
#endif

    unsigned long now = millis();
    const bool mounted = TinyUSBDevice.mounted();

    // Parsing and every keyboard-state mutation happen on this core. Work is
    // bounded so USB service and the failsafe are revisited on every iteration.
    const bool did_work = serviceVendorRx(now);
    if (did_work) {
        now = millis();
    }
    serviceLineTimeout(vendor_parser, now);
    serviceFailsafe(now);

    // Keyboard reports have priority over protocol replies and are scan-paced.
    keyboard.flush(now, mounted);
    serviceKeyboardTransportError();
    serviceVendorTx();

    // LED signal helpers timestamp with millis(); refresh before comparing those
    // timestamps so an intervening millisecond tick cannot look like wraparound.
    now = millis();
    led::update(
        now,
        mounted,
        keyboard.hasPressedKeys() || mouse_report.buttons != 0);
    serviceBootloaderReset(now);
}

// ---- core 1: NeoPixel only --------------------------------------------------
void setup1()
{
    led::begin();
}

void loop1()
{
    led::render(millis());
    sleep_us(250);
}

} // namespace Firmware
