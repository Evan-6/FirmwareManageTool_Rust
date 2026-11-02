//! Keyboard control: single keys, presses, chords, sequences and text.

use crate::device::Device;
use crate::error::{Error, Result};
use gdb_motion::{Rng, SplitMix64, lognormal};
use input_protocol::{KeyId, v3::Command};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TypeTextConfig {
    pub text: String,
    /// Median gap between keys. The actual gap is drawn log-normally around it,
    /// because human typing is right-skewed: mostly quick, occasionally a pause.
    pub interval_ms: u32,
    /// Median time the key stays down; sampled the same way as `interval_ms`.
    pub hold_ms: u32,
    /// Spread of the gap distribution. 0 types at a metronome pace, which no
    /// hand does; the value is read as a fraction of `interval_ms`.
    pub jitter_ms: u32,
    /// Chance that a key lands on its neighbour and is corrected with a
    /// backspace, 0..=1. Real typing is not perfect.
    pub typo_rate: f64,
}

impl Default for TypeTextConfig {
    fn default() -> Self {
        Self {
            text: String::new(),
            interval_ms: 45,
            hold_ms: 18,
            jitter_ms: 30,
            typo_rate: 0.02,
        }
    }
}

impl TypeTextConfig {
    /// The three-knob form the C ABI exposes. A caller that asked for zero
    /// jitter asked for machine timing, so it gets no slips either; the policy
    /// lives here so every binding agrees.
    pub fn paced(text: String, interval_ms: u32, hold_ms: u32, jitter_ms: u32) -> Self {
        Self {
            text,
            interval_ms,
            hold_ms,
            jitter_ms,
            typo_rate: if jitter_ms == 0 { 0.0 } else { 0.02 },
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

/// Backspace usage on page 7, used to correct a slip.
const BACKSPACE: u16 = 0x2A;

/// A neighbouring key on the same row, or the same key when there is none.
fn slip(usage: u16, rng: &mut SplitMix64) -> u16 {
    if !(0x04..=0x27).contains(&usage) {
        return usage;
    }
    let step = if rng.unit() < 0.5 { -1 } else { 1 };
    let candidate = i32::from(usage) + step;
    if (0x04..=0x27).contains(&candidate) {
        candidate as u16
    } else {
        usage
    }
}

/// Median gap between keys, drawn log-normally around `interval_ms`; the spread
/// comes from `jitter_ms` as a fraction of the interval.
fn gap_ms(config: &TypeTextConfig, rng: &mut SplitMix64) -> f64 {
    let base = f64::from(config.interval_ms.max(1));
    let spread = (f64::from(config.jitter_ms) / base).clamp(0.0, 0.6);
    if spread > 0.0 {
        lognormal(rng, base, spread).clamp(base * 0.35, base * 4.0)
    } else {
        base
    }
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
        crate::timing::sleep_ms(f64::from(hold_ms.max(1)));
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
        crate::timing::sleep_ms(f64::from(hold_ms.max(1)));
        for usage in usages.iter().rev() {
            self.key_up(7, *usage)?;
        }
        Ok(())
    }

    pub fn sequence(&self, steps: &[KeyStep]) -> Result<()> {
        for step in steps {
            self.long_press(step.page, step.usage, step.hold_ms)?;
            if step.gap_ms > 0 {
                crate::timing::sleep_ms(f64::from(step.gap_ms));
            }
        }
        Ok(())
    }

    /// Types `text` with human timing.
    ///
    /// Gaps and holds are drawn log-normally around the configured medians,
    /// whitespace and punctuation get an extra pause, and roughly one key in
    /// `typo_rate` lands on its neighbour and is corrected with a backspace. A
    /// metronome at exactly `interval_ms` is the easiest thing to detect.
    pub fn type_text(&self, config: &TypeTextConfig) -> Result<()> {
        let mut rng = SplitMix64::from_entropy();
        for stroke in strokes(&config.text) {
            if config.typo_rate > 0.0 && rng.unit() < config.typo_rate.clamp(0.0, 1.0) {
                let wrong = slip(stroke.key.usage, &mut rng);
                if wrong != stroke.key.usage {
                    self.tap(wrong, stroke.shift, config, &mut rng)?;
                    crate::timing::sleep_ms(lognormal(&mut rng, 130.0, 0.35).clamp(50.0, 520.0));
                    self.tap(BACKSPACE, false, config, &mut rng)?;
                }
            }
            self.tap(stroke.key.usage, stroke.shift, config, &mut rng)?;
            let mut gap = gap_ms(config, &mut rng);
            if matches!(
                stroke.key.usage,
                0x2C | 0x28 | 0x2B | 0x33 | 0x36 | 0x37 | 0x38
            ) {
                gap += lognormal(&mut rng, 180.0, 0.4).clamp(60.0, 600.0);
            }
            crate::timing::sleep_ms(gap);
        }
        Ok(())
    }

    /// One key down, a sampled hold, key up. Shift is handled per stroke so the
    /// hold covers the shifted character rather than the modifier alone.
    fn tap(
        &self,
        usage: u16,
        shift: bool,
        config: &TypeTextConfig,
        rng: &mut SplitMix64,
    ) -> Result<()> {
        if shift {
            self.key_down(LEFT_SHIFT.usage_page, LEFT_SHIFT.usage)?;
        }
        self.key_down(7, usage)?;
        let hold = lognormal(rng, f64::from(config.hold_ms.max(8)), 0.22).clamp(25.0, 130.0);
        crate::timing::sleep_ms(hold);
        self.key_up(7, usage)?;
        if shift {
            self.key_up(LEFT_SHIFT.usage_page, LEFT_SHIFT.usage)?;
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
