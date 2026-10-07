#include "BootloaderGate.h"
#include "InputConfig.h"
namespace hidfw {
bool BootloaderGate::advance(uint32_t now, bool confirmed, bool output_idle, bool vendor_ready,
                             bool tx_empty) {
    if (!confirmed) {
        pending_ = false;
        return false;
    }
    if (!pending_) {
        pending_ = true;
        at_ = now;
    }
    return uint32_t(now - at_) >= config::BootloaderDelayMs && output_idle && vendor_ready &&
           tx_empty;
}
} // namespace hidfw
