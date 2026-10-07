#pragma once
constexpr uint8_t KeyboardReportId = 2, ConsumerReportId = 5, MouseReportId = 3;
constexpr uint8_t V3OutputDepth = 2, V3BoardId = 3;
#define V3_PROGMEM PROGMEM
// Byte loads/stores are atomic on AVR; exchange also protects its read/write pair.
template <class T> class AvrAtomic {
    volatile T value_;

  public:
    constexpr AvrAtomic(T value) : value_(value) {}
    T load() const { return value_; }
    void store(T value) { value_ = value; }
    operator T() const { return load(); }
    AvrAtomic &operator=(T value) {
        store(value);
        return *this;
    }
    T exchange(T value) {
        T previous;
        ATOMIC_BLOCK(ATOMIC_RESTORESTATE) {
            previous = value_;
            value_ = value;
        }
        return previous;
    }
};
using AtomicByte = AvrAtomic<uint8_t>;
using AtomicBool = AvrAtomic<bool>;
template <uint8_t Depth> struct ReportQueue {
    uint8_t data[Depth][63]{};
    uint8_t head = 0, count = 0;
};
using V3RxQueue = ReportQueue<1>;
using V3TxQueue = ReportQueue<8>;
template <uint8_t D> bool queue_try_add(ReportQueue<D> *q, const void *report) {
    bool ok = false;
    ATOMIC_BLOCK(ATOMIC_RESTORESTATE) {
        if (q->count < D) {
            memcpy(q->data[(q->head + q->count) % D], report, 63);
            ++q->count;
            ok = true;
        }
    }
    return ok;
}
template <uint8_t D> bool queue_try_remove(ReportQueue<D> *q, void *report) {
    bool ok = false;
    ATOMIC_BLOCK(ATOMIC_RESTORESTATE) {
        if (q->count) {
            if (report)
                memcpy(report, q->data[q->head], 63);
            q->head = (q->head + 1) % D;
            --q->count;
            ok = true;
        }
    }
    return ok;
}
template <uint8_t D> bool queue_try_peek(ReportQueue<D> *q, void *report) {
    bool ok = false;
    ATOMIC_BLOCK(ATOMIC_RESTORESTATE) {
        if (q->count) {
            memcpy(report, q->data[q->head], 63);
            ok = true;
        }
    }
    return ok;
}
template <uint8_t D> uint8_t queue_get_level(ReportQueue<D> *q) { return q->count; }
void platformDeviceId(uint8_t *id) {
    // A stable model identity, not a unique per-board identifier.
    const uint8_t fixed[8] = {'H', 'P', 'K', 'B', 0, 0, 0x24, 3};
    memcpy(id, fixed, sizeof(fixed));
}
using hid_report_type_t = uint8_t;
constexpr uint8_t HID_REPORT_TYPE_INVALID = 0;
uint8_t usb_generation = 0;
bool bootloader_reset_pending = false;
unsigned long bootloader_reset_requested_at = 0;
void scheduleBootloaderReset();
namespace led {
void signalError() {}
void signalFailsafe() {}
} // namespace led
