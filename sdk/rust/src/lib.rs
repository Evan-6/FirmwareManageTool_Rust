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
pub mod replay;
pub mod task;
pub mod transport;
pub mod watch;

mod session;

pub use broker::{Client as BrokerClient, Server as BrokerServer};
pub use device::{Device, DeviceInfo, DeviceOptions};
pub use error::{Error, ErrorCode, Result};
pub use event::{ConnectionState, EventHandler};
pub use keyboard::{ChordConfig, KeyStroke, TypeTextConfig};
pub use log::Level;
pub use manager::{DeviceEntry, Manager};
pub use metrics::MetricsSnapshot;
pub use motion::{MotionOptions, Movement};
pub use mouse::Button;
pub use replay::{Recorder, Recording};
pub use task::{Priority, Scheduler, TaskId};
pub use transport::Transport;
pub use watch::{Watch, WatchEvent};

/// Vendor HID identity. The firmware answers only this pair.
pub const VENDOR_ID: u16 = 0x03F0;
pub const PRODUCT_ID: u16 = 0x0024;
pub const USAGE_PAGE: u16 = 0xFF60;
pub const USAGE: u16 = 0x61;
