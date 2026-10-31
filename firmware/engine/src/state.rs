//! 31-byte input snapshot plus the physical-key rules shared by every command
//! that can change held state.

use crate::config::SESSIONS;
use crate::keys_generated::{CONSUMER_USAGES, KEYBOARD_BITMAP};

pub const CONSUMER_BITS: u8 = 0x7f;
pub const BUTTON_BITS: u8 = 0x1f;
pub const KEYBOARD_END: u16 = 224;

/// Bit set means the firmware accepts that physical Keyboard usage.
pub fn keyboard_supported(usage: u16) -> bool {
    usage < KEYBOARD_END && KEYBOARD_BITMAP[usage as usize / 8] & (1 << (usage % 8)) != 0
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputState {
    pub modifiers: u8,
    pub keys: [u8; 28],
    pub consumer: u8,
    pub buttons: u8,
}

const _: () = assert!(
    core::mem::size_of::<InputState>() == 31,
    "input snapshot must keep its 31-byte wire layout"
);

impl InputState {
    /// Applies one physical key. Returns false when the usage is not accepted.
    pub fn set(&mut self, page: u16, usage: u16, down: bool) -> bool {
        let (byte, bit) = if page == 7 && keyboard_supported(usage) {
            // Modifiers live in their own byte; every other usage indexes the
            // 28-byte bitmap, which is why 0xE0..=0xE7 must be matched first.
            if (0xe0..=0xe7).contains(&usage) {
                (&mut self.modifiers, 1u8 << (usage - 0xe0))
            } else {
                (&mut self.keys[usage as usize / 8], 1u8 << (usage % 8))
            }
        } else if page == 12 {
            match CONSUMER_USAGES.iter().position(|&u| u == usage) {
                Some(index) => (&mut self.consumer, 1u8 << index),
                None => return false,
            }
        } else {
            return false;
        };
        if down {
            *byte |= bit;
        } else {
            *byte &= !bit;
        }
        true
    }

    /// Rejects bits that the host may not set directly.
    pub fn valid(&self) -> bool {
        if self.consumer & !CONSUMER_BITS != 0 || self.buttons & !BUTTON_BITS != 0 {
            return false;
        }
        let mut usage = 0u16;
        while usage < KEYBOARD_END {
            if self.keys[usage as usize / 8] & (1 << (usage % 8)) != 0 && !keyboard_supported(usage)
            {
                return false;
            }
            usage += 1;
        }
        true
    }

    /// Number of pressed keys (modifiers included), consumer and mouse excluded.
    /// Used by SESSION_LIST so the host can see who is holding what.
    pub fn count_keys(&self) -> u8 {
        let mut count = self.modifiers.count_ones() as u8;
        for byte in self.keys {
            count = count.saturating_add(byte.count_ones() as u8);
        }
        count
    }

    pub fn held(&self) -> bool {
        self.modifiers != 0
            || self.consumer != 0
            || self.buttons != 0
            || self.keys.iter().any(|&b| b != 0)
    }
}

pub type Sources = [InputState; SESSIONS];

/// Union of every non-zero session state: two clients holding the same key must
/// both release before the device does.
pub fn merge_sources(states: &Sources) -> InputState {
    let mut merged = InputState::default();
    for state in states {
        merged.modifiers |= state.modifiers;
        merged.consumer |= state.consumer;
        merged.buttons |= state.buttons;
        for (into, from) in merged.keys.iter_mut().zip(state.keys.iter()) {
            *into |= *from;
        }
    }
    merged
}
