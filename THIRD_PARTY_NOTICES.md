# Third-party notices

The root MIT license applies to the new Rust application and its documentation/scripts.
It does not relicense the copied `firmware/` sources or third-party Rust dependencies.

- `firmware/` was copied from the existing sibling FirmwareManageTool project. Existing
  copyright notices and licenses remain applicable; project-specific firmware without
  an explicit license is included as supplied, without a new license grant.
- The bundled Arduino AVR core, HID library, and Keyboard library contain Arduino and
  contributor copyright notices. See individual source headers and
  `firmware/boards/leonardo_avr/libraries/Keyboard/LICENSE` (GNU LGPL 2.1).
  Full firmware source is provided to permit inspection and modification.
- Arduino CLI, AVR/RP2040 cores and Adafruit NeoPixel downloaded during operation retain
  their own licenses. They are installed separately and are not included as binaries
  in this application's portable ZIP.
- Windows fonts are loaded from the operating system and are not redistributed.
- Rust dependency versions are fixed by Cargo.lock. Packaging collects upstream
  LICENSE/LICENCE/COPYING/NOTICE files into `ThirdPartyLicenses/`, together with a
  machine-readable dependency list. This includes platform-specific and build-time
  packages as well as runtime dependencies.

Primary upstream projects:

- egui/eframe: https://github.com/emilk/egui (MIT OR Apache-2.0)
- hidapi Rust bindings: https://github.com/ruabmbua/hidapi-rs (MIT)
- HIDAPI C library: https://github.com/libusb/hidapi (upstream dual/triple license;
  license files are included from the hidapi package)
- Arduino AVR core: https://github.com/arduino/ArduinoCore-avr
- Arduino Keyboard: https://github.com/arduino-libraries/Keyboard
- arduino-pico: https://github.com/earlephilhower/arduino-pico
- Adafruit NeoPixel: https://github.com/adafruit/Adafruit_NeoPixel
