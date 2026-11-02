//! GDBOX host SDK.
//!
//! One `Device` owns one vendor-HID session; every feature in the SDK is a
//! method on it or on a small handle it hands out (`device.mouse()`,
//! `device.keyboard()`, `device.scheduler()`). The firmware only moves bytes;
//! everything here — absolute coordinates, motion, automation — is host side.

pub mod broker;
pub mod device;
pub mod error;
pub mod event;
pub mod flash;
pub mod keyboard;
pub mod log;
pub mod manager;
pub mod metrics;
pub mod mock;
pub mod motion;
pub mod mouse;
pub mod platform;
pub mod probe;
pub mod replay;
pub mod task;
pub mod timing;
pub mod transport;
pub mod watch;

mod session;

pub use broker::{Client as BrokerClient, Server as BrokerServer};
pub use device::{Device, DeviceInfo, DeviceOptions, Ownership, StrayHook};
pub use error::{Error, ErrorCode, Result};
pub use event::{ConnectionState, EventHandler};
pub use keyboard::{ChordConfig, KeyStroke, TypeTextConfig};
pub use log::Level;
pub use manager::{DeviceEntry, Manager};
pub use metrics::MetricsSnapshot;
pub use motion::{MotionOptions, Movement};
pub use mouse::Button;
pub use probe::{Observer, Probe, ProbeEvent};
pub use replay::{Recorder, Recording};
pub use task::{Priority, Scheduler, TaskId};
pub use transport::Transport;
pub use watch::{Watch, WatchEvent};

/// Vendor HID identity. This is the project's own id — an unassigned pid.codes
/// PID under the open-source `0x1209` vendor id — so the device presents itself
/// as the keyboard it is instead of borrowing another vendor's model number.
/// `firmware/app/src/main.rs` must carry the same pair; `gdb_probe` answers only
/// this one.
pub const VENDOR_ID: u16 = 0x1209;
pub const PRODUCT_ID: u16 = 0x0B0B;
pub const USAGE_PAGE: u16 = 0xFF60;
pub const USAGE: u16 = 0x61;
