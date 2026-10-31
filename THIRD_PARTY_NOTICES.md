# Third-party notices

The root MIT license applies to the new Rust application and its documentation/scripts.
It does not relicense the copied `firmware/` sources or third-party Rust dependencies.

- `firmware/` was copied from the existing sibling FirmwareManageTool project. Existing
  copyright notices and licenses remain applicable; project-specific firmware without
  an explicit license is included as supplied, without a new license grant.
- The firmware builds from Rust sources only (embassy-rp / embassy-usb and their
  dependencies, pinned by `firmware/app/Cargo.lock`). No Arduino core, TinyUSB or
  Adafruit library is downloaded or linked any more.
- Windows fonts are loaded from the operating system and are not redistributed.
- Rust dependency versions are fixed by Cargo.lock. Packaging collects upstream
  LICENSE/LICENCE/COPYING/NOTICE files into `ThirdPartyLicenses/`, together with a
  machine-readable dependency list. This includes platform-specific and build-time
  packages as well as runtime dependencies.

Primary upstream projects:

- Tauri 2: https://github.com/tauri-apps/tauri (MIT OR Apache-2.0)
- React / React DOM: https://github.com/facebook/react (MIT)
- Vite: https://github.com/vitejs/vite (MIT)
- hidapi Rust bindings: https://github.com/ruabmbua/hidapi-rs (MIT)
- HIDAPI C library: https://github.com/libusb/hidapi (upstream dual/triple license;
  license files are included from the hidapi package)
- embassy: https://github.com/embassy-rs/embassy (MIT OR Apache-2.0)
