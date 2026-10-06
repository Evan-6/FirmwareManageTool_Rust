// RP2040 internal module; included once by Firmware.cpp.
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
enum class RxEventType : uint8_t { Byte, BadReportId };

struct RxEvent {
    RxEventType type;
    uint8_t value;
};

queue_t vendor_rx;
std::atomic<uint32_t> rx_overflow_count{0};
std::atomic<bool> rx_overflow_pending{false};

void rxPush(RxEventType type, uint8_t value = 0) {
    const RxEvent event{type, value};
    if (!queue_try_add(&vendor_rx, &event)) {
        rx_overflow_count.fetch_add(1, std::memory_order_relaxed);
        rx_overflow_pending.store(true, std::memory_order_release);
    }
}

bool rxPop(RxEvent &event) { return queue_try_remove(&vendor_rx, &event); }

uint8_t txFreeSpace(const TxQueue &queue) {
    return static_cast<uint8_t>((queue.tail - queue.head - 1) & TxMask);
}

uint8_t txQueued(const TxQueue &queue) {
    return static_cast<uint8_t>((queue.head - queue.tail) & TxMask);
}

void clearTxQueue(TxQueue &queue) {
    queue.head = 0;
    queue.tail = 0;
}

bool queueRam(const char *data, uint8_t length) {
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

bool queueText(const char *text) {
    const size_t length = strlen(text);
    if (length > 255) {
        ++vendor_tx.dropped_messages;
        return false;
    }

    return queueRam(text, static_cast<uint8_t>(length));
}

uint32_t txDroppedMessages() { return vendor_tx.dropped_messages; }

#define QUEUE_TEXT(text) queueText(text)

// Every err:* reply also arms the red flash, so failures are visible on the
// device without a host-side console.
#define QUEUE_ERROR(text)                                                                          \
    do {                                                                                           \
        led::signalError();                                                                        \
        queueText(text);                                                                           \
    } while (0)

void serviceVendorTx() {
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
