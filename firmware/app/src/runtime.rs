//! One main-loop pass, in the same stage order as the C++ `FirmwareRuntime::loop`:
//!
//! 1. reset / RX fault flags
//! 2. consume up to four received packets
//! 3. advance the output scheduler and submit at most one input report
//! 4. vendor completion, then control replies
//! 5. send the head vendor reply when the endpoint is ready
//! 6. LED decision, then the bootloader gate
//!
//! The transport owns USB; this type owns the engine, the LED timers and the boot
//! gate, so every hardware-free rule stays testable on the host.

use firmware_engine::{
    BOOT_CONFIRMED, BootloaderGate, ERROR, FAILSAFE, InputEngine, InputReport, Report,
    StatusIndicator,
};

/// Fault code plus the callback error count that accompanied it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RxFault {
    /// 1 = framing, 8 = RX queue overflow, 0 = none.
    pub code: u8,
    pub errors: u16,
}

/// Everything the runtime needs from the USB peripheral. Implemented by the
/// embassy transport on the device and by a mock in `tests/runtime.rs`.
pub trait Transport {
    fn mounted(&mut self) -> bool;
    fn input_ready(&mut self) -> bool;
    fn vendor_ready(&mut self) -> bool;
    /// Submit a report. `true` means accepted by the stack, not yet completed.
    fn send_input(&mut self, report: &InputReport) -> bool;
    fn send_vendor(&mut self, report: &Report, boot_generation: u32) -> bool;
    fn read(&mut self, report: &mut Report) -> bool;
    fn discard_rx(&mut self);
    /// Report id of the input report completed since the last call, or 0.
    fn take_input_complete(&mut self) -> u8;
    fn take_input_failure(&mut self) -> bool;
    fn take_vendor_complete(&mut self) -> Option<u32>;
    /// Optional hook for commands the platform answers itself from flash
    /// (persisted settings). Returning a report queues it as the reply.
    fn answer_platform(&mut self, _report: &Report) -> Option<Report> {
        None
    }
    fn take_reset(&mut self) -> bool;
    fn take_rx_fault(&mut self) -> RxFault;
}

pub struct Runtime<T: Transport> {
    engine: InputEngine,
    transport: T,
    led: StatusIndicator,
    boot: BootloaderGate,
    rx_generation: u32,
    boot_requested: bool,
}

impl<T: Transport> Runtime<T> {
    pub fn new(transport: T) -> Self {
        Self {
            engine: InputEngine::new(),
            transport,
            led: StatusIndicator::default(),
            boot: BootloaderGate::default(),
            rx_generation: 0,
            boot_requested: false,
        }
    }

    /// One pass. `now_ms` comes from the platform clock.
    pub fn step(&mut self, now: u32) {
        if self.transport.take_reset() {
            self.engine.receive_fault(7, 0, now);
        }
        let fault = self.transport.take_rx_fault();
        self.engine.receive_fault(fault.code, fault.errors, now);
        self.discard_faulted_rx(now);

        let mut packet = Report::default();
        for _ in 0..4 {
            if !self.transport.read(&mut packet) {
                break;
            }
            self.engine.handle(&packet, now);
            // The engine validated session and sequence; only then does the
            // platform get to answer from flash.
            if let Some(reply) = self.transport.answer_platform(&packet) {
                let id = firmware_engine::wire::read32(&packet.bytes[4..]);
                let seq = firmware_engine::wire::read32(&packet.bytes[8..]);
                // Only answer when the engine actually accepted this command;
                // otherwise a stale or replayed report could rewrite the sector.
                let accepted = self
                    .engine
                    .session(id)
                    .map(|session| session.received == seq)
                    .unwrap_or(false);
                if accepted {
                    self.engine.push_reply(reply);
                }
            }
            self.discard_faulted_rx(now);
        }

        let mounted = self.transport.mounted();
        let completed = self.transport.take_input_complete();
        let failed = self.transport.take_input_failure();
        let ready = self.transport.input_ready();
        let mut input = InputReport::default();
        if self
            .engine
            .advance(now, mounted, completed, failed, ready, &mut input)
            && !self.transport.send_input(&input)
        {
            self.engine.input_rejected(now);
        }
        self.discard_faulted_rx(now);

        if let Some(generation) = self.transport.take_vendor_complete() {
            self.engine.vendor_completed(generation);
        }
        self.engine.service_control(now);
        self.discard_faulted_rx(now);

        let mounted = self.transport.mounted();
        if mounted
            && self.transport.vendor_ready()
            && let Some(reply) = self.engine.vendor_reply().copied()
            && self
                .transport
                .send_vendor(&reply, self.engine.boot_generation())
        {
            self.engine.vendor_submitted();
        }

        let notices = self.engine.take_notices();
        if notices & ERROR != 0 {
            self.led.signal_error(now);
        }
        if notices & FAILSAFE != 0 {
            self.led.signal_failsafe(now);
        }
        if notices & BOOT_CONFIRMED != 0 {
            self.led.signal_bootloader();
        }
        let held = self.engine.desired_state().held();
        self.led.update(now, mounted, held);

        if self.boot.advance(
            now,
            self.engine.boot_confirmed(),
            self.engine.idle(),
            self.transport.vendor_ready(),
            self.engine.tx_empty(),
        ) {
            self.boot_requested = true;
        }
    }

    /// A discarded RX queue must not replay stale packets after a fault.
    fn discard_faulted_rx(&mut self, now: u32) {
        if self.rx_generation != self.engine.rx_generation() {
            self.rx_generation = self.engine.rx_generation();
            self.transport.discard_rx();
            let errors = self.transport.take_rx_fault();
            self.engine.receive_fault(0, errors.errors, now);
        }
    }

    /// True once after the boot gate opens; the caller resets into BOOTSEL.
    pub fn take_boot_request(&mut self) -> bool {
        let requested = self.boot_requested;
        self.boot_requested = false;
        requested
    }

    pub fn engine(&self) -> &InputEngine {
        &self.engine
    }

    /// Hands the engine the facts only the platform knows (queue depth, reset
    /// cause, lock LEDs). Called once per loop; a few bytes, no allocation.
    pub fn set_platform(&mut self, info: firmware_engine::PlatformInfo) {
        self.engine.set_platform(info);
    }

    /// Records how late this tick ran, in microseconds; STATUS_EXT reports the
    /// high-water mark and clears it.
    /// Build identity for STATUS_EXT; Feature 14 carries the same value.
    pub fn set_build(&mut self, build: u32) {
        self.engine.set_build(build);
    }

    pub fn note_late_tick(&mut self, late_us: u16) {
        self.engine.note_late_tick(late_us);
    }

    pub fn led_state(&self) -> firmware_engine::LedState {
        self.led.state()
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }
}
