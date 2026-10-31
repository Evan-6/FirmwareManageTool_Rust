//! Portable Vendor HID v3 device engine.
//!
//! This is a line-by-line port of the C++ engine that the XIAO sketch compiles
//! today (`firmware/boards/rp2040/src/input/`). It owns sessions, output
//! scheduling, TX replies, fault handling and the bootloader gate, and it takes
//! `now_ms` from the caller: it never reads a clock, USB state or an LED.
//!
//! No heap, no floating point, no `std`. The only generated input is
//! `keys_generated.rs`, which comes from `shared/input-protocol/keys.json`.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

pub mod boot_gate;
pub mod config;
pub mod engine;
pub mod led;
pub mod scheduler;
pub mod session;
pub mod state;
pub mod wire;

mod keys_generated;

pub use boot_gate::BootloaderGate;
pub use engine::{BOOT_CONFIRMED, Counters, ERROR, FAILSAFE, InputEngine, PlatformInfo};
pub use led::{LedState, StatusIndicator};
pub use scheduler::{Completion, InputReport};
pub use session::{Pending, Session, SessionTable};
pub use state::InputState;
pub use wire::Report;
