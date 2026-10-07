#include "Firmware.h"
#include "src/runtime/FirmwareRuntime.h"
namespace {
hidfw::FirmwareRuntime runtime;
}
namespace Firmware {
void setup() { runtime.begin(); }
void loop() { runtime.loop(); }
} // namespace Firmware
