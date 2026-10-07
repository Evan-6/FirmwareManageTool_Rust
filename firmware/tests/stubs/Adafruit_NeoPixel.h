#pragma once
#include <stdint.h>
#include <vector>
#define NEO_GRB 1
#define NEO_KHZ800 2
inline std::vector<uint32_t> test_pixel_frames;
class Adafruit_NeoPixel {
    uint32_t colour_ = 0;

  public:
    Adafruit_NeoPixel(unsigned, unsigned, unsigned) {}
    void begin() {}
    void setBrightness(uint8_t) {}
    static uint32_t Color(uint8_t r, uint8_t g, uint8_t b) {
        return uint32_t(r) << 16 | uint32_t(g) << 8 | b;
    }
    void setPixelColor(unsigned, uint32_t colour) { colour_ = colour; }
    void show() { test_pixel_frames.push_back(colour_); }
};
