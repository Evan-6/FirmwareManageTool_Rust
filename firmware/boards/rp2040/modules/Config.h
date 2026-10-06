// RP2040 internal module; included once by Firmware.cpp.
constexpr unsigned long FailsafeReleaseMs = 30000UL;
constexpr unsigned long LineIdleTimeoutMs = 1000UL;

constexpr uint8_t RxBudgetPerLoop = 64;
constexpr uint8_t LineBufferSize = 128;
constexpr uint8_t TxBufferSize = 128; // Must be a power of two and <= 256.
constexpr uint16_t RxQueueDepth = 128;

constexpr bool EnableKeyCommandAcks = true;
constexpr bool EnableStatusLed = FIRMWARE_ENABLE_NEOPIXEL != 0;
constexpr bool EnableRgbAnimations = true; // false => flat colours, no fades.
#
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
constexpr uint8_t AbsMouseReportId = 4;
constexpr uint8_t MaxNonModifierKeys = 6;
constexpr uint8_t VendorCommandReportId = 10;  // OUT: host -> device
constexpr uint8_t VendorResponseReportId = 11; // IN:  device -> host
constexpr uint8_t VendorHidPayloadSize = 63;   // 64-byte endpoint minus report id.
constexpr unsigned long BootloaderResetDelayMs = 120UL;

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

static_assert((TxBufferSize & (TxBufferSize - 1)) == 0, "TxBufferSize must be a power of two");
