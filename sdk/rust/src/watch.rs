//! Device arrivals and removals without polling the bus.
//!
//! A host that wants to know when the board is plugged in should not run its own
//! enumeration loop on the UI thread: [`Watch`] owns one thread that does it and
//! hands the answer to a callback.
//!
//! The enumeration is [`Manager::list`], which only walks the HID device table —
//! it never reads or writes a report on the vendor collection, so watching
//! cannot steal a reply from the session that owns the board (the trap in
//! AGENTS.md). Volumes come from [`crate::flash::drives`], which reads
//! `INFO_UF2.TXT` on each drive letter, and a bootloader drive that appears is
//! reported as [`WatchEvent::BootloaderArrived`].
//!
//! The first poll reports everything already present as an arrival, so a host
//! that starts late still sees the whole bus instead of waiting for the next
//! plug-in.

use crate::error::{Error, Result};
use crate::manager::{DeviceEntry, Manager};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Fifty polls a second is faster than anyone unplugs a cable and cheap enough
/// to leave running.
pub const DEFAULT_INTERVAL: Duration = Duration::from_millis(500);

/// Below this the watch thread would spend its life enumerating; a shorter
/// request is rounded up.
const MIN_INTERVAL: Duration = Duration::from_millis(10);

/// What changed between two polls of the bus.
#[derive(Clone, Debug)]
pub enum WatchEvent {
    /// A vendor HID device is now present.
    Arrived(DeviceEntry),
    /// A vendor HID device is gone.
    Removed(DeviceEntry),
    /// An RP2040 bootloader drive appeared; the path is its root.
    BootloaderArrived(PathBuf),
}

/// One look at the bus.
#[derive(Clone, Debug, Default)]
pub struct Listing {
    /// Enumerated vendor HID devices, in the order the platform reported them.
    pub devices: Vec<DeviceEntry>,
    /// Mounted RP2040 bootloader volumes.
    pub volumes: Vec<PathBuf>,
}

/// Where a [`Watch`] gets its listings.
///
/// [`SystemInventory`] is the real one. It is a seam because a test needs to
/// script arrivals and removals without plugging anything in — and because a
/// host that already owns its own HID backend can supply its own enumeration
/// rather than have the SDK open a second handle.
pub trait Inventory: Send + 'static {
    fn listing(&self) -> Result<Listing>;
}

/// hidapi enumeration plus the Windows drive table: no report is read or written.
pub struct SystemInventory;

impl Inventory for SystemInventory {
    fn listing(&self) -> Result<Listing> {
        Ok(Listing {
            devices: Manager::list()?,
            // Volumes are a bonus: a drive table that cannot be read (another
            // platform, no permission) must not stop device events.
            volumes: crate::flash::drives().unwrap_or_default(),
        })
    }
}

/// A running arrival/removal watch. Dropping it stops the thread.
pub struct Watch {
    control: Arc<Control>,
    thread: Option<JoinHandle<()>>,
}

struct Control {
    stopped: Mutex<bool>,
    wake: Condvar,
}

impl Default for Control {
    fn default() -> Self {
        Self {
            stopped: Mutex::new(false),
            wake: Condvar::new(),
        }
    }
}

impl Control {
    fn stop(&self) {
        let mut stopped = self
            .stopped
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        *stopped = true;
        drop(stopped);
        self.wake.notify_all();
    }

    fn is_stopped(&self) -> bool {
        *self
            .stopped
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// Sleeps for one interval, returning as soon as `stop` is called, so a host
    /// never waits a whole interval to shut down.
    fn wait(&self, interval: Duration) {
        let stopped = self
            .stopped
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let (stopped, _) = self
            .wake
            .wait_timeout_while(stopped, interval, |stopped| !*stopped)
            .unwrap_or_else(|poison| poison.into_inner());
        drop(stopped);
    }
}

impl Watch {
    /// Starts watching the real bus. `callback` runs on the watch thread and must
    /// not block: it is the only thread polling, so a slow callback delays the
    /// next diff.
    pub fn start(
        interval: Duration,
        callback: impl Fn(&[DeviceEntry], WatchEvent) + Send + 'static,
    ) -> Result<Self> {
        Self::start_with(Box::new(SystemInventory), interval, callback)
    }

    /// Starts watching an arbitrary [`Inventory`]; the seam every test uses.
    pub fn start_with(
        inventory: Box<dyn Inventory>,
        interval: Duration,
        callback: impl Fn(&[DeviceEntry], WatchEvent) + Send + 'static,
    ) -> Result<Self> {
        let control = Arc::new(Control::default());
        let running = control.clone();
        let interval = interval.max(MIN_INTERVAL);
        let handle = thread::Builder::new()
            .name("gdb-watch".to_string())
            .spawn(move || run(inventory, interval, running, callback))
            .map_err(|error| Error::io(error.to_string()))?;
        Ok(Self {
            control,
            thread: Some(handle),
        })
    }

    /// Stops the thread and waits for it. Safe to call twice.
    pub fn stop(&mut self) {
        self.control.stop();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }

    /// True while the thread has not been stopped.
    pub fn is_running(&self) -> bool {
        self.thread.is_some()
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run(
    inventory: Box<dyn Inventory>,
    interval: Duration,
    control: Arc<Control>,
    callback: impl Fn(&[DeviceEntry], WatchEvent),
) {
    let mut devices: Vec<DeviceEntry> = Vec::new();
    let mut volumes: Vec<PathBuf> = Vec::new();
    let mut reported = false;
    while !control.is_stopped() {
        match inventory.listing() {
            Ok(listing) => {
                emit(&devices, &volumes, &listing, &callback);
                devices = listing.devices;
                volumes = listing.volumes;
                reported = false;
            }
            // A bus that cannot be read is not an empty bus: keep the last
            // listing and try again rather than reporting a mass removal.
            Err(error) => {
                if !reported {
                    reported = true;
                    crate::sdk_log!(
                        crate::log::Level::Warn,
                        "watch could not enumerate: {}",
                        error.message
                    );
                }
            }
        }
        control.wait(interval);
    }
}

/// Reports the difference between the last listing and this one, newest bus
/// state included in every call.
fn emit(
    devices: &[DeviceEntry],
    volumes: &[PathBuf],
    listing: &Listing,
    callback: &impl Fn(&[DeviceEntry], WatchEvent),
) {
    let current = &listing.devices;
    for entry in current {
        if !devices.iter().any(|old| key(old) == key(entry)) {
            callback(current, WatchEvent::Arrived(entry.clone()));
        }
    }
    for entry in devices {
        if !current.iter().any(|new| key(new) == key(entry)) {
            callback(current, WatchEvent::Removed(entry.clone()));
        }
    }
    for volume in &listing.volumes {
        if !volumes.contains(volume) {
            callback(current, WatchEvent::BootloaderArrived(volume.clone()));
        }
    }
}

/// Identity across polls: the serial number, or the HID path when the device
/// reports none (the mock and some hubs do not).
fn key(entry: &DeviceEntry) -> &str {
    if entry.serial.is_empty() {
        &entry.path
    } else {
        &entry.serial
    }
}
