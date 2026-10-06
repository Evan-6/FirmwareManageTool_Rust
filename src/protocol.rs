//! Versioned wire codecs; v2 remains available for AVR and older RP2040.
mod v2;
pub use input_protocol::{KeyId, v3};
pub use v2::*;
