//! Text and chord input on top of the v3 key protocol.
//!
//! Both are user-facing conveniences: typing a string, and pressing several keys
//! so they land in the same USB frame (a chord that is two frames apart is a
//! different input to most games).

use input_protocol::KeyId;
use serde::{Deserialize, Serialize};

/// One typist pass: what to type and how human the timing should be.
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

/// Several keys pressed inside one HID frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ChordConfig {
    /// Keyboard usages (page 7); modifiers are 0xE0..=0xE7.
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

/// One physical key press, including whether shift has to be held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyStroke {
    pub key: KeyId,
    pub shift: bool,
}

pub const LEFT_SHIFT: KeyId = KeyId {
    usage_page: 7,
    usage: 0xE1,
};

fn usage(c: char) -> Option<(u16, bool)> {
    // (usage, needs shift) for US layout; anything else is dropped.
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

/// Converts text into keystrokes. Characters the keyboard cannot express are
/// skipped, so the count of strokes is not necessarily the count of chars.
pub fn strokes(text: &str) -> Vec<KeyStroke> {
    let mut out = Vec::new();
    for c in text.chars() {
        match c {
            // Normalise Windows line endings into a single Enter.
            '\r' => {}
            _ => {
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
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_letters_digits_and_shifted_symbols() {
        let a = strokes("a")[0];
        assert_eq!(a.key.usage, 0x04);
        assert!(!a.shift);

        let upper = strokes("A")[0];
        assert_eq!(upper.key.usage, 0x04);
        assert!(upper.shift);

        let bang = strokes("!")[0];
        assert_eq!(bang.key.usage, 0x1E);
        assert!(bang.shift);

        let zero = strokes("0")[0];
        assert_eq!(zero.key.usage, 0x27);
        assert!(!zero.shift);
    }

    #[test]
    fn skips_newlines_crlf_and_unknown_characters() {
        assert_eq!(strokes("a\r\nb").len(), 3);
        let enter = strokes("\n")[0];
        assert_eq!(enter.key.usage, 0x28);
        assert!(strokes("中").is_empty());
    }
}
