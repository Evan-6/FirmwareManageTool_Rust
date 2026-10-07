constexpr bool EnableStatusLed = FIRMWARE_ENABLE_NEOPIXEL != 0;
constexpr bool EnableRgbAnimations = true; // false => flat colours, no fades.
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
constexpr uint8_t ConsumerReportId = 5;
constexpr uint8_t MouseReportId = 3;
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

constexpr unsigned long BootloaderResetDelayMs = 120;
constexpr uint8_t V3OutputDepth = 32;
constexpr uint8_t V3BoardId =
#if defined(ARDUINO_SEEED_XIAO_RP2040)
    2;
#else
    1;
#endif
#define V3_PROGMEM
using AtomicBool = std::atomic<bool>;
using AtomicByte = std::atomic<uint8_t>;
using V3RxQueue = queue_t;
using V3TxQueue = queue_t;
void platformDeviceId(uint8_t *id) {
    pico_unique_board_id_t uid;
    pico_get_unique_board_id(&uid);
    memcpy(id, uid.id, 8);
}
bool bootloader_reset_pending = false;
unsigned long bootloader_reset_requested_at = 0;
void scheduleBootloaderReset();
