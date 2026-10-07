#include "StatusIndicator.h"
namespace hidfw {
constexpr uint16_t ErrorFlashMs = 140;
constexpr uint16_t FailsafeHoldMs = 3000;

void StatusIndicator::signalError(uint32_t now) {
    error_at_ = now;
    error_active_ = true;
}

void StatusIndicator::signalFailsafe(uint32_t now) {
    failsafe_at_ = now;
    failsafe_active_ = true;
}

void StatusIndicator::signalBootloader() { boot_signalled_ = true; }

void StatusIndicator::update(uint32_t now, bool mounted, bool has_pressed_keys) {
    LedState state = LedState::Off;

    if (boot_signalled_) {
        state = LedState::Bootloader;
    } else if (!mounted) {
        state = LedState::Off;
    } else if (error_active_ && now - error_at_ < ErrorFlashMs) {
        state = LedState::Error;
    } else if (failsafe_active_ && now - failsafe_at_ < FailsafeHoldMs) {
        state = LedState::Failsafe;
    } else if (has_pressed_keys) {
        state = LedState::Held;
    } else {
        state = LedState::Idle;
    }

    if (error_active_ && now - error_at_ >= ErrorFlashMs) {
        error_active_ = false;
    }
    if (failsafe_active_ && now - failsafe_at_ >= FailsafeHoldMs) {
        failsafe_active_ = false;
    }

    published_.store(state, std::memory_order_release);
}

} // namespace hidfw
