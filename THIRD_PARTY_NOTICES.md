# Third-party notices

The root MIT license applies to the new Rust application and its documentation/scripts.
It does not relicense the copied `firmware/` sources or third-party Rust dependencies.

- `firmware/` was copied from the existing sibling FirmwareManageTool project. Existing
  copyright notices and licenses remain applicable; project-specific firmware without
  an explicit license is included as supplied, without a new license grant.
- Arduino CLI, the arduino-pico RP2040 core and Adafruit NeoPixel downloaded during operation retain
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
- arduino-pico: https://github.com/earlephilhower/arduino-pico
- Adafruit NeoPixel: https://github.com/adafruit/Adafruit_NeoPixel
