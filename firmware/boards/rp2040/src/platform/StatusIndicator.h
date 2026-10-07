#pragma once
#include <atomic>
#include <stdint.h>
namespace hidfw {
enum class LedState : uint8_t { Off, Idle, Held, Error, Failsafe, Bootloader };
class StatusIndicator {
  public:
    void signalError(uint32_t now);
    void signalFailsafe(uint32_t now);
    void signalBootloader();
    void update(uint32_t now, bool mounted, bool held);
    LedState state() const { return published_.load(std::memory_order_acquire); }

  private:
    std::atomic<LedState> published_{LedState::Off};
    uint32_t error_at_ = 0, failsafe_at_ = 0;
    bool error_active_ = false, failsafe_active_ = false, boot_signalled_ = false;
};
} // namespace hidfw
