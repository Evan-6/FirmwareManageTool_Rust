// RP2040 internal module; included once by Firmware.cpp.
struct KeyboardReport {
    uint8_t modifiers;
    uint8_t reserved;
    uint8_t keys[MaxNonModifierKeys];
};

struct KeySpec {
    uint8_t usage;
    uint8_t modifier_mask;
};

enum class ApplyResult : uint8_t { Applied, AlreadyDesired, ReportFull, TransportFailed };

constexpr uint8_t ModLeftCtrl = 1u << 0;
constexpr uint8_t ModLeftShift = 1u << 1;
constexpr uint8_t ModLeftAlt = 1u << 2;
constexpr uint8_t ModLeftGui = 1u << 3;
constexpr uint8_t ModRightCtrl = 1u << 4;
constexpr uint8_t ModRightShift = 1u << 5;
constexpr uint8_t ModRightAlt = 1u << 6;
constexpr uint8_t ModRightGui = 1u << 7;

constexpr KeySpec usageKey(uint8_t usage) { return KeySpec{usage, 0}; }

constexpr KeySpec modifierKey(uint8_t mask) { return KeySpec{0, mask}; }
