//! Record an input session and play it back: macros, regression repro, demos.
//!
//! A recording stores *intent*, not raw reports. Each entry says what the caller
//! asked for — "key 7:0x04 down", "move by 40,-12", "one barrier" — and playback
//! rebuilds the frames through [`Device`] at the moment it runs. So a recording
//! survives a firmware change that keeps the same opcodes: a new report layout, a
//! different sequence base or a rearranged feature report cannot make it stale,
//! because no bytes are stored. The flip side is that a recording cannot promise
//! what the device *did* — use the packet trace (`gdb::log`) for that.
//!
//! Playback is deterministic: entries are applied in order with no random source
//! of its own, so the same file against the same device state produces the same
//! frames every time. `seed` is carried along so the gesture can be regenerated
//! (the movement maths in `gdb_motion` is seeded SplitMix64), and `at_ms` is the
//! wall-clock offset the entry was captured at.
//!
//! # File format
//!
//! JSON, one object, written with [`Recording::save`]:
//!
//! ```json
//! {
//!   "version": 1,
//!   "seed": 16909060,
//!   "screen": [1920, 1080],
//!   "started_at_ms": 1767225600000,
//!   "entries": [
//!     { "at_ms": 0, "op": "move_raw", "dx": 40, "dy": -12 },
//!     { "at_ms": 12, "op": "key", "page": 7, "usage": 4, "down": true },
//!     { "at_ms": 96, "op": "key", "page": 7, "usage": 4, "down": false },
//!     { "at_ms": 96, "op": "wheel", "dx": -40, "dy": 120 },
//!     { "at_ms": 97, "op": "barrier" }
//!   ]
//! }
//! ```
//!
//! Keys are snake_case and appear in this order (`at_ms` first, then the op
//! tag). `screen` is `[width, height]`, `started_at_ms` is Unix milliseconds,
//! and `at_ms` is milliseconds since that start. `op` is one of `key`,
//! `buttons`, `wheel`, `move_by`, `move_raw`, `barrier`, `release_all`, `wait`.
//! Those rules are the whole contract: any other language can read a file with
//! nothing but a JSON parser. `version` is the format version, and a file with a
//! different one is refused instead of being played halfway.
//!
//! # Recording
//!
//! [`Recorder`] is a tee the caller writes into next to each call it already
//! makes — there is no hook inside [`Device`], so nothing here can drift from
//! what the caller actually did:
//!
//! ```
//! use gdb::replay::{NoCancel, Op, Recorder};
//! use gdb::{DeviceOptions, Manager};
//!
//! let (device, _handle) = Manager::open_mock_with_handle(DeviceOptions::default()).unwrap();
//! let mut recorder = Recorder::new(4, (1920, 1080));
//! device.keyboard().key_down(7, 0x04).unwrap();
//! recorder.record(Op::Key { page: 7, usage: 0x04, down: true });
//! device.keyboard().key_up(7, 0x04).unwrap();
//! recorder.record(Op::Key { page: 7, usage: 0x04, down: false });
//!
//! let mut replay_device = device.clone();
//! let outcome = recorder.play(&mut replay_device, 1.0, &NoCancel).expect("playback");
//! assert_eq!(outcome.played, 2);
//! ```

use crate::device::Device;
use crate::error::{Error, Result};
use crate::flash::Cancel;
use crate::motion::MotionOptions;
use input_protocol::KeyId;
use input_protocol::v3::Command;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Format version written by this build, and the only one it will replay.
pub const VERSION: u32 = 1;

/// One thing the caller asked the device to do.
///
/// Deltas are relative for the move ops (`dx`/`dy` in pixels), `Wheel` carries
/// the horizontal axis in `dx` and the vertical one in `dy` (the wire order is
/// vertical first), and `Buttons` is the whole bit mask, not a delta — so a
/// recording never depends on what was held down before it started.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// HID key on `page` with a catalog `usage`.
    Key { page: u16, usage: u16, down: bool },
    /// Button mask: left=bit0, right=bit1, middle=bit2, back=bit3, forward=bit4.
    Buttons { bits: u8 },
    /// One wheel group; `dx` is horizontal, `dy` is vertical.
    Wheel { dx: i16, dy: i16 },
    /// Interpolated relative move with a barrier at the end.
    MoveBy { dx: i32, dy: i32 },
    /// One relative move with no barrier and no interpolation.
    MoveRaw { dx: i32, dy: i32 },
    /// Flush: completion guarantee for everything sent before it.
    Barrier,
    /// Let go of every key and button this session holds.
    ReleaseAll,
    /// A recorded pause. Playback already waits out the `at_ms` gap, so this
    /// entry exists to say "nothing happened for a while" in the file itself.
    Wait,
}

/// One entry: when it happened, and what was asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Milliseconds since the recording started.
    pub at_ms: u64,
    #[serde(flatten)]
    pub op: Op,
}

impl Entry {
    pub fn new(at_ms: u64, op: Op) -> Self {
        Self { at_ms, op }
    }
}

/// A recorded session. Opaque on purpose: `version` is always [`VERSION`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recording {
    version: u32,
    seed: u64,
    screen: (i32, i32),
    started_at_ms: u64,
    entries: Vec<Entry>,
}

impl Recording {
    /// An empty recording stamped now.
    pub fn new(seed: u64, screen: (i32, i32)) -> Self {
        Self {
            version: VERSION,
            seed,
            screen,
            started_at_ms: now_ms(),
            entries: Vec::new(),
        }
    }

    pub fn version(&self) -> u32 {
        self.version
    }

    /// The PRNG seed the gesture was captured with.
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Screen the session was recorded on, as `(width, height)`.
    pub fn screen(&self) -> (i32, i32) {
        self.screen
    }

    /// Unix milliseconds when recording started.
    pub fn started_at_ms(&self) -> u64 {
        self.started_at_ms
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Appends one entry. `at_ms` is milliseconds since recording started;
    /// entries keep insertion order and playback treats an out-of-order `at_ms`
    /// as "no wait" instead of reordering the recording.
    pub fn push(&mut self, at_ms: u64, op: Op) -> u64 {
        self.entries.push(Entry::new(at_ms, op));
        at_ms
    }

    /// Writes the JSON document. Two saves of the same recording are byte for
    /// byte identical: no maps, and no timestamp is added at save time.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let mut text = serde_json::to_string_pretty(self)
            .map_err(|error| Error::invalid(format!("cannot encode recording: {error}")))?;
        text.push('\n');
        std::fs::write(path, text)
            .map_err(|error| Error::io(format!("cannot write {}: {error}", path.display())))
    }

    /// Reads a recording written by this format version.
    ///
    /// A different `version` — newer or older — is refused with
    /// [`ErrorCode::Invalid`](crate::ErrorCode::Invalid) before any entry is
    /// looked at, and a file that is not a recording at all is refused the same
    /// way: a half-understood macro must never reach the device.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .map_err(|error| Error::io(format!("cannot read {}: {error}", path.display())))?;
        let value: serde_json::Value = serde_json::from_str(&text)
            .map_err(|error| Error::invalid(format!("{} is not JSON: {error}", path.display())))?;
        match value.get("version").and_then(serde_json::Value::as_u64) {
            Some(version) if version == u64::from(VERSION) => {}
            Some(version) => {
                return Err(Error::invalid(format!(
                    "recording {} is version {version}; this build replays version {VERSION}",
                    path.display()
                )));
            }
            None => {
                return Err(Error::invalid(format!(
                    "recording {} has no version field, so it is not a GDBOX recording",
                    path.display()
                )));
            }
        }
        serde_json::from_value(value).map_err(|error| {
            Error::invalid(format!("{} is not a recording: {error}", path.display()))
        })
    }

    /// Replays every entry onto `device`.
    ///
    /// Inter-entry gaps come from `at_ms` and are divided by `speed` (2.0 runs
    /// twice as fast). Cancellation is checked between entries and inside every
    /// wait; a cancelled playback stops, releases whatever it held down and
    /// returns an [`Outcome`] with the rest counted as `skipped` rather than an
    /// error — a stopped macro is a normal ending. A device failure is an error,
    /// because the rest of the recording no longer means anything.
    pub fn play(&self, device: &mut Device, speed: f64, cancel: &dyn Cancel) -> Result<Outcome> {
        if !speed.is_finite() || !(0.001..=1000.0).contains(&speed) {
            return Err(Error::invalid(
                "playback speed must be a number between 0.001 and 1000",
            ));
        }
        let total = self.entries.len();
        let start = Instant::now();
        let mut played = 0usize;
        let mut last_at = 0u64;
        let mut due_ms = 0f64;
        for entry in &self.entries {
            let gap_ms = entry.at_ms.saturating_sub(last_at);
            last_at = last_at.max(entry.at_ms);
            due_ms += gap_ms as f64 / speed;
            let deadline = start + Duration::from_secs_f64(due_ms / 1000.0);
            if !wait_until(deadline, cancel) {
                let elapsed_ms = start.elapsed().as_millis() as u64;
                if played > 0 {
                    // A stopped macro must not leave a key or a button stuck.
                    let _ = device.release_all();
                }
                return Ok(Outcome {
                    played,
                    skipped: total - played,
                    elapsed_ms,
                });
            }
            apply(device, entry.op)?;
            played += 1;
        }
        Ok(Outcome {
            played,
            skipped: 0,
            elapsed_ms: start.elapsed().as_millis() as u64,
        })
    }
}

/// What a recording says about itself, in one object a host can log or display.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Info {
    pub version: u32,
    pub seed: u64,
    /// The screen the session was recorded on, as `[width, height]`.
    pub screen: (i32, i32),
    pub started_at_ms: u64,
    pub entries: usize,
}

impl Recording {
    /// The header of a recording: format version, seed, screen, entry count.
    pub fn info(&self) -> Info {
        Info {
            version: self.version,
            seed: self.seed,
            screen: self.screen,
            started_at_ms: self.started_at_ms,
            entries: self.entries.len(),
        }
    }
}

/// A recording read one entry at a time, for a host that draws or forwards the
/// entries instead of replaying the file itself.
///
/// Reading is not playing: an entry is data, and only [`Recording::play`] knows
/// how to send it with the recorded timing and how to release what a cancelled
/// playback still held. [`Reader::play`] is that call, for a host that wants both.
#[derive(Clone, Debug)]
pub struct Reader {
    recording: Recording,
    index: usize,
    closed: bool,
}

impl Reader {
    /// Loads a recording; the same version check [`Recording::load`] does.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            recording: Recording::load(path)?,
            index: 0,
            closed: false,
        })
    }

    pub fn info(&self) -> Info {
        self.recording.info()
    }

    pub fn recording(&self) -> &Recording {
        &self.recording
    }

    /// Back to the first entry, so a second pass reads the same file again.
    pub fn rewind(&mut self) {
        self.index = 0;
        self.closed = false;
    }

    /// Stops reading. The recording stays loaded, so `info` still answers; `next`
    /// returns `None` and `play` is refused.
    pub fn close(&mut self) {
        self.closed = true;
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// One entry as the file writes it: `{"at_ms":12,"op":"key","page":7,…}`.
    ///
    /// The shape is the recording's own, so a host that reads a file and a host
    /// that reads this call parse the same keys.
    pub fn entry_json(entry: &Entry) -> Result<String> {
        serde_json::to_string(entry)
            .map_err(|error| Error::invalid(format!("cannot encode entry: {error}")))
    }

    /// Plays the whole recording onto `device`, from the first entry.
    ///
    /// The same call [`Recording::play`] makes, including the release on
    /// cancellation; `closed` is refused with `Invalid` so a host that stopped
    /// reading does not also send input by accident.
    pub fn play(&self, device: &mut Device, speed: f64) -> Result<Outcome> {
        if self.closed {
            return Err(Error::invalid(
                "the replay reader is closed; rewind it before playing",
            ));
        }
        self.recording.play(device, speed, &NoCancel)
    }
}

/// The entries in order, so `for entry in &mut reader` reads a recording and a
/// closed reader simply ends. An entry is `Copy`: it is a timestamp and an op.
impl Iterator for Reader {
    type Item = Entry;

    fn next(&mut self) -> Option<Entry> {
        if self.closed {
            return None;
        }
        let entry = self.recording.entries.get(self.index).copied();
        if entry.is_some() {
            self.index += 1;
        }
        entry
    }
}

/// What a playback did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// Entries that reached the device, in order.
    pub played: usize,
    /// Entries left when playback stopped early, by cancellation.
    pub skipped: usize,
    /// Wall clock the playback took, gaps included.
    pub elapsed_ms: u64,
}

impl Outcome {
    pub fn is_complete(&self) -> bool {
        self.skipped == 0
    }
}

/// Builds a [`Recording`] from calls the caller already makes.
///
/// The clock starts at [`Recorder::new`], so `record` timestamps are offsets
/// from then; nothing here touches the device.
#[derive(Clone, Debug)]
pub struct Recorder {
    recording: Recording,
    started: Instant,
}

impl Recorder {
    pub fn new(seed: u64, screen: (i32, i32)) -> Self {
        Self {
            recording: Recording::new(seed, screen),
            started: Instant::now(),
        }
    }

    /// Stamps `op` with the time since this recorder was created.
    ///
    /// Call it next to the device call it describes, so the file says what the
    /// caller did rather than what the SDK guessed.
    pub fn record(&mut self, op: Op) -> u64 {
        let at_ms = self.started.elapsed().as_millis() as u64;
        self.record_at(at_ms, op)
    }

    /// Records an explicit offset instead of "now" — for building fixtures and
    /// for importing timing measured elsewhere.
    pub fn record_at(&mut self, at_ms: u64, op: Op) -> u64 {
        self.recording.push(at_ms, op)
    }

    pub fn recording(&self) -> &Recording {
        &self.recording
    }

    pub fn into_recording(self) -> Recording {
        self.recording
    }

    pub fn seed(&self) -> u64 {
        self.recording.seed()
    }

    pub fn screen(&self) -> (i32, i32) {
        self.recording.screen()
    }

    pub fn len(&self) -> usize {
        self.recording.len()
    }

    pub fn is_empty(&self) -> bool {
        self.recording.is_empty()
    }

    pub fn clear(&mut self) {
        self.recording.entries.clear();
        self.started = Instant::now();
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        self.recording.save(path)
    }

    /// Opens a saved recording for playing or for appending more ops. The
    /// timestamps already in the file are kept as they are.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            recording: Recording::load(path)?,
            started: Instant::now(),
        })
    }

    pub fn play(&self, device: &mut Device, speed: f64, cancel: &dyn Cancel) -> Result<Outcome> {
        self.recording.play(device, speed, cancel)
    }
}

/// A cancellation token that never fires: playback runs to the end.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoCancel;

impl Cancel for NoCancel {
    fn cancelled(&self) -> bool {
        false
    }
}

/// Applies one recorded op through the typed SDK surface, so the bookkeeping
/// (held buttons, mock cursor) stays right for whatever runs next.
fn apply(device: &mut Device, op: Op) -> Result<()> {
    match op {
        Op::Key { page, usage, down } => device.send_input(&Command::Key(
            KeyId {
                usage_page: page,
                usage,
            },
            down,
        )),
        Op::Buttons { bits } => {
            device.set_buttons(bits)?;
            device.send_input(&Command::MouseButtons(bits))
        }
        Op::Wheel { dx, dy } => device.send_input(&Command::Wheel(dy, dx)),
        Op::MoveBy { dx, dy } => device.mouse().move_by(dx, dy, MotionOptions::linear(0)),
        Op::MoveRaw { dx, dy } => device.mouse().move_raw(dx, dy),
        Op::Barrier => device.barrier(),
        Op::ReleaseAll => device.release_all(),
        Op::Wait => Ok(()),
    }
}

/// Waits for `deadline` in short slices so a cancel lands promptly; returns
/// `false` when the token fired instead of reaching the deadline.
fn wait_until(deadline: Instant, cancel: &dyn Cancel) -> bool {
    loop {
        if cancel.cancelled() {
            return false;
        }
        let now = Instant::now();
        if now >= deadline {
            return true;
        }
        thread::sleep((deadline - now).min(Duration::from_millis(2)));
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn op_tags_are_the_documented_names() {
        let cases = [
            (
                Op::Key {
                    page: 7,
                    usage: 4,
                    down: true,
                },
                "\"op\":\"key\"",
            ),
            (Op::Buttons { bits: 1 }, "\"op\":\"buttons\""),
            (Op::Wheel { dx: 1, dy: 2 }, "\"op\":\"wheel\""),
            (Op::MoveBy { dx: 1, dy: 2 }, "\"op\":\"move_by\""),
            (Op::MoveRaw { dx: 1, dy: 2 }, "\"op\":\"move_raw\""),
            (Op::Barrier, "\"op\":\"barrier\""),
            (Op::ReleaseAll, "\"op\":\"release_all\""),
            (Op::Wait, "\"op\":\"wait\""),
        ];
        for (op, tag) in cases {
            let entry = Entry::new(3, op);
            let text = serde_json::to_string(&entry).expect("encode");
            assert!(text.starts_with("{\"at_ms\":3,"), "{text}");
            assert!(text.contains(tag), "{text}");
            let back: Entry = serde_json::from_str(&text).expect("decode");
            assert_eq!(back, entry);
        }
    }

    #[test]
    fn wheel_keeps_horizontal_in_dx() {
        let op = Op::Wheel { dx: -40, dy: 120 };
        let text = serde_json::to_string(&op).expect("encode");
        assert!(text.contains("\"dx\":-40"), "{text}");
        assert!(text.contains("\"dy\":120"), "{text}");
    }

    #[test]
    fn playback_refuses_a_speed_that_is_not_a_positive_number() {
        let recording = Recording::new(1, (1920, 1080));
        let (mut device, _handle) =
            crate::Manager::open_mock_with_handle(Default::default()).unwrap();
        for speed in [0.0, -1.0, f64::NAN, f64::INFINITY, 10_000.0] {
            let error = recording
                .play(&mut device, speed, &NoCancel)
                .expect_err("speed must be refused");
            assert_eq!(error.code(), crate::ErrorCode::Invalid);
        }
    }

    /// A reader hands the entries out in order and in the file's own JSON, so a
    /// host in another language parses one shape whether it reads the file or
    /// this call.
    #[test]
    fn a_reader_hands_entries_out_one_at_a_time() {
        let root = std::env::temp_dir().join(format!(
            "gdb-replay-reader-{}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("recording.json");
        let mut recording = Recording::new(9, (1280, 720));
        recording.push(
            0,
            Op::Key {
                page: 7,
                usage: 4,
                down: true,
            },
        );
        recording.push(40, Op::MoveBy { dx: 12, dy: -6 });
        recording.push(80, Op::Barrier);
        recording.save(&path).unwrap();

        let mut reader = Reader::open(&path).expect("reader");
        let info = reader.info();
        assert_eq!(info.version, VERSION);
        assert_eq!(info.seed, 9);
        assert_eq!(info.screen, (1280, 720));
        assert_eq!(info.entries, 3);
        assert!(
            serde_json::to_string(&info)
                .unwrap()
                .contains("\"started_at_ms\"")
        );

        let mut lines = Vec::new();
        for entry in &mut reader {
            lines.push(Reader::entry_json(&entry).unwrap());
        }
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(lines[0].contains("\"op\":\"key\""), "{}", lines[0]);
        assert!(lines[1].contains("\"op\":\"move_by\""), "{}", lines[1]);
        assert!(reader.next().is_none(), "the end is None, not a repeat");

        // The same recording still plays, and the play call is the SDK's own.
        let (mut device, _handle) =
            crate::Manager::open_mock_with_handle(Default::default()).unwrap();
        let outcome = reader.play(&mut device, 1000.0).expect("playback");
        assert!(outcome.is_complete());
        assert_eq!(outcome.played, 3);

        // A closed reader stops reading and refuses to send input.
        reader.close();
        assert!(reader.is_closed());
        assert!(reader.next().is_none());
        assert_eq!(
            reader.play(&mut device, 1.0).unwrap_err().code(),
            crate::ErrorCode::Invalid
        );
        reader.rewind();
        assert!(!reader.is_closed());
        assert!(reader.next().is_some(), "a rewind reads the file again");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A file that is not this format is refused before any entry is looked at:
    /// a half-understood macro must never reach the device.
    #[test]
    fn a_reader_refuses_a_missing_or_foreign_file() {
        let root = std::env::temp_dir().join(format!(
            "gdb-replay-bad-{}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let missing = Reader::open(root.join("nothing.json")).expect_err("missing file");
        assert_eq!(missing.code(), crate::ErrorCode::Io);

        let path = root.join("other.json");
        std::fs::write(
            &path,
            "{\"version\":7,\"seed\":1,\"screen\":[1,1],\"started_at_ms\":0,\"entries\":[]}",
        )
        .unwrap();
        let wrong = Reader::open(&path).expect_err("version 7");
        assert_eq!(wrong.code(), crate::ErrorCode::Invalid);
        assert!(wrong.message.contains("version 7"), "{wrong}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
