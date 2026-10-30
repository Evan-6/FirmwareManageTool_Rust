//! Reboot a running device into BOOTSEL using nothing but raw vendor reports.
//!
//! ```text
//! cargo run --release --no-default-features --example boot
//! ```
//!
//! The normal path is `probe --bootloader`, but that one has to read Feature 14
//! first. This fallback only needs the vendor OUT endpoint, so it still works
//! when the capability block is broken — which is exactly when you need to
//! reflash.

use std::time::{Duration, Instant};

use firmware_manage_tool::hid;

const VENDOR_USAGE_PAGE: u16 = 0xff60;
const VENDOR_USAGE: u16 = 0x61;
const VID: u16 = 0x03f0;
const PID: u16 = 0x0024;
const DATA_REPORT_ID: u8 = 12;
const WIRE_VERSION: u8 = 3;
const OPEN: u8 = 1;
const BOOTLOADER: u8 = 6;

/// One 64-byte vendor OUT report: report id, then the 63-byte wire payload.
fn frame(op: u8, session: u32, sequence: u32) -> [u8; 64] {
    let mut f = [0u8; 64];
    f[0] = DATA_REPORT_ID;
    f[1] = WIRE_VERSION;
    f[2] = op;
    f[3] = 0; // payload length
    f[5..9].copy_from_slice(&session.to_le_bytes());
    f[9..13].copy_from_slice(&sequence.to_le_bytes());
    f
}

fn main() -> anyhow::Result<()> {
    let api = hidapi::HidApi::new()?;
    let target = api
        .device_list()
        .find(|device| {
            device.vendor_id() == VID
                && device.product_id() == PID
                && device.usage_page() == VENDOR_USAGE_PAGE
                && device.usage() == VENDOR_USAGE
        })
        .ok_or_else(|| anyhow::anyhow!("找不到 vendor HID，裝置可能已在 BOOTSEL 或未插上"))?;

    let device = api.open_path(target.path())?;
    let session = 0xa1b2_c3d4;

    device.write(&frame(OPEN, session, 1))?;
    std::thread::sleep(Duration::from_millis(20));
    device.write(&frame(BOOTLOADER, session, 2))?;
    println!("已送出 OPEN + BOOTLOADER（session {session:#010X}），等待裝置離開");

    let deadline = Instant::now() + Duration::from_secs(5);
    while !hid::enumerate()?.is_empty() {
        anyhow::ensure!(Instant::now() < deadline, "5 秒內裝置沒有離開");
        std::thread::sleep(Duration::from_millis(100));
    }
    println!("裝置已離線，應出現 RPI-RP2 磁碟");
    Ok(())
}
