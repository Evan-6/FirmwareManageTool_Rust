//! Bootloader entry gate: a confirmed BOOTLOADER reply must age 120 ms and the
//! bus must be quiet (output idle, vendor ready, TX empty) before rebooting.

use crate::config::BOOTLOADER_DELAY_MS;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BootloaderGate {
    pending: bool,
    at: u32,
}

impl BootloaderGate {
    pub fn advance(
        &mut self,
        now: u32,
        confirmed: bool,
        output_idle: bool,
        vendor_ready: bool,
        tx_empty: bool,
    ) -> bool {
        if !confirmed {
            self.pending = false;
            return false;
        }
        if !self.pending {
            self.pending = true;
            self.at = now;
        }
        now.wrapping_sub(self.at) >= BOOTLOADER_DELAY_MS && output_idle && vendor_ready && tx_empty
    }

    pub fn pending(&self) -> bool {
        self.pending
    }
}
