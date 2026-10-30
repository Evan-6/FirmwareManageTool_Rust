//! Seeed XIAO RP2040 firmware: two HID interfaces driven by the Rust engine.
//!
//! Interface 0 is the input HID (NKRO id 2, Consumer id 5, relative mouse id 3),
//! interface 1 is the vendor channel (OUT 12 / IN 13 / Feature 14). The report
//! descriptors are frozen in `usb_descriptors.rs` and pinned by
//! `tests/descriptors.rs`; Feature 14 must answer 64 bytes with the report id in
//! `buf[0]`, or Windows rejects it as a short report.
//!
//! The main loop keeps the stage order the C++ runtime used to have:
//! `Runtime::step` submits at most one report, and the awaited write reports its
//! completion on the next pass, so "accepted" is never confused with "sent".

#![no_std]
#![no_main]

use core::cell::Cell;

use embassy_executor::Spawner;
use embassy_futures::join::join3;
use embassy_rp::bind_interrupts;
use embassy_rp::peripherals::USB;
use embassy_rp::usb::{Driver, InterruptHandler};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Instant, Ticker};
use embassy_usb::class::hid::{
    Config as HidConfig, HidBootProtocol, HidReader, HidReaderWriter, HidSubclass, HidWriter,
    ReportId, RequestHandler, State,
};
use embassy_usb::control::OutResponse;
use embassy_usb::{Builder, Config, Handler};
use firmware_engine::{InputReport, Report, wire};
use panic_halt as _;
use xiao_firmware::{Runtime, RxFault, Transport, usb_descriptors};

bind_interrupts!(struct Irqs {
    USBCTRL_IRQ => InterruptHandler<USB>;
});

/// Feature 14 `board` byte; must match `src/firmware.rs::BOARD_ID` on the host.
const BOARD_ID: u8 = 2;
const FLASH_SIZE: usize = 2 * 1024 * 1024;
const REPORT_SIZE: usize = 64;
const INPUT_REPORT_SIZE: usize = 1 + 29;
const VENDOR_REPORT_ID: u8 = 13;
const DATA_REPORT_ID: u8 = 12;
const POLL_MS: u8 = 1;
const MAX_POWER_MA: u16 = 100;
const DEVICE_RELEASE: u16 = 0x0310;
const RX_DEPTH: usize = 32;

/// RP2040 (Cortex-M0+) has no atomic read-modify-write, so shared flags live in a
/// critical-section cell instead of `AtomicU16::fetch_add`, which does not exist
/// on this target.
#[derive(Clone, Copy)]
struct Flags {
    configured: bool,
    reset: bool,
    framing: u16,
    overflow: u16,
}

impl Flags {
    const fn new() -> Self {
        Self {
            configured: false,
            reset: false,
            framing: 0,
            overflow: 0,
        }
    }
}

static FLAGS: Mutex<CriticalSectionRawMutex, Cell<Flags>> = Mutex::new(Cell::new(Flags::new()));
static RX: Channel<CriticalSectionRawMutex, Report, RX_DEPTH> = Channel::new();
static FEATURE: Mutex<CriticalSectionRawMutex, Cell<[u8; 63]>> = Mutex::new(Cell::new([0; 63]));

fn update_flags(update: impl FnOnce(&mut Flags)) {
    FLAGS.lock(|cell| {
        let mut flags = cell.get();
        update(&mut flags);
        cell.set(flags);
    });
}

struct DeviceHandler;

impl Handler for DeviceHandler {
    fn configured(&mut self, configured: bool) {
        update_flags(|flags| flags.configured = configured);
    }

    fn reset(&mut self) {
        update_flags(|flags| {
            flags.configured = false;
            flags.reset = true;
        });
    }
}

/// Feature 14 (read-only capabilities) and the keyboard LED output report.
struct VendorRequests;

impl RequestHandler for VendorRequests {
    fn get_report(&mut self, id: ReportId, buf: &mut [u8]) -> Option<usize> {
        match id {
            // Windows hands back the report ID as byte 0 for reports that declare
            // one, so the 63-byte payload has to be shifted by one and the total
            // length stays 64 (a 63-byte reply is rejected as a short report).
            ReportId::Feature(14) if buf.len() >= 64 => {
                buf[0] = 14;
                buf[1..64].copy_from_slice(&FEATURE.lock(|cell| cell.get()));
                Some(64)
            }
            _ => None,
        }
    }

    fn set_report(&mut self, _id: ReportId, _data: &[u8]) -> OutResponse {
        // Lock-key LEDs: accepted and ignored, exactly like the C++ callback.
        OutResponse::Accepted
    }
}

struct UsbTransport<'d> {
    input: HidWriter<'d, Driver<'d, USB>, REPORT_SIZE>,
    vendor: HidWriter<'d, Driver<'d, USB>, REPORT_SIZE>,
    pending_input: Option<InputReport>,
    inflight_input: Option<u8>,
    input_failed: bool,
    pending_vendor: Option<(Report, u32)>,
    vendor_complete: Option<u32>,
}

impl UsbTransport<'_> {
    async fn flush_input(&mut self) {
        let Some(report) = self.pending_input.take() else {
            return;
        };
        let length = report.length as usize;
        let mut buffer = [0u8; INPUT_REPORT_SIZE];
        buffer[0] = report.id;
        buffer[1..1 + length].copy_from_slice(&report.data[..length]);
        match self.input.write(&buffer[..1 + length]).await {
            Ok(()) => self.inflight_input = Some(report.id),
            Err(_) => self.input_failed = true,
        }
    }

    async fn flush_vendor(&mut self) {
        let Some((report, generation)) = self.pending_vendor.take() else {
            return;
        };
        let mut buffer = [0u8; REPORT_SIZE];
        buffer[0] = VENDOR_REPORT_ID;
        buffer[1..].copy_from_slice(&report.bytes);
        if self.vendor.write(&buffer).await.is_ok() {
            self.vendor_complete = Some(generation);
        }
    }
}

impl Transport for UsbTransport<'_> {
    fn mounted(&mut self) -> bool {
        FLAGS.lock(|cell| cell.get().configured)
    }

    fn input_ready(&mut self) -> bool {
        self.pending_input.is_none() && self.inflight_input.is_none()
    }

    fn vendor_ready(&mut self) -> bool {
        self.pending_vendor.is_none()
    }

    fn send_input(&mut self, report: &InputReport) -> bool {
        if self.pending_input.is_some() {
            return false;
        }
        self.pending_input = Some(*report);
        true
    }

    fn send_vendor(&mut self, report: &Report, boot_generation: u32) -> bool {
        if self.pending_vendor.is_some() {
            return false;
        }
        self.pending_vendor = Some((*report, boot_generation));
        true
    }

    fn read(&mut self, report: &mut Report) -> bool {
        match RX.try_receive() {
            Ok(packet) => {
                *report = packet;
                true
            }
            Err(_) => false,
        }
    }

    fn discard_rx(&mut self) {
        while RX.try_receive().is_ok() {}
    }

    fn take_input_complete(&mut self) -> u8 {
        self.inflight_input.take().unwrap_or(0)
    }

    fn take_input_failure(&mut self) -> bool {
        core::mem::take(&mut self.input_failed)
    }

    fn take_vendor_complete(&mut self) -> Option<u32> {
        self.vendor_complete.take()
    }

    fn take_reset(&mut self) -> bool {
        let reset = FLAGS.lock(|cell| {
            let mut flags = cell.get();
            let pending = flags.reset;
            flags.reset = false;
            cell.set(flags);
            pending
        });
        if !reset {
            return false;
        }
        // A reset invalidates every in-flight transfer, as in the C++ transport.
        self.inflight_input = None;
        self.input_failed = false;
        self.pending_vendor = None;
        self.vendor_complete = None;
        true
    }

    fn take_rx_fault(&mut self) -> RxFault {
        let (framing, overflow) = FLAGS.lock(|cell| {
            let mut flags = cell.get();
            let counts = (flags.framing, flags.overflow);
            flags.framing = 0;
            flags.overflow = 0;
            cell.set(flags);
            counts
        });
        let code = if framing != 0 {
            1
        } else if overflow != 0 {
            8
        } else {
            0
        };
        RxFault {
            code,
            errors: framing.saturating_add(overflow),
        }
    }
}

/// Reads vendor OUT reports (id 12) into the engine's RX queue.
async fn vendor_reader<'d>(mut reader: HidReader<'d, Driver<'d, USB>, REPORT_SIZE>) -> ! {
    let mut buffer = [0u8; REPORT_SIZE];
    loop {
        match reader.read(&mut buffer).await {
            Ok(REPORT_SIZE) if buffer[0] == DATA_REPORT_ID => {
                let mut report = Report::default();
                report.bytes.copy_from_slice(&buffer[1..REPORT_SIZE]);
                if RX.try_send(report).is_err() {
                    update_flags(|flags| flags.overflow = flags.overflow.saturating_add(1));
                }
            }
            // TinyUSB also hands over the payload without a report id.
            Ok(63) => {
                let mut report = Report::default();
                report.bytes.copy_from_slice(&buffer[..63]);
                if RX.try_send(report).is_err() {
                    update_flags(|flags| flags.overflow = flags.overflow.saturating_add(1));
                }
            }
            Ok(_) => update_flags(|flags| flags.framing = flags.framing.saturating_add(1)),
            Err(_) => update_flags(|flags| flags.framing = flags.framing.saturating_add(1)),
        }
    }
}

async fn engine_loop(transport: UsbTransport<'_>) -> ! {
    let mut runtime = Runtime::new(transport);
    let mut ticker = Ticker::every(Duration::from_millis(1));
    loop {
        let now = Instant::now().as_millis() as u32;
        runtime.step(now);
        runtime.transport_mut().flush_input().await;
        runtime.transport_mut().flush_vendor().await;
        if runtime.take_boot_request() {
            embassy_rp::rom_data::reset_to_usb_boot(0, 0);
        }
        ticker.next().await;
    }
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    // Flash unique id (same source as pico_get_unique_board_id), printed as 16
    // upper-case hex characters so the host can tell boards apart.
    let mut id = [0u8; 8];
    {
        let mut flash =
            embassy_rp::flash::Flash::<_, embassy_rp::flash::Blocking, FLASH_SIZE>::new_blocking(
                p.FLASH,
            );
        let _ = flash.blocking_unique_id(&mut id);
    }
    FEATURE.lock(|cell| {
        cell.set(wire::feature(&wire::DeviceInfo {
            board: BOARD_ID,
            id,
        }))
    });

    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut serial = [b'0'; 16];
    for (index, byte) in id.iter().enumerate() {
        serial[2 * index] = HEX[(byte >> 4) as usize];
        serial[2 * index + 1] = HEX[(byte & 0x0f) as usize];
    }
    let serial = core::str::from_utf8(&serial).unwrap_or("0000000000000000");

    // embassy-usb states and handlers must outlive the driver borrow, so they are
    // declared first and therefore dropped last.
    let mut device_handler = DeviceHandler;
    let mut input_state = State::new();
    let mut vendor_state = State::new();
    let mut vendor_requests = VendorRequests;

    let driver = Driver::new(p.USB, Irqs);
    let mut config = Config::new(0x03f0, 0x0024);
    config.manufacturer = Some("HP");
    config.product = Some("HP Keyboard");
    config.serial_number = Some(serial);
    config.device_release = DEVICE_RELEASE;
    config.max_power = MAX_POWER_MA;
    config.max_packet_size_0 = 64;

    let mut config_descriptor = [0u8; 256];
    let mut bos_descriptor = [0u8; 256];
    let mut msos_descriptor = [0u8; 256];
    let mut control_buffer = [0u8; 64];
    let mut builder = Builder::new(
        driver,
        config,
        &mut config_descriptor,
        &mut bos_descriptor,
        &mut msos_descriptor,
        &mut control_buffer,
    );
    builder.handler(&mut device_handler);

    let input = HidWriter::<_, REPORT_SIZE>::new(
        &mut builder,
        &mut input_state,
        HidConfig {
            report_descriptor: usb_descriptors::INPUT_REPORT_DESCRIPTOR,
            request_handler: None,
            poll_ms: POLL_MS,
            max_packet_size: REPORT_SIZE as u16,
            hid_subclass: HidSubclass::No,
            hid_boot_protocol: HidBootProtocol::None,
        },
    );

    let vendor = HidReaderWriter::<_, REPORT_SIZE, REPORT_SIZE>::new(
        &mut builder,
        &mut vendor_state,
        HidConfig {
            report_descriptor: usb_descriptors::VENDOR_REPORT_DESCRIPTOR,
            request_handler: Some(&mut vendor_requests),
            poll_ms: POLL_MS,
            max_packet_size: REPORT_SIZE as u16,
            hid_subclass: HidSubclass::No,
            hid_boot_protocol: HidBootProtocol::None,
        },
    );
    let (vendor_reader_half, vendor_writer_half) = vendor.split();

    let mut usb = builder.build();

    let transport = UsbTransport {
        input,
        vendor: vendor_writer_half,
        pending_input: None,
        inflight_input: None,
        input_failed: false,
        pending_vendor: None,
        vendor_complete: None,
    };

    join3(
        usb.run(),
        vendor_reader(vendor_reader_half),
        engine_loop(transport),
    )
    .await;
}
