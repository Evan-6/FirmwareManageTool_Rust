//! Transport-agnostic firmware runtime plus the USB report descriptors that the
//! shipping C++ sketch compiles.
//!
//! The library half is `no_std` and host-testable; the embassy USB stack lives in
//! `main.rs` behind the `hardware` feature because it only builds for the device.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

pub mod runtime;
pub mod usb_descriptors;

pub use runtime::{Runtime, RxFault, Transport};
