//! Runs the Rust SDK against the in-memory device; no hardware needed.
//!
//!     cargo run -p gdb --example mock

use gdb::{Button, DeviceOptions, Manager, MotionOptions, Priority};

fn main() -> gdb::Result<()> {
    let (device, _mock) = Manager::open_mock_with_handle(DeviceOptions::default())?;
    println!("info: {:?}", device.info());
    println!("status: {:?}", device.status()?);

    device.keyboard().type_text(&gdb::TypeTextConfig {
        text: "hello".to_string(),
        interval_ms: 1,
        hold_ms: 1,
        jitter_ms: 0,
        typo_rate: 0.0,
    })?;
    device.keyboard().chord(&[0xE0, 0x06], 20)?;

    let mouse = device.mouse();
    mouse.move_by(120, -40, MotionOptions::linear(30))?;
    mouse.move_to(400, 300, MotionOptions::default())?;
    mouse.move_path(&[(100, 100), (320, 240)], MotionOptions::linear(40))?;
    mouse.click(Button::Left, 1)?;
    mouse.wheel(120, 0)?;

    let movement = mouse.start_move_to(800, 600, MotionOptions::linear(300));
    movement.retarget(900, 650);
    movement.wait()?;

    let scheduler = device.scheduler();
    scheduler.pause();
    scheduler.submit(std::time::Duration::ZERO, Priority(0), |device| {
        device.barrier()
    })?;
    scheduler.submit(std::time::Duration::ZERO, Priority(10), |device| {
        device.release_all()
    })?;
    scheduler.resume();
    scheduler.wait_idle(std::time::Duration::from_secs(2))?;
    println!("scheduler errors: {:?}", scheduler.take_errors());

    device.close()?;
    println!("rust mock ok");
    Ok(())
}
