//! Hardware smoke test for the Vendor HID v3 firmware.
//!
//! ```text
//! cargo run --release --no-default-features --example probe
//! cargo run --release --no-default-features --example probe -- --watch 5
//! cargo run --release --no-default-features --example probe -- --lease
//! cargo run --release --no-default-features --example probe -- --bootloader
//! cargo run --release --no-default-features --example probe -- --ext
//! ```
//!
//! It walks the exact path the GUI uses: enumerate the vendor collection, read
//! Feature 14, OPEN a session, read STATUS, keep the lease alive with HEARTBEAT
//! and close with BARRIER. `--lease` sits idle past `lease_ms` and expects the
//! device to drop the session; `--bootloader` asks the device to reboot into
//! BOOTSEL and waits for it to leave the bus. Exit code 0 means every step
//! answered; the printed counters are what you diff between runs.

use std::time::{Duration, Instant};

use firmware_manage_tool::cancel::Cancellation;
use firmware_manage_tool::hid;
use input_protocol::v3::Command;

fn after_value(status: &firmware_manage_tool::protocol::Status, field: &str) -> String {
    status.fields.get(field).cloned().unwrap_or_default()
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let watch_secs = args
        .windows(2)
        .find(|pair| pair[0] == "--watch")
        .and_then(|pair| pair[1].parse::<u64>().ok())
        .unwrap_or(2);
    let lease_check = args.iter().any(|arg| arg == "--lease");
    let enter_bootloader = args.iter().any(|arg| arg == "--bootloader");
    let keys_check = args.iter().any(|arg| arg == "--keys");
    let ext_check = args.iter().any(|arg| arg == "--ext");

    let devices = hid::enumerate()?;
    if devices.is_empty() {
        anyhow::bail!(
            "找不到 Vendor HID（VID:PID 0x03F0:0x0024，usage page 0xFF60 / usage 0x61）。\n\
             依序檢查：\n\
             1. 板子是否已燒錄並重新插拔（BOOTSEL 狀態會出現 RPI-RP2 磁碟，不會有 HID）；\n\
             2. 裝置管理員是否出現「HP Keyboard」（含兩個 HID 介面）；\n\
             3. 是否有其他程式佔用（GUI 或舊的 latency worker）。"
        );
    }
    for device in &devices {
        println!("found: {}", device.label());
    }
    let device = &devices[0];

    let cancel = Cancellation::default();
    let mut session = hid::open(device)?;

    session.synchronize(&cancel)?;
    let (board, firmware, lease_ms, poll_ms, flags, consumer, device_id, build) = {
        let caps = session
            .capabilities()
            .ok_or_else(|| anyhow::anyhow!("synchronize 後沒有 capabilities"))?;
        (
            caps.board,
            caps.firmware.clone(),
            caps.lease_ms,
            caps.poll_ms,
            caps.flags,
            caps.consumer,
            caps.device_id,
            caps.build,
        )
    };
    println!(
        "feature14: board={board} firmware={firmware} build={build:08x} lease={lease_ms}ms poll={poll_ms}ms flags={flags:#08b} consumer={consumer:#04X} device={device_id:02X?}"
    );
    anyhow::ensure!(board == 2, "board id 應為 2（XIAO RP2040），實際 {board}");

    let hello = session.hello(&cancel)?;
    println!(
        "hello: protocol={} nkro={} media={} mouse={} lease={}ms fw={}",
        hello.protocol, hello.nkro, hello.media, hello.mouse, hello.lease_ms, hello.firmware
    );

    let first = session.status(&cancel)?;
    println!("status: {}", first.raw);

    if ext_check {
        for _ in 0..2 {
            let ext = session.status_ext(&cancel)?;
            println!(
                "ext: uptime={}ms late_max={}us out={} rx={} tx={} sessions={} last_fault={} age={:?} reset={} boots={} build={:08x}",
                ext.uptime_ms,
                ext.late_max_us,
                ext.out_depth,
                ext.rx_depth,
                ext.tx_depth,
                ext.sessions,
                ext.last_fault,
                ext.last_fault_age_ms,
                ext.reset_reason,
                ext.boot_count,
                ext.build
            );
            anyhow::ensure!(ext.sessions >= 1, "STATUS_EXT 應顯示至少一個 session");
            anyhow::ensure!(ext.boot_count >= 1, "boot count 應至少為 1");
            anyhow::ensure!(
                ext.build != 0,
                "build hash 為 0：Feature 14 沒有帶入 commit"
            );
            cancel.sleep(Duration::from_millis(20))?;
        }
        println!("ext ok: STATUS_EXT 回報 uptime／佇列水位／reset reason／build hash");
    }

    if keys_check {
        // Windows claims the keyboard collection for itself (raw ReadFile returns
        // ERROR_ACCESS_DENIED), so the input path is proven from the device side:
        // STATUS must show the held key and the completion counter must move,
        // which only happens when the host polls the report off the endpoint.
        let key = input_protocol::KeyId {
            usage_page: 7,
            usage: 4, // 'a'
        };
        let before = session.status(&cancel)?;
        session.input(Command::Key(key, true), &cancel)?;
        cancel.sleep(Duration::from_millis(60))?;
        let held = session.status(&cancel)?;
        session.input(Command::Key(key, false), &cancel)?;
        cancel.sleep(Duration::from_millis(60))?;
        let released = session.status(&cancel)?;

        println!("before:   {}", before.raw);
        println!("held:     {}", held.raw);
        println!("released: {}", released.raw);
        anyhow::ensure!(
            after_value(&held, "keys") == "a",
            "按住後 STATUS 的 keys 應為 \"a\"，實際 {:?}",
            after_value(&held, "keys")
        );
        anyhow::ensure!(
            after_value(&released, "keys").is_empty(),
            "放開後 STATUS 的 keys 應為空，實際 {:?}",
            after_value(&released, "keys")
        );
        anyhow::ensure!(
            held.count("completed") > before.count("completed"),
            "keyboard report 沒有完成（completed {} → {}）",
            before.count("completed"),
            held.count("completed")
        );
        println!("keys ok: 按住→keys=a、completed 前進、放開→清空");
        session.finish(&cancel)?;
        session.close(&cancel)?;
        return Ok(());
    }

    if enter_bootloader {
        session.input(Command::Bootloader, &cancel)?;
        println!("bootloader: BOOTLOADER 已被接受，等待裝置離開");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !hid::enumerate()?.is_empty() {
            anyhow::ensure!(
                Instant::now() < deadline,
                "裝置 5 秒內沒有離開（BOOTSEL 未生效？）"
            );
            cancel.sleep(Duration::from_millis(100))?;
        }
        println!("bootloader: 裝置已離線，應出現 RPI-RP2 磁碟");
        return Ok(());
    }

    if lease_check {
        // No HEARTBEAT on purpose: the device must drop the session itself.
        cancel.sleep(Duration::from_millis(u64::from(lease_ms) + 500))?;
        let outcome = session.status(&cancel);
        anyhow::ensure!(
            outcome.is_err() && !session.is_valid(),
            "閒置超過 {} ms 後仍可通訊，租約沒有生效",
            lease_ms
        );
        println!(
            "lease ok: 裝置在 {} ms 後釋放 session（{}）",
            lease_ms,
            outcome.unwrap_err()
        );
        return Ok(());
    }

    // HEARTBEAT every 500 ms keeps the lease alive while the bus is polled.
    let deadline = Instant::now() + Duration::from_secs(watch_secs.max(1));
    session.wait_until(deadline, &cancel)?;
    let last = session.status(&cancel)?;
    last.check_transport_since(&first)?;
    println!(
        "watch {}s: hid/tx 計數未變動: {}",
        watch_secs.max(1),
        last.raw
    );

    let ext = session.status_ext(&cancel)?;
    println!(
        "ext: uptime={}ms late_max={}us out={} rx={} tx={} sessions={} boots={} build={:08x}",
        ext.uptime_ms,
        ext.late_max_us,
        ext.out_depth,
        ext.rx_depth,
        ext.tx_depth,
        ext.sessions,
        ext.boot_count,
        ext.build
    );
    session.finish(&cancel)?;
    session.close(&cancel)?;
    println!("ok: Feature 14 → OPEN → STATUS → STATUS_EXT → HEARTBEAT → BARRIER → CLOSE 全部通過");
    Ok(())
}
