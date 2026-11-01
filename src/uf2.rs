//! RP2040 ELF → UF2 packaging.
//!
//! The packer moved into the SDK (`gdb::flash::uf2`) because flashing owns it;
//! this module stays as the two names the build script (`src/bin/uf2pack.rs`)
//! calls.

pub use gdb::flash::uf2::{blocks, pack};
