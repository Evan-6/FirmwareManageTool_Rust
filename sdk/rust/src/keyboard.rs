//! Keyboard control: single keys, presses, chords, sequences and text.

use crate::device::Device;
use crate::error::{Error, Result};
use gdb_motion::{Rng, SplitMix64};
use input_protocol::{KeyId, v3::Command};
use serde::{Deserialize, Serialize};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TypeTextConfig {
    pub text: String,
    pub interval_ms: u32,
    pub hold_ms: u32,
    /// Random extra delay added to every interval (0 = metronome).
    pub jitter_ms: u32,
}

impl Default for TypeTextConfig {
    fn default() -> Self {
        Self {
            text: String::new(),
            interval_ms: 45,
            hold_ms: 18,
            jitter_ms: 30,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ChordConfig {
    /// Keyboard usages on page 7; modifiers are 0xE0..=0xE7.
    pub usages: Vec<u16>,
    pub hold_ms: u32,
}

impl Default for ChordConfig {
    fn default() -> Self {
        Self {
            usages: vec![0xE0, 0xE1, 0x06],
            hold_ms: 60,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyStroke {
    pub key: KeyId,
    pub shift: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyStep {
    pub page: u16,
    pub usage: u16,
    pub hold_ms: u32,
    pub gap_ms: u32,
}

pub const LEFT_SHIFT: KeyId = KeyId {
    usage_page: 7,
    usage: 0xE1,
};

fn usage(c: char) -> Option<(u16, bool)> {
    let mapped = match c {
        'a'..='z' => (0x04 + (c as u16 - 'a' as u16), false),
        'A'..='Z' => (0x04 + (c as u16 - 'A' as u16), true),
        '1'..='9' => (0x1E + (c as u16 - '1' as u16), false),
        '0' => (0x27, false),
        '-' => (0x2D, false),
        '=' => (0x2E, false),
        '[' => (0x2F, false),
        ']' => (0x30, false),
        '\\' => (0x31, false),
        ';' => (0x33, false),
        '\'' => (0x34, false),
        '`' => (0x35, false),
        ',' => (0x36, false),
        '.' => (0x37, false),
        '/' => (0x38, false),
        '!' => (0x1E, true),
        '@' => (0x1F, true),
        '#' => (0x20, true),
        '$' => (0x21, true),
        '%' => (0x22, true),
        '^' => (0x23, true),
        '&' => (0x24, true),
        '*' => (0x25, true),
        '(' => (0x26, true),
        ')' => (0x27, true),
        '_' => (0x2D, true),
        '+' => (0x2E, true),
        '{' => (0x2F, true),
        '}' => (0x30, true),
        '|' => (0x31, true),
        ':' => (0x33, true),
        '"' => (0x34, true),
        '~' => (0x35, true),
        '<' => (0x36, true),
        '>' => (0x37, true),
        '?' => (0x38, true),
        '\n' => (0x28, false),
        '\t' => (0x2B, false),
        ' ' => (0x2C, false),
        _ => return None,
    };
    Some(mapped)
}

/// Converts text into keystrokes on a US layout. Characters the firmware has no
/// usage for are dropped, so the stroke count is not the character count.
pub fn strokes(text: &str) -> Vec<KeyStroke> {
    let mut out = Vec::new();
    for c in text.chars() {
        if c == '\r' {
            continue;
        }
        if let Some((usage, shift)) = usage(c) {
            out.push(KeyStroke {
                key: KeyId {
                    usage_page: 7,
                    usage,
                },
                shift,
            });
        }
    }
    out
}

pub struct Keyboard {
    device: Device,
}

impl Device {
    pub fn keyboard(&self) -> Keyboard {
        Keyboard {
            device: self.clone(),
        }
    }
}

impl Keyboard {
    pub fn key_down(&self, page: u16, usage: u16) -> Result<()> {
        self.emit(page, usage, true)
    }

    pub fn key_up(&self, page: u16, usage: u16) -> Result<()> {
        self.emit(page, usage, false)
    }

    pub fn press(&self, page: u16, usage: u16) -> Result<()> {
        self.long_press(page, usage, 18)
    }

    pub fn long_press(&self, page: u16, usage: u16, hold_ms: u32) -> Result<()> {
        self.key_down(page, usage)?;
        thread::sleep(Duration::from_millis(u64::from(hold_ms.max(1))));
        self.key_up(page, usage)
    }

    /// Presses every usage, holds, then releases in reverse order.
    pub fn chord(&self, usages: &[u16], hold_ms: u32) -> Result<()> {
        if usages.is_empty() {
            return Err(Error::invalid("a chord needs at least one usage"));
        }
        for usage in usages {
            self.key_down(7, *usage)?;
        }
        thread::sleep(Duration::from_millis(u64::from(hold_ms.max(1))));
        for usage in usages.iter().rev() {
            self.key_up(7, *usage)?;
        }
        Ok(())
    }

    pub fn sequence(&self, steps: &[KeyStep]) -> Result<()> {
        for step in steps {
            self.long_press(step.page, step.usage, step.hold_ms)?;
            if step.gap_ms > 0 {
                thread::sleep(Duration::from_millis(u64::from(step.gap_ms)));
            }
        }
        Ok(())
    }

    pub fn type_text(&self, config: &TypeTextConfig) -> Result<()> {
        let mut rng = SplitMix64::from_entropy();
        for stroke in strokes(&config.text) {
            if stroke.shift {
                self.key_down(LEFT_SHIFT.usage_page, LEFT_SHIFT.usage)?;
            }
            self.key_down(stroke.key.usage_page, stroke.key.usage)?;
            thread::sleep(Duration::from_millis(u64::from(config.hold_ms.max(1))));
            self.key_up(stroke.key.usage_page, stroke.key.usage)?;
            if stroke.shift {
                self.key_up(LEFT_SHIFT.usage_page, LEFT_SHIFT.usage)?;
            }
            let jitter = if config.jitter_ms == 0 {
                0
            } else {
                (rng.unit() * f64::from(config.jitter_ms)) as u32
            };
            thread::sleep(Duration::from_millis(u64::from(
                config.interval_ms + jitter,
            )));
        }
        Ok(())
    }

    pub fn release_all(&self) -> Result<()> {
        self.device.release_all()
    }

    fn emit(&self, page: u16, usage: u16, down: bool) -> Result<()> {
        let key = KeyId {
            usage_page: page,
            usage,
        };
        if !key.valid() {
            return Err(Error::invalid(format!(
                "usage {page:#06x}:{usage:#06x} is not in the key catalog"
            )));
        }
        self.device.send_input(&Command::Key(key, down))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_letters_digits_and_shifted_symbols() {
        assert_eq!(strokes("a")[0].key.usage, 0x04);
        assert!(!strokes("a")[0].shift);
        assert!(strokes("A")[0].shift);
        assert!(strokes("!")[0].shift);
        assert_eq!(strokes("!")[0].key.usage, 0x1E);
        assert_eq!(strokes("0")[0].key.usage, 0x27);
    }

    #[test]
    fn skips_newlines_crlf_and_unknown_characters() {
        assert_eq!(strokes("a\r\nb").len(), 3);
        assert_eq!(strokes("\n")[0].key.usage, 0x28);
        assert!(strokes("中").is_empty());
    }
}
