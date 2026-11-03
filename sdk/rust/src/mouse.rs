//! Mouse control: buttons, wheel, clicks, drags and the two movement flavours.

use crate::device::Device;
use crate::error::{Error, Result};
use crate::motion::{MotionOptions, Movement, run_move_by, run_move_to};
use gdb_motion::{Rng, SplitMix64, lognormal};
use input_protocol::v3::Command;

/// HID button bits: left=bit0, right=bit1, middle=bit2, back=bit3, forward=bit4.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left = 1,
    Right = 2,
    Middle = 3,
    Back = 4,
    Forward = 5,
}

impl Button {
    pub fn from_u8(value: u8) -> Result<Self> {
        Ok(match value {
            1 => Self::Left,
            2 => Self::Right,
            3 => Self::Middle,
            4 => Self::Back,
            5 => Self::Forward,
            _ => return Err(Error::invalid("button must be 1..=5")),
        })
    }
    fn bit(self) -> u8 {
        1 << (self as u8 - 1)
    }
}

pub struct Mouse {
    device: Device,
}

impl Device {
    pub fn mouse(&self) -> Mouse {
        Mouse {
            device: self.clone(),
        }
    }
}

impl Mouse {
    pub fn move_by(&self, dx: i32, dy: i32, options: MotionOptions) -> Result<()> {
        let shared = crate::motion::MovementShared::default();
        run_move_by(&self.device, dx, dy, &options, &shared)
    }

    /// One relative move with no barrier and no interpolation: the deltas go out
    /// as they are, split only at the ±1024 HID limit. This is the hot path for
    /// remote control, where the caller batches frames and flushes with
    /// `Device::barrier` only at points that need a completion guarantee.
    pub fn move_raw(&self, dx: i32, dy: i32) -> Result<()> {
        for (x, y) in gdb_motion::split_delta(dx, dy) {
            self.device.send_mouse_move(x as i16, y as i16)?;
        }
        Ok(())
    }

    pub fn move_to(&self, x: i32, y: i32, options: MotionOptions) -> Result<()> {
        let shared = crate::motion::MovementShared::default();
        run_move_to(&self.device, x, y, &options, &shared)
    }

    /// Starts an absolute move on a worker thread so the caller can retarget or
    /// cancel it while it runs.
    pub fn start_move_to(&self, x: i32, y: i32, options: MotionOptions) -> Movement {
        Movement::spawn(self.device.clone(), x, y, options, true)
    }

    pub fn start_move_by(&self, dx: i32, dy: i32, options: MotionOptions) -> Movement {
        Movement::spawn(self.device.clone(), dx, dy, options, false)
    }

    /// Visits each point in order; every segment uses the same options.
    pub fn move_path(&self, points: &[(i32, i32)], options: MotionOptions) -> Result<()> {
        let shared = crate::motion::MovementShared::default();
        crate::motion::run_path(&self.device, points, &options, &shared)
    }

    pub fn start_move_path(&self, points: Vec<(i32, i32)>, options: MotionOptions) -> Movement {
        Movement::spawn_path(self.device.clone(), points, options)
    }

    pub fn button(&self, button: Button, down: bool) -> Result<()> {
        // Each session owns its own button mask; the firmware unions the owners.
        let current = self.device.buttons();
        let next = if down {
            current | button.bit()
        } else {
            current & !button.bit()
        };
        self.device.set_buttons(next)?;
        self.device.send_input(&Command::MouseButtons(next))
    }

    /// Presses and releases `count` times, with the hold and the gap drawn per
    /// click.
    ///
    /// A constant 25 ms press with a constant 35 ms gap is the clearest timing
    /// signature there is: every program that records input sees identical
    /// numbers forever. A caller that needs an exact press length uses
    /// [`Mouse::click_timed`].
    pub fn click(&self, button: Button, count: u8) -> Result<()> {
        self.click_timed(button, count, 0, 0, 0)
    }

    /// Presses and releases `count` times with the timing the caller asked for.
    ///
    /// `hold_ms` is how long the button stays down and `gap_ms` how long the
    /// SDK waits after the release. A zero means "draw it the way `click`
    /// does", which keeps the humanised default; any other value is used as it
    /// is. `jitter_ms` spreads each of the two by up to that much in either
    /// direction, so a caller can keep a recognisable shape without two clicks
    /// landing on the same millisecond: `jitter_ms == 0` is an exact press.
    ///
    /// A press is never left down: an error part way through releases the
    /// button before the error reaches the caller.
    pub fn click_timed(
        &self,
        button: Button,
        count: u8,
        hold_ms: u32,
        gap_ms: u32,
        jitter_ms: u32,
    ) -> Result<()> {
        if count == 0 {
            return Err(Error::invalid("click count must be at least 1"));
        }
        let mut rng = SplitMix64::from_entropy();
        for _ in 0..count {
            self.button(button, true)?;
            let guard = ReleaseGuard::new(self, Release::Button(button));
            let hold = draw(&mut rng, hold_ms, 70.0, 0.25, 38.0, 150.0, jitter_ms, 1.0);
            crate::timing::sleep_ms(hold);
            self.button(button, false)?;
            guard.disarm();
            let gap = draw(&mut rng, gap_ms, 130.0, 0.30, 60.0, 340.0, jitter_ms, 0.0);
            crate::timing::sleep_ms(gap);
        }
        Ok(())
    }

    pub fn double_click(&self, button: Button) -> Result<()> {
        self.click(button, 2)
    }

    /// Moves to `(x, y)` on `screen` and then clicks there.
    ///
    /// The two halves a task repeats — aim, then press and release — as one
    /// call, so the caller crosses the language boundary once. The move is
    /// clamped into `screen` exactly as [`Mouse::move_to`] clamps it, and the
    /// timing is [`Mouse::click_timed`]'s.
    #[allow(clippy::too_many_arguments)]
    pub fn click_at(
        &self,
        x: i32,
        y: i32,
        screen: crate::platform::Screen,
        button: Button,
        count: u8,
        hold_ms: u32,
        gap_ms: u32,
        jitter_ms: u32,
    ) -> Result<()> {
        if count == 0 {
            return Err(Error::invalid("click count must be at least 1"));
        }
        self.move_to(
            x,
            y,
            MotionOptions {
                screen,
                ..MotionOptions::default()
            },
        )?;
        self.click_timed(button, count, hold_ms, gap_ms, jitter_ms)
    }

    pub fn wheel(&self, vertical: i32, horizontal: i32) -> Result<()> {
        if !(-1024..=1024).contains(&vertical) || !(-1024..=1024).contains(&horizontal) {
            return Err(Error::invalid("wheel is limited to +/-1024 per axis"));
        }
        self.device
            .send_input(&Command::Wheel(vertical as i16, horizontal as i16))
    }

    pub fn drag(&self, button: Button, x: i32, y: i32, options: MotionOptions) -> Result<()> {
        self.button(button, true)?;
        // The release is the caller's whether the move works or not, so the
        // guard stays armed: it releases everything on drop.
        let guard = ReleaseGuard::new(self, Release::All);
        let result = self.move_to(x, y, options);
        drop(guard);
        result
    }

    pub fn release_all(&self) -> Result<()> {
        self.device.release_all()
    }
}

/// One timing draw: the caller's number when there is one, otherwise the
/// humanised log-normal around `median` clamped to `min..=max`. `jitter_ms`
/// moves either kind by up to that much in either direction.
#[allow(clippy::too_many_arguments)]
fn draw(
    rng: &mut SplitMix64,
    fixed: u32,
    median: f64,
    spread: f64,
    min: f64,
    max: f64,
    jitter_ms: u32,
    floor: f64,
) -> f64 {
    let base = if fixed == 0 {
        lognormal(rng, median, spread).clamp(min, max)
    } else {
        f64::from(fixed)
    };
    if jitter_ms == 0 {
        return base.max(floor);
    }
    let offset = (rng.unit() * 2.0 - 1.0) * f64::from(jitter_ms);
    (base + offset).max(floor)
}

/// What a guard releases when it is dropped: every held input, or one button.
enum Release {
    All,
    Button(Button),
}

/// Holds a press open, so an error or an early return while it is down cannot
/// leave it down: the drop sends the release, and `disarm` records that the
/// caller already did.
struct ReleaseGuard<'a> {
    mouse: &'a Mouse,
    release: Release,
    armed: bool,
}

impl<'a> ReleaseGuard<'a> {
    fn new(mouse: &'a Mouse, release: Release) -> Self {
        Self {
            mouse,
            release,
            armed: true,
        }
    }

    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for ReleaseGuard<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        match self.release {
            Release::All => {
                let _ = self.mouse.release_all();
            }
            Release::Button(button) => {
                let _ = self.mouse.button(button, false);
            }
        }
    }
}
