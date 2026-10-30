//! Core-0 LED decision. Publishing is the platform's job (an atomic on the
//! device), so this type only holds the timers and the last decided state.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum LedState {
    #[default]
    Off = 0,
    Idle,
    Held,
    Error,
    Failsafe,
    Bootloader,
}

pub const ERROR_FLASH_MS: u32 = 140;
pub const FAILSAFE_HOLD_MS: u32 = 3000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatusIndicator {
    published: LedState,
    error_at: u32,
    failsafe_at: u32,
    error_active: bool,
    failsafe_active: bool,
    boot_signalled: bool,
}

impl StatusIndicator {
    pub fn signal_error(&mut self, now: u32) {
        self.error_at = now;
        self.error_active = true;
    }

    pub fn signal_failsafe(&mut self, now: u32) {
        self.failsafe_at = now;
        self.failsafe_active = true;
    }

    pub fn signal_bootloader(&mut self) {
        self.boot_signalled = true;
    }

    pub fn update(&mut self, now: u32, mounted: bool, has_pressed_keys: bool) {
        let state = if self.boot_signalled {
            LedState::Bootloader
        } else if !mounted {
            LedState::Off
        } else if self.error_active && now.wrapping_sub(self.error_at) < ERROR_FLASH_MS {
            LedState::Error
        } else if self.failsafe_active && now.wrapping_sub(self.failsafe_at) < FAILSAFE_HOLD_MS {
            LedState::Failsafe
        } else if has_pressed_keys {
            LedState::Held
        } else {
            LedState::Idle
        };

        if self.error_active && now.wrapping_sub(self.error_at) >= ERROR_FLASH_MS {
            self.error_active = false;
        }
        if self.failsafe_active && now.wrapping_sub(self.failsafe_at) >= FAILSAFE_HOLD_MS {
            self.failsafe_active = false;
        }
        self.published = state;
    }

    pub fn state(&self) -> LedState {
        self.published
    }
}
