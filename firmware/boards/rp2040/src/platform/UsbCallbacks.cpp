#include "UsbTransport.h"
#include <device/dcd.h>
namespace {
std::atomic<hidfw::UsbTransport *> active{nullptr};
}
namespace hidfw {
void bindTransport(UsbTransport *transport) { active.store(transport); }
void unbindTransport(UsbTransport *transport) {
    active.compare_exchange_strong(transport, nullptr);
}
uint16_t vendorFeature(uint8_t id, hid_report_type_t type, uint8_t *data, uint16_t length) {
    if (auto *transport = active.load())
        return transport->feature(id, type, data, length);
    return 0;
}
void vendorReceive(uint8_t id, hid_report_type_t type, const uint8_t *data, uint16_t length) {
    if (auto *transport = active.load())
        transport->receive(id, type, data, length);
}
} // namespace hidfw
extern "C" void tud_hid_report_complete_cb(uint8_t instance, const uint8_t *report,
                                           uint16_t length) {
    if (auto *transport = active.load())
        transport->completed(instance, report, length);
}
extern "C" void tud_hid_report_failed_cb(uint8_t instance, hid_report_type_t, const uint8_t *,
                                         uint16_t) {
    if (auto *transport = active.load())
        transport->failed(instance);
}
extern "C" void tud_event_hook_cb(uint8_t, uint32_t event, bool) {
    if (event == DCD_EVENT_BUS_RESET || event == DCD_EVENT_UNPLUGGED)
        if (auto *transport = active.load())
            transport->reset();
}
