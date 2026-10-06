#pragma once
#include <stdint.h>
#include <string.h>
#include <vector>
#define USE_TINYUSB 1
#define HID_ITF_PROTOCOL_NONE 0
#define HID_REPORT_ID(x) x
#define TUD_HID_REPORT_DESC_MOUSE(x) 0x85, x
// Descriptor expansion is checked by the actual Arduino builds, not this I/O fake.
enum hid_report_type_t {
    HID_REPORT_TYPE_INVALID = 0,
    HID_REPORT_TYPE_INPUT = 1,
    HID_REPORT_TYPE_OUTPUT = 2,
    HID_REPORT_TYPE_FEATURE = 3
};
struct TinyStub {
    bool mounted_ = true;
    bool mounted() { return mounted_; }
    bool isInitialized() { return true; }
    void begin(int) {}
    void setID(int, int) {}
    void setDeviceVersion(int) {}
    void setSerialDescriptor(const char *) {}
    void setManufacturerDescriptor(const char *) {}
    void setProductDescriptor(const char *) {}
    void detach() { mounted_ = false; }
    void attach() { mounted_ = true; }
};
inline TinyStub TinyUSBDevice;
struct TestReport {
    uint8_t instance, id;
    std::vector<uint8_t> data;
    bool delivered = false;
};
inline std::vector<TestReport> test_reports;
extern "C" void tud_hid_report_complete_cb(uint8_t, const uint8_t *, uint16_t);
class Adafruit_USBD_HID {
  public:
    uint8_t instance;
    bool ready_ = true;
    static inline uint8_t next = 0;
    Adafruit_USBD_HID(const uint8_t *, uint16_t, int, int, bool) : instance(next++) {}
    bool ready() { return ready_; }
    bool begin() { return true; }
    template <class A, class B> void setReportCallback(A, B) {}
    bool sendReport(uint8_t id, const void *p, uint8_t n) {
        if (!ready_)
            return false;
        ready_ = false;
        const uint8_t *b = static_cast<const uint8_t *>(p);
        test_reports.push_back({instance, id, {b, b + n}, false});
        return true;
    }
};
