//! RP2040 ELF → UF2 packaging.
//!
//! The packer moved into the SDK (`gdb::flash::uf2`) because flashing owns it;
//! this module stays as the name the build script (`src/bin/uf2pack.rs`) uses.

pub use gdb::flash::uf2::{FAMILY_RP2040, XIP_BASE, XIP_LIMIT, blocks, pack};
