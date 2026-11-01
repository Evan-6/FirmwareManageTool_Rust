//! Arrival and removal callbacks over a scripted bus: no HID, no drive, no
//! hardware.

use gdb::manager::DeviceEntry;
use gdb::watch::{Inventory, Listing, Watch, WatchEvent};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

fn entry(serial: &str) -> DeviceEntry {
    DeviceEntry {
        path: format!("\\\\?\\hid#{serial}"),
        serial: serial.to_string(),
        product: "GDBOX".to_string(),
        manufacturer: "Seeed".to_string(),
        vid: gdb::VENDOR_ID,
        pid: gdb::PRODUCT_ID,
    }
}

/// A bus that answers from a script: each poll consumes one step and the last
/// step repeats, so a slow thread can never fall off the end of the list.
struct Scripted {
    steps: Mutex<Vec<(Vec<DeviceEntry>, Vec<PathBuf>)>>,
}

impl Scripted {
    fn new(steps: Vec<(Vec<DeviceEntry>, Vec<PathBuf>)>) -> Self {
        Self {
            steps: Mutex::new(steps),
        }
    }
}

impl Inventory for Scripted {
    fn listing(&self) -> gdb::Result<Listing> {
        let mut steps = self.steps.lock().unwrap();
        let (devices, volumes) = if steps.len() > 1 {
            steps.remove(0)
        } else {
            steps[0].clone()
        };
        Ok(Listing { devices, volumes })
    }
}

#[test]
fn a_device_that_appears_arrives_and_a_device_that_goes_is_removed() {
    let (sender, events) = channel();
    let bus = Scripted::new(vec![
        (vec![entry("A")], vec![]),
        (vec![entry("A")], vec![]),
        (vec![], vec![]),
    ]);
    let mut watch = Watch::start_with(
        Box::new(bus),
        Duration::from_millis(20),
        move |listing, event| {
            let _ = sender.send((listing.len(), event));
        },
    )
    .expect("watch");

    let (seen, event) = events
        .recv_timeout(Duration::from_secs(2))
        .expect("the first poll reports what is already there");
    // The callback always sees the whole current listing.
    assert_eq!(seen, 1);
    match event {
        WatchEvent::Arrived(found) => assert_eq!(found.serial, "A"),
        other => panic!("expected an arrival, got {other:?}"),
    }

    let (seen, event) = events
        .recv_timeout(Duration::from_secs(2))
        .expect("an unplugged device is reported");
    assert_eq!(seen, 0);
    match event {
        WatchEvent::Removed(found) => assert_eq!(found.serial, "A"),
        other => panic!("expected a removal, got {other:?}"),
    }

    // An unchanged bus is silent: the second step repeats step one, so no
    // duplicate arrival may be reported for it.
    assert!(
        events.recv_timeout(Duration::from_millis(200)).is_err(),
        "an unchanged listing must not produce an event"
    );
    watch.stop();
}

#[test]
fn a_bootsel_volume_is_reported_as_a_bootloader_arrival() {
    let (sender, events) = channel();
    let root = PathBuf::from("R:\\");
    let bus = Scripted::new(vec![(vec![], vec![]), (vec![], vec![root.clone()])]);
    let mut watch = Watch::start_with(
        Box::new(bus),
        Duration::from_millis(20),
        move |listing, event| {
            let _ = sender.send((listing.len(), event));
        },
    )
    .expect("watch");

    let (_, event) = events
        .recv_timeout(Duration::from_secs(2))
        .expect("a plugged-in UF2 drive is reported");
    match event {
        WatchEvent::BootloaderArrived(path) => assert_eq!(path, root),
        other => panic!("expected a bootloader arrival, got {other:?}"),
    }

    assert!(
        events.recv_timeout(Duration::from_millis(200)).is_err(),
        "a bootloader volume that stays mounted is reported once"
    );
    watch.stop();
}

#[test]
fn stop_ends_the_thread_within_two_intervals() {
    let bus = Scripted::new(vec![(vec![], vec![])]);
    let interval = Duration::from_millis(100);
    let mut watch = Watch::start_with(Box::new(bus), interval, |_, _| {}).expect("watch");
    assert!(watch.is_running());
    std::thread::sleep(Duration::from_millis(20));

    let started = Instant::now();
    watch.stop();
    let elapsed = started.elapsed();
    assert!(
        elapsed < interval * 2,
        "stop took {elapsed:?}, longer than two {interval:?} intervals"
    );
    assert!(!watch.is_running());
    // Stopping twice, and dropping a stopped watch, must not hang or panic.
    watch.stop();
}

#[test]
fn a_short_interval_is_rounded_up_instead_of_spinning() {
    // 1 ms would enumerate 1000 times a second; the watch clamps to 10 ms and
    // still reports the first listing immediately.
    let (sender, events) = channel();
    let bus = Scripted::new(vec![(vec![entry("A")], vec![])]);
    let mut watch = Watch::start_with(Box::new(bus), Duration::from_millis(1), move |_, event| {
        let _ = sender.send(event);
    })
    .expect("watch");
    assert!(matches!(
        events.recv_timeout(Duration::from_secs(2)),
        Ok(WatchEvent::Arrived(_))
    ));
    watch.stop();
}
