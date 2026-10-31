//! Mouse control: buttons, wheel, clicks, drags and the two movement flavours.

use crate::device::Device;
use crate::error::{Error, Result};
use crate::motion::{MotionOptions, Movement, run_move_by, run_move_to};
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

    pub fn click(&self, button: Button, count: u8) -> Result<()> {
        if count == 0 {
            return Err(Error::invalid("click count must be at least 1"));
        }
        for _ in 0..count {
            self.button(button, true)?;
            std::thread::sleep(std::time::Duration::from_millis(25));
            self.button(button, false)?;
            std::thread::sleep(std::time::Duration::from_millis(35));
        }
        Ok(())
    }

    pub fn double_click(&self, button: Button) -> Result<()> {
        self.click(button, 2)
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
        let guard = ReleaseGuard {
            mouse: self,
            armed: true,
        };
        let result = self.move_to(x, y, options);
        drop(guard);
        result
    }

    pub fn release_all(&self) -> Result<()> {
        self.device.release_all()
    }
}

struct ReleaseGuard<'a> {
    mouse: &'a Mouse,
    armed: bool,
}

impl Drop for ReleaseGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.mouse.release_all();
        }
    }
}
