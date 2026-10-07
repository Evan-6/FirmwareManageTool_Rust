#pragma once
#include "BoardConfig.h"
#include "StatusIndicator.h"
#if FIRMWARE_ENABLE_NEOPIXEL
#include <Adafruit_NeoPixel.h>
#endif
namespace hidfw {
struct Rgb {
    uint8_t r, g, b;
};
constexpr bool operator==(const Rgb &a, const Rgb &b) {
    return a.r == b.r && a.g == b.g && a.b == b.b;
}
class StatusLedRenderer {
  public:
    void begin(uint32_t now);
    void render(uint32_t now, LedState state);

  private:
#if FIRMWARE_ENABLE_NEOPIXEL
    Adafruit_NeoPixel pixels_{1, board::NeoPixelDataPin, NEO_GRB + NEO_KHZ800};
    uint32_t last_frame_at_ = 0;
    Rgb current_{0, 0, 0}, shown_{255, 255, 255};
#endif
};
} // namespace hidfw
