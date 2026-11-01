//! Text and chord input.
//!
//! The mapping from characters to HID usages and the typist timing live in the
//! SDK (`gdb::keyboard`), so this module is the tool's name for those types: the
//! same serde shape the React UI and the stored macros already use.
//!
//! Chords are the one place the tool keeps its own execution: it presses every
//! key inside a single `SNAPSHOT` frame, whereas the SDK's `Keyboard::chord`
//! sends one frame per key. Two frames apart is a different input to most games.

pub use gdb::keyboard::{ChordConfig, KeyStroke, LEFT_SHIFT, TypeTextConfig, strokes};
