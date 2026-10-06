#pragma once
#include <algorithm>
#include <atomic>
#include <stdint.h>
extern unsigned long test_time;
inline unsigned long millis() { return test_time; }
inline void delay(unsigned long n) { test_time += n; }
inline void yield() {}
template <class T> T constrain(T n, T a, T b) { return std::min(b, std::max(a, n)); }
struct SerialStub {
    void end() {}
};
inline SerialStub Serial;
struct PicoStub {
    bool rebooted = false;
    void rebootToBootloader() { rebooted = true; }
};
inline PicoStub rp2040;
