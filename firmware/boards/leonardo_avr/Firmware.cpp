#include "Firmware.h"

#include <Arduino.h>
#include <HID.h>
#include <avr/pgmspace.h>
#include <avr/wdt.h>
#include <string.h>

#if !defined(USBCON)
#error "This firmware requires a native-USB AVR board such as Leonardo, Micro, or Pro Micro."
#endif

namespace {

// -----------------------------------------------------------------------------
// Configuration
// -----------------------------------------------------------------------------

constexpr unsigned long FailsafeReleaseMs = 30000UL;
constexpr unsigned long LineIdleTimeoutMs = 1000UL;

constexpr uint8_t RxBudgetPerLoop = 64;
constexpr uint8_t TxBudgetPerLoop = 64;
constexpr uint8_t LineBufferSize = 128;
constexpr uint8_t TxBufferSize = 128; // Must be a power of two and <= 256.

constexpr bool EnableKeyCommandAcks = true;
constexpr bool EnableStatusLed = false; // false gives the shortest command path.

#if defined(CDC_ENABLED)
constexpr unsigned long SerialBaudRate = 115200UL;
constexpr bool ReleaseOnSerialClose = true;
#endif

constexpr uint8_t KeyboardReportId = 2;
constexpr uint8_t MouseReportId = 3;
constexpr uint8_t AbsMouseReportId = 4;
constexpr uint8_t MaxNonModifierKeys = 6;
constexpr uint8_t VendorCommandReportId = 10;
constexpr uint8_t VendorResponseReportId = 11;
constexpr uint8_t VendorHidPayloadSize = USB_EP_SIZE - 1;
constexpr unsigned long BootloaderResetDelayMs = 120UL;
constexpr int32_t MaxMouseDeltaPerCommand = 1024;

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
    ReportFull,
    TransportError
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

const uint8_t HidReportDescriptor[] PROGMEM = {
    0x05, 0x01,       // Usage Page (Generic Desktop)
    0x09, 0x06,       // Usage (Keyboard)
    0xA1, 0x01,       // Collection (Application)
    0x85, KeyboardReportId,

    0x05, 0x07,       // Usage Page (Keyboard/Keypad)
    0x19, 0xE0,       // Usage Minimum (Left Control)
    0x29, 0xE7,       // Usage Maximum (Right GUI)
    0x15, 0x00,       // Logical Minimum (0)
    0x25, 0x01,       // Logical Maximum (1)
    0x75, 0x01,       // Report Size (1)
    0x95, 0x08,       // Report Count (8)
    0x81, 0x02,       // Input (Data, Variable, Absolute)

    0x95, 0x01,       // Report Count (1)
    0x75, 0x08,       // Report Size (8)
    0x81, 0x03,       // Input (Constant, Variable, Absolute)

    0x95, 0x06,       // Report Count (6)
    0x75, 0x08,       // Report Size (8)
    0x15, 0x00,       // Logical Minimum (0)
    0x25, 0x73,       // Logical Maximum (115)
    0x05, 0x07,       // Usage Page (Keyboard/Keypad)
    0x19, 0x00,       // Usage Minimum (Reserved)
    0x29, 0x73,       // Usage Maximum (Keyboard F24)
    0x81, 0x00,       // Input (Data, Array, Absolute)

    0xC0,             // End Collection

    // Relative mouse: five buttons, X/Y, vertical wheel and horizontal pan.
    0x05, 0x01,       // Usage Page (Generic Desktop)
    0x09, 0x02,       // Usage (Mouse)
    0xA1, 0x01,       // Collection (Application)
    0x85, MouseReportId,
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
    0x81, 0x01,       //     Input (Constant)
    0x05, 0x01,       //     Usage Page (Generic Desktop)
    0x09, 0x30,       //     Usage (X)
    0x09, 0x31,       //     Usage (Y)
    0x09, 0x38,       //     Usage (Wheel)
    0x15, 0x81,       //     Logical Minimum (-127)
    0x25, 0x7F,       //     Logical Maximum (127)
    0x75, 0x08,       //     Report Size (8)
    0x95, 0x03,       //     Report Count (3)
    0x81, 0x06,       //     Input (Data, Variable, Relative)
    0x05, 0x0C,       //     Usage Page (Consumer)
    0x0A, 0x38, 0x02, //     Usage (AC Pan)
    0x15, 0x81,       //     Logical Minimum (-127)
    0x25, 0x7F,       //     Logical Maximum (127)
    0x75, 0x08,       //     Report Size (8)
    0x95, 0x01,       //     Report Count (1)
    0x81, 0x06,       //     Input (Data, Variable, Relative)
    0xC0,             //   End Collection
    0xC0,             // End Collection

    // Absolute pointer: five buttons and 16-bit X/Y covering the primary display.
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
    0x81, 0x01,       //     Input (Constant)
    0x05, 0x01,       //     Usage Page (Generic Desktop)
    0x09, 0x30,       //     Usage (X)
    0x09, 0x31,       //     Usage (Y)
    0x27, 0x00, 0x00, 0x00, 0x00, // Logical Minimum (0)
    0x27, 0xFF, 0xFF, 0x00, 0x00, // Logical Maximum (65535)
    0x75, 0x10,       //     Report Size (16)
    0x95, 0x02,       //     Report Count (2)
    0x81, 0x02,       //     Input (Data, Variable, Absolute)
    0xC0,             //   End Collection
    0xC0              // End Collection
};

uint8_t popcount8(uint8_t value)
{
    value = static_cast<uint8_t>(value - ((value >> 1) & 0x55));
    value = static_cast<uint8_t>((value & 0x33) + ((value >> 2) & 0x33));
    return static_cast<uint8_t>((value + (value >> 4)) & 0x0F);
}

class FastKeyboard {
public:
    FastKeyboard()
        : send_failures_(0)
    {
        memset(&report_, 0, sizeof(report_));
        static HIDSubDescriptor descriptor_node(
            HidReportDescriptor,
            sizeof(HidReportDescriptor));
        HID().AppendDescriptor(&descriptor_node);
    }

    void begin()
    {
        HID().begin();
        memset(&report_, 0, sizeof(report_));
    }

    const KeyboardReport& report() const
    {
        return report_;
    }

    uint8_t nonModifierCount() const
    {
        uint8_t count = 0;
        for (uint8_t usage : report_.keys) {
            if (usage != 0) {
                ++count;
            }
        }
        return count;
    }

    uint8_t pressedCount() const
    {
        return static_cast<uint8_t>(
            nonModifierCount() + popcount8(report_.modifiers));
    }

    bool hasPressedKeys() const
    {
        if (report_.modifiers != 0) {
            return true;
        }

        for (uint8_t usage : report_.keys) {
            if (usage != 0) {
                return true;
            }
        }

        return false;
    }

    uint16_t sendFailures() const
    {
        return send_failures_;
    }

    ApplyResult press(const KeySpec& key)
    {
        KeyboardReport desired = report_;

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
        KeyboardReport desired = report_;

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
        KeyboardReport empty_report{};

        if (memcmp(&report_, &empty_report, sizeof(report_)) == 0) {
            return ApplyResult::AlreadyDesired;
        }

        // Safety first: local state becomes empty even if USB transmission fails.
        report_ = empty_report;

        if (sendCurrentReport() < 0) {
            return ApplyResult::TransportError;
        }

        return ApplyResult::Applied;
    }

private:
    ApplyResult apply(const KeyboardReport& desired)
    {
        if (memcmp(&report_, &desired, sizeof(report_)) == 0) {
            return ApplyResult::AlreadyDesired;
        }

        const KeyboardReport previous = report_;
        report_ = desired;

        if (sendCurrentReport() < 0) {
            report_ = previous;
            return ApplyResult::TransportError;
        }

        return ApplyResult::Applied;
    }

    int sendCurrentReport()
    {
        const int result = HID().SendReport(
            KeyboardReportId,
            &report_,
            sizeof(report_));

        if (result < 0) {
            ++send_failures_;
        }

        return result;
    }

    KeyboardReport report_;
    uint16_t send_failures_;
};

FastKeyboard keyboard;

struct MouseReport {
    uint8_t buttons;
    int8_t x;
    int8_t y;
    int8_t wheel;
    int8_t hwheel;
};

struct __attribute__((packed)) AbsMouseReport {
    uint8_t buttons;
    uint16_t x;
    uint16_t y;
};

MouseReport mouse_report{};
uint16_t abs_mouse_x = 0;
uint16_t abs_mouse_y = 0;
uint16_t mouse_send_failures = 0;
bool abs_mouse_known = false;

bool sendMouseDelta(int32_t x, int32_t y, int32_t wheel, int32_t hwheel)
{
    do {
        MouseReport report = mouse_report;
        report.x = static_cast<int8_t>(constrain(x, -127, 127));
        report.y = static_cast<int8_t>(constrain(y, -127, 127));
        report.wheel = static_cast<int8_t>(constrain(wheel, -127, 127));
        report.hwheel = static_cast<int8_t>(constrain(hwheel, -127, 127));

        if (HID().SendReport(MouseReportId, &report, sizeof(report)) < 0) {
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
    AbsMouseReport report{};
    report.buttons = static_cast<uint8_t>(mouse_report.buttons & 0x1F);
    report.x = x;
    report.y = y;

    if (HID().SendReport(AbsMouseReportId, &report, sizeof(report)) < 0) {
        ++mouse_send_failures;
        return false;
    }

    abs_mouse_x = x;
    abs_mouse_y = y;
    abs_mouse_known = true;
    return true;
}

bool resyncAbsMouseButtons()
{
    return !abs_mouse_known || sendAbsMouse(abs_mouse_x, abs_mouse_y);
}

bool updateMouseButton(uint8_t button, bool down)
{
    const uint8_t mask = static_cast<uint8_t>(1u << (button - 1));
    if (down) {
        mouse_report.buttons |= mask;
    }
    else {
        mouse_report.buttons &= static_cast<uint8_t>(~mask);
    }

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
// Non-blocking command response queues
// -----------------------------------------------------------------------------

enum class ResponseTarget : uint8_t {
    Serial,
    VendorHid,
    Broadcast
};

struct TxQueue {
    uint8_t buffer[TxBufferSize];
    uint8_t head = 0;
    uint8_t tail = 0;
    uint16_t dropped_messages = 0;
};

struct CommandParser;

constexpr uint8_t TxMask = TxBufferSize - 1;

TxQueue serial_tx;
TxQueue vendor_tx;
ResponseTarget active_response_target = ResponseTarget::Serial;

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

bool queueRamTo(TxQueue& queue, const char* data, uint8_t length)
{
    if (length > txFreeSpace(queue)) {
        ++queue.dropped_messages;
        return false;
    }

    for (uint8_t i = 0; i < length; ++i) {
        queue.buffer[queue.head] = static_cast<uint8_t>(data[i]);
        queue.head = static_cast<uint8_t>((queue.head + 1) & TxMask);
    }

    return true;
}

bool queueFlashTo(TxQueue& queue, PGM_P text, uint8_t length)
{
    if (length > txFreeSpace(queue)) {
        ++queue.dropped_messages;
        return false;
    }

    for (uint8_t i = 0; i < length; ++i) {
        queue.buffer[queue.head] = pgm_read_byte(text + i);
        queue.head = static_cast<uint8_t>((queue.head + 1) & TxMask);
    }

    return true;
}

bool queueRam(const char* data, uint8_t length)
{
    if (active_response_target == ResponseTarget::Serial) {
        return queueRamTo(serial_tx, data, length);
    }
    if (active_response_target == ResponseTarget::VendorHid) {
        return queueRamTo(vendor_tx, data, length);
    }

    const bool serial_ok = queueRamTo(serial_tx, data, length);
    const bool vendor_ok = queueRamTo(vendor_tx, data, length);
    return serial_ok && vendor_ok;
}

bool queueFlash(PGM_P text)
{
    const size_t length = strlen_P(text);
    if (length > 255) {
        if (active_response_target == ResponseTarget::Serial ||
            active_response_target == ResponseTarget::Broadcast) {
            ++serial_tx.dropped_messages;
        }
        if (active_response_target == ResponseTarget::VendorHid ||
            active_response_target == ResponseTarget::Broadcast) {
            ++vendor_tx.dropped_messages;
        }
        return false;
    }

    const uint8_t byte_length = static_cast<uint8_t>(length);
    if (active_response_target == ResponseTarget::Serial) {
        return queueFlashTo(serial_tx, text, byte_length);
    }
    if (active_response_target == ResponseTarget::VendorHid) {
        return queueFlashTo(vendor_tx, text, byte_length);
    }

    const bool serial_ok = queueFlashTo(serial_tx, text, byte_length);
    const bool vendor_ok = queueFlashTo(vendor_tx, text, byte_length);
    return serial_ok && vendor_ok;
}

uint32_t txDroppedMessages()
{
    return static_cast<uint32_t>(serial_tx.dropped_messages) +
           vendor_tx.dropped_messages;
}

#define QUEUE_FLASH(text) queueFlash(PSTR(text))

// -----------------------------------------------------------------------------
// Vendor-defined HID command interface
// -----------------------------------------------------------------------------

const uint8_t VendorHidReportDescriptor[] PROGMEM = {
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

typedef struct {
    InterfaceDescriptor hid;
    HIDDescDescriptor desc;
    EndpointDescriptor in;
    EndpointDescriptor out;
} VendorHidDescriptor;

class VendorHidTransport : public PluggableUSBModule {
public:
    VendorHidTransport()
        : PluggableUSBModule(2, 1, ep_type_)
    {
        ep_type_[0] = EP_TYPE_INTERRUPT_IN;
        ep_type_[1] = EP_TYPE_INTERRUPT_OUT;
        PluggableUSB().plug(this);
    }

    void begin()
    {
        clearTxQueue(vendor_tx);
    }

    void serviceRx(CommandParser& parser, unsigned long now);

    void serviceTx()
    {
        if (vendor_tx.head == vendor_tx.tail ||
            USB_SendSpace(inEndpoint()) < USB_EP_SIZE) {
            return;
        }

        uint8_t report[USB_EP_SIZE] = {};
        uint8_t next_tail = vendor_tx.tail;
        uint8_t amount = txQueued(vendor_tx);
        if (amount > VendorHidPayloadSize) {
            amount = VendorHidPayloadSize;
        }

        report[0] = VendorResponseReportId;
        for (uint8_t i = 0; i < amount; ++i) {
            report[i + 1] = vendor_tx.buffer[next_tail];
            next_tail = static_cast<uint8_t>((next_tail + 1) & TxMask);
        }

        if (USB_Send(inEndpoint() | TRANSFER_RELEASE, report, sizeof(report)) >= 0) {
            vendor_tx.tail = next_tail;
        }
    }

protected:
    int getInterface(uint8_t* interfaceCount) override
    {
        *interfaceCount += 1;
        VendorHidDescriptor descriptor = {
            D_INTERFACE(pluggedInterface, 2, USB_DEVICE_CLASS_HUMAN_INTERFACE, HID_SUBCLASS_NONE, HID_PROTOCOL_NONE),
            D_HIDREPORT(sizeof(VendorHidReportDescriptor)),
            D_ENDPOINT(USB_ENDPOINT_IN(inEndpoint()), USB_ENDPOINT_TYPE_INTERRUPT, USB_EP_SIZE, 0x01),
            D_ENDPOINT(USB_ENDPOINT_OUT(outEndpoint()), USB_ENDPOINT_TYPE_INTERRUPT, USB_EP_SIZE, 0x01)
        };
        return USB_SendControl(0, &descriptor, sizeof(descriptor));
    }

    int getDescriptor(USBSetup& setup) override
    {
        if (setup.bmRequestType != REQUEST_DEVICETOHOST_STANDARD_INTERFACE) {
            return 0;
        }
        if (setup.wValueH != HID_REPORT_DESCRIPTOR_TYPE) {
            return 0;
        }
        if (setup.wIndex != pluggedInterface) {
            return 0;
        }

        return USB_SendControl(
            TRANSFER_PGM,
            VendorHidReportDescriptor,
            sizeof(VendorHidReportDescriptor));
    }

    bool setup(USBSetup& setup) override
    {
        if (setup.wIndex != pluggedInterface) {
            return false;
        }

        const uint8_t request = setup.bRequest;
        const uint8_t requestType = setup.bmRequestType;

        if (requestType == REQUEST_HOSTTODEVICE_CLASS_INTERFACE &&
            (request == HID_SET_IDLE || request == HID_SET_PROTOCOL)) {
            return true;
        }

        if (requestType == REQUEST_DEVICETOHOST_CLASS_INTERFACE &&
            (request == HID_GET_IDLE || request == HID_GET_PROTOCOL)) {
            uint8_t value = 0;
            USB_SendControl(0, &value, sizeof(value));
            return true;
        }

        return false;
    }

    uint8_t getShortName(char* name) override
    {
        name[0] = 'G';
        name[1] = 'D';
        name[2] = 'K';
        return 3;
    }

private:
    uint8_t inEndpoint() const
    {
        return pluggedEndpoint;
    }

    uint8_t outEndpoint() const
    {
        return static_cast<uint8_t>(pluggedEndpoint + 1);
    }

    uint8_t ep_type_[2];
};

VendorHidTransport vendor_hid;

#if defined(CDC_ENABLED)
bool serialPortOpen()
{
    // Unlike "if (Serial)", dtr()/rts() do not add a 10 ms delay on AVR.
    return Serial.dtr() || Serial.rts();
}

void serviceTx(bool port_open)
{
    if (!port_open || serial_tx.head == serial_tx.tail) {
        return;
    }

    int writable = Serial.availableForWrite();
    if (writable <= 0) {
        return;
    }

    uint8_t queued = txQueued(serial_tx);
    uint8_t contiguous = serial_tx.head > serial_tx.tail
        ? static_cast<uint8_t>(serial_tx.head - serial_tx.tail)
        : static_cast<uint8_t>(TxBufferSize - serial_tx.tail);

    uint8_t amount = queued;
    if (amount > contiguous) {
        amount = contiguous;
    }
    if (amount > TxBudgetPerLoop) {
        amount = TxBudgetPerLoop;
    }
    if (amount > static_cast<uint8_t>(writable)) {
        amount = static_cast<uint8_t>(writable);
    }

    if (amount == 0) {
        return;
    }

    const size_t written = Serial.write(&serial_tx.buffer[serial_tx.tail], amount);
    serial_tx.tail = static_cast<uint8_t>((serial_tx.tail + written) & TxMask);
}
#else
bool serialPortOpen()
{
    return false;
}

void serviceTx(bool)
{
}
#endif

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

CommandParser serial_parser;
CommandParser vendor_parser;
unsigned long last_lease_renewed_at = 0;

uint16_t line_too_long_count = 0;
uint16_t invalid_byte_count = 0;
uint16_t line_timeout_count = 0;
uint16_t failsafe_count = 0;

bool previous_port_open = false;
bool bootloader_reset_pending = false;
unsigned long bootloader_reset_requested_at = 0;

void updateStatusLed()
{
    if (EnableStatusLed) {
        digitalWrite(
            LED_BUILTIN,
            (keyboard.hasPressedKeys() || mouse_report.buttons != 0) ? HIGH : LOW);
    }
}

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
    const KeyboardReport& report = keyboard.report();
    TextBuilder output;

    output.append("ok:status,p=");
    output.appendUnsigned(keyboard.pressedCount());
    output.append(",n=");
    output.appendUnsigned(keyboard.nonModifierCount());
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
        line_timeout_count);
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
            updateStatusLed();
            if (EnableKeyCommandAcks) {
                is_down
                    ? QUEUE_FLASH("ok:down\n")
                    : QUEUE_FLASH("ok:up\n");
            }
            return;

        case ApplyResult::AlreadyDesired:
            renewLease();
            if (EnableKeyCommandAcks) {
                is_down
                    ? QUEUE_FLASH("ok:already_down\n")
                    : QUEUE_FLASH("ok:already_up\n");
            }
            return;

        case ApplyResult::ReportFull:
            QUEUE_FLASH("err:hid_report_full\n");
            return;

        case ApplyResult::TransportError:
            QUEUE_FLASH("err:hid_send_failed\n");
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
                QUEUE_FLASH("err:bad_sync\n");
                return;
            }

            KeySpec key{};
            if (!parseKeyToken(token, key)) {
                QUEUE_FLASH("err:unknown_key\n");
                return;
            }

            if (!addKeyToSnapshot(desired, key)) {
                QUEUE_FLASH("err:hid_report_full\n");
                return;
            }

            if (separator == nullptr) {
                break;
            }

            cursor = separator + 1;
        }
    }

    const ApplyResult result = keyboard.applySnapshot(desired);

    if (result == ApplyResult::TransportError) {
        QUEUE_FLASH("err:hid_send_failed\n");
        return;
    }

    renewLease();
    updateStatusLed();

    if (EnableKeyCommandAcks) {
        result == ApplyResult::Applied
            ? QUEUE_FLASH("ok:sync\n")
            : QUEUE_FLASH("ok:sync_unchanged\n");
    }
}

void handleReleaseAll()
{
    const ApplyResult result = keyboard.releaseAll();
    const bool mouse_ok = releaseAllMouseButtons();
    updateStatusLed();

    if (result == ApplyResult::TransportError || !mouse_ok) {
        QUEUE_FLASH("err:release_all_send_failed\n");
    }
    else {
        QUEUE_FLASH("ok:release_all\n");
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
            QUEUE_FLASH("err:bad_mouse\n");
            return true;
        }

        if (!sendMouseDelta(first, second, 0, 0)) {
            QUEUE_FLASH("err:hid_send_failed\n");
            return true;
        }

        renewLease();
        if (EnableKeyCommandAcks) {
            QUEUE_FLASH("ok:mouse_move\n");
        }
        return true;
    }

    if (strncmp(line, "mouse_abs:", 10) == 0) {
        if (!parseSignedPair(line + 10, first, second) ||
            first < 0 || first > 65535 || second < 0 || second > 65535) {
            QUEUE_FLASH("err:bad_mouse\n");
            return true;
        }

        if (!sendAbsMouse(static_cast<uint16_t>(first),
                          static_cast<uint16_t>(second))) {
            QUEUE_FLASH("err:hid_send_failed\n");
            return true;
        }

        renewLease();
        if (EnableKeyCommandAcks) {
            QUEUE_FLASH("ok:mouse_abs\n");
        }
        return true;
    }

    if (strncmp(line, "mouse_button:", 13) == 0) {
        char* separator = strchr(line + 13, ',');
        if (separator == nullptr) {
            QUEUE_FLASH("err:bad_mouse\n");
            return true;
        }

        *separator = '\0';
        char* end = nullptr;
        const long button = strtol(line + 13, &end, 10);
        const bool down = strcmp(separator + 1, "down") == 0;
        const bool up = strcmp(separator + 1, "up") == 0;
        if (end == line + 13 || *end != '\0' ||
            button < 1 || button > 5 || (!down && !up)) {
            QUEUE_FLASH("err:bad_mouse\n");
            return true;
        }

        if (!updateMouseButton(static_cast<uint8_t>(button), down)) {
            QUEUE_FLASH("err:hid_send_failed\n");
            return true;
        }

        renewLease();
        updateStatusLed();
        if (EnableKeyCommandAcks) {
            QUEUE_FLASH("ok:mouse_button\n");
        }
        return true;
    }

    if (strncmp(line, "mouse_wheel:", 12) == 0 ||
        strncmp(line, "mouse_hwheel:", 13) == 0) {
        const bool horizontal = line[6] == 'h';
        const char* amount_text = line + (horizontal ? 13 : 12);
        char* end = nullptr;
        const long amount = strtol(amount_text, &end, 10);
        if (end == amount_text || *end != '\0' ||
            amount < -MaxMouseDeltaPerCommand ||
            amount > MaxMouseDeltaPerCommand) {
            QUEUE_FLASH("err:bad_mouse\n");
            return true;
        }

        if (!sendMouseDelta(0, 0,
                            horizontal ? 0 : amount,
                            horizontal ? amount : 0)) {
            QUEUE_FLASH("err:hid_send_failed\n");
            return true;
        }

        renewLease();
        if (EnableKeyCommandAcks) {
            horizontal
                ? QUEUE_FLASH("ok:mouse_hwheel\n")
                : QUEUE_FLASH("ok:mouse_wheel\n");
        }
        return true;
    }

    return false;
}

bool hasNewLufaBootloader()
{
    return pgm_read_word(FLASHEND - 1) == NEW_LUFA_SIGNATURE;
}

void enterBootloaderNow()
{
    uint16_t magic_key_pos = MAGIC_KEY_POS;

#if MAGIC_KEY_POS != (RAMEND - 1)
    if (hasNewLufaBootloader()) {
        magic_key_pos = (RAMEND - 1);
    }

    if (magic_key_pos != (RAMEND - 1) &&
        *(uint16_t*)magic_key_pos != MAGIC_KEY) {
        *(uint16_t*)(RAMEND - 1) = *(uint16_t*)magic_key_pos;
    }
#endif

    *(uint16_t*)magic_key_pos = MAGIC_KEY;
    wdt_enable(WDTO_120MS);
    while (true) {
    }
}

void scheduleBootloaderReset()
{
    bootloader_reset_pending = true;
    bootloader_reset_requested_at = millis();
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
        QUEUE_FLASH("pong\n");
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
        updateStatusLed();
        QUEUE_FLASH("ok:enter_bootloader\n");
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
        QUEUE_FLASH("err:bad_command\n");
        return;
    }

    key_token = trimAndLower(key_token);

    KeySpec key{};
    if (!parseKeyToken(key_token, key)) {
        QUEUE_FLASH("err:unknown_key\n");
        return;
    }

    const ApplyResult result = is_down
        ? keyboard.press(key)
        : keyboard.release(key);

    respondToKeyResult(result, is_down);
}

void finishLine(CommandParser& parser, ResponseTarget target)
{
    if (parser.discarding_line) {
        parser.line_length = 0;
        parser.discarding_line = false;
        return;
    }

    active_response_target = target;
    parser.line_buffer[parser.line_length] = '\0';
    handleCommand(parser.line_buffer);
    parser.line_length = 0;
}

void appendCommandByte(CommandParser& parser, ResponseTarget target, uint8_t value)
{
    active_response_target = target;

    if (value == '\r') {
        finishLine(parser, target);
        parser.swallow_lf = true;
        return;
    }

    if (value == '\n') {
        if (parser.swallow_lf) {
            parser.swallow_lf = false;
            return;
        }

        finishLine(parser, target);
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
        QUEUE_FLASH("err:invalid_byte\n");
        return;
    }

    if (parser.line_length >= LineBufferSize - 1) {
        parser.line_length = 0;
        parser.discarding_line = true;
        ++line_too_long_count;
        QUEUE_FLASH("err:line_too_long\n");
        return;
    }

    parser.line_buffer[parser.line_length++] = static_cast<char>(value);
}

void serviceRx(unsigned long now)
{
#if defined(CDC_ENABLED)
    uint8_t processed = 0;

    while (processed < RxBudgetPerLoop && Serial.available() > 0) {
        const int value = Serial.read();
        if (value < 0) {
            break;
        }

        appendCommandByte(serial_parser, ResponseTarget::Serial, static_cast<uint8_t>(value));
        ++processed;
    }

    if (processed > 0) {
        serial_parser.last_rx_byte_at = now;
    }
#else
    (void)now;
#endif
}

void VendorHidTransport::serviceRx(CommandParser& parser, unsigned long now)
{
    uint8_t processed = 0;

    while (processed < RxBudgetPerLoop && USB_Available(outEndpoint()) > 0) {
        uint8_t report[USB_EP_SIZE] = {};
        const int received = USB_Recv(outEndpoint(), report, sizeof(report));
        if (received <= 0) {
            break;
        }

        ++processed;
        if (report[0] != VendorCommandReportId) {
            active_response_target = ResponseTarget::VendorHid;
            QUEUE_FLASH("err:bad_report_id\n");
            continue;
        }

        for (uint8_t i = 1; i < static_cast<uint8_t>(received); ++i) {
            const uint8_t value = report[i];
            if (value == 0) {
                break;
            }
            appendCommandByte(parser, ResponseTarget::VendorHid, value);
        }
    }

    if (processed > 0) {
        parser.last_rx_byte_at = now;
    }
}

void serviceLineTimeout(CommandParser& parser, ResponseTarget target, unsigned long now)
{
    if ((parser.line_length > 0 || parser.discarding_line) &&
        now - parser.last_rx_byte_at >= LineIdleTimeoutMs) {
        active_response_target = target;
        resetParser(parser);
        ++line_timeout_count;
        QUEUE_FLASH("err:line_timeout\n");
    }
}

void serviceFailsafe(unsigned long now)
{
    if ((keyboard.hasPressedKeys() || mouse_report.buttons != 0) &&
        now - last_lease_renewed_at >= FailsafeReleaseMs) {
        keyboard.releaseAll();
        releaseAllMouseButtons();
        updateStatusLed();
        ++failsafe_count;
        active_response_target = ResponseTarget::Broadcast;
        QUEUE_FLASH("warn:failsafe_release_all\n");
    }
}

void servicePortState(bool port_open, unsigned long now)
{
#if defined(CDC_ENABLED)
    if (port_open == previous_port_open) {
        return;
    }

    previous_port_open = port_open;
    resetParser(serial_parser);
    clearTxQueue(serial_tx);

    if (!port_open) {
        if (ReleaseOnSerialClose) {
            keyboard.releaseAll();
            releaseAllMouseButtons();
            updateStatusLed();
        }
        return;
    }

    last_lease_renewed_at = now;
    active_response_target = ResponseTarget::Serial;
    QUEUE_FLASH("mscv-keyboard:ready,protocol=2\n");
#else
    (void)port_open;
    (void)now;
#endif
}

} // namespace

namespace Firmware {

void setup()
{
    if (EnableStatusLed) {
        pinMode(LED_BUILTIN, OUTPUT);
        digitalWrite(LED_BUILTIN, LOW);
    }

#if defined(CDC_ENABLED)
    Serial.begin(SerialBaudRate);
#endif
    keyboard.begin();
    vendor_hid.begin();

    const unsigned long now = millis();
    serial_parser.last_rx_byte_at = now;
    vendor_parser.last_rx_byte_at = now;
    last_lease_renewed_at = now;
#if defined(CDC_ENABLED)
    previous_port_open = serialPortOpen();

    if (previous_port_open) {
        active_response_target = ResponseTarget::Serial;
        QUEUE_FLASH("mscv-keyboard:ready,protocol=2\n");
    }
#else
    previous_port_open = false;
#endif
}

void loop()
{
    unsigned long now = millis();
    bool port_open = serialPortOpen();

    servicePortState(port_open, now);
    serviceFailsafe(now);
    serviceLineTimeout(serial_parser, ResponseTarget::Serial, now);
    serviceLineTimeout(vendor_parser, ResponseTarget::VendorHid, now);

    serviceRx(now);
    vendor_hid.serviceRx(vendor_parser, now);

    // Re-check after command handling so failsafe cannot be starved by RX floods.
    now = millis();
    serviceFailsafe(now);

    // Replies have lower priority than HID input processing.
    port_open = serialPortOpen();
    vendor_hid.serviceTx();
    serviceTx(port_open);
    serviceBootloaderReset(millis());
}

} // namespace Firmware
