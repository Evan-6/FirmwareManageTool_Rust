//! Firmware upload: build the Rust firmware with cargo, pack it into a UF2, hand
//! it to the RP2040 bootloader drive and verify the device comes back.
//!
//! Nothing here needs the Arduino toolchain. The only external requirement is a
//! Rust toolchain with the `thumbv6m-none-eabi` target; `skip_dependencies`
//! avoids even the `rustup target add` when the machine already has it.

use crate::{
    cancel::Cancellation,
    hid::{self, Device},
    platform,
    process::{ProcessLine, ProcessRunner, ProcessSpec},
    uf2,
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// Crate name of the firmware binary this path builds.
pub const APP_CRATE: &str = "xiao-firmware";
/// Only supported firmware target: Seeed XIAO RP2040 (Cortex-M0+).
pub const TARGET: &str = "thumbv6m-none-eabi";
/// Feature that turns on the embassy USB device; without it the crate is a lib.
pub const FEATURES: &str = "hardware";
/// Feature `board` id reported by the XIAO firmware build.
pub const BOARD_ID: u8 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UploadConfig {
    /// Explicit `cargo.exe`; empty means "look on PATH".
    pub cargo_path: String,
    /// Firmware root (the folder that contains `app/`); empty means the shipped one.
    pub firmware_path: String,
    /// Skip `rustup target add` when the target is known to be installed.
    pub skip_dependencies: bool,
    pub auto_bootloader: bool,
    pub boot_target: String,
}
impl Default for UploadConfig {
    fn default() -> Self {
        Self {
            cargo_path: String::new(),
            firmware_path: String::new(),
            skip_dependencies: false,
            auto_bootloader: true,
            boot_target: String::new(),
        }
    }
}
impl UploadConfig {
    pub fn sources(&self) -> PathBuf {
        if self.firmware_path.trim().is_empty() {
            platform::firmware_dir()
        } else {
            PathBuf::from(&self.firmware_path)
        }
    }
    pub fn app(&self) -> PathBuf {
        self.sources().join("app")
    }
    pub fn elf(&self) -> PathBuf {
        self.app()
            .join("target")
            .join(TARGET)
            .join("release")
            .join(APP_CRATE)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UploadOutcome {
    TransferFailed(String),
    TransferredUnverified(String),
    Verified(String),
}
impl UploadOutcome {
    pub fn label(&self) -> String {
        match self {
            Self::TransferFailed(e) => format!("傳輸失敗：{e}"),
            Self::TransferredUnverified(e) => format!("傳輸完成，但未驗證：{e}"),
            Self::Verified(e) => format!("傳輸完成且握手通過：{e}"),
        }
    }
}
pub trait UploadObserver {
    fn log(&mut self, line: String);
    fn progress(&mut self, stage: usize, message: &str);
    fn choose_target(&mut self, candidates: Vec<String>, cancel: &Cancellation) -> Result<String>;
}

fn args(items: &[&str]) -> Vec<OsString> {
    items.iter().map(OsString::from).collect()
}

fn run_tool<R: ProcessRunner>(
    runner: &R,
    program: &Path,
    arguments: Vec<OsString>,
    timeout: Duration,
    cancel: &Cancellation,
    observer: &mut dyn UploadObserver,
) -> Result<String> {
    observer.log(format!(
        "{} {}",
        program.display(),
        arguments
            .iter()
            .map(|s| s.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    ));
    let result = runner.run(
        &ProcessSpec {
            program: program.to_path_buf(),
            args: arguments,
            timeout,
        },
        cancel,
        &mut |line: ProcessLine| observer.log(format!("{:?}: {}", line.stream, line.text)),
    )?;
    result.check()?;
    Ok(result.stdout)
}

/// `cargo`, preferring an explicit setting.
fn ensure_cargo(config: &UploadConfig) -> Result<PathBuf> {
    if !config.cargo_path.trim().is_empty() {
        let path = PathBuf::from(config.cargo_path.trim());
        ensure!(path.is_file(), "指定的 cargo 不存在：{}", path.display());
        return Ok(path);
    }
    #[cfg(windows)]
    let name = "cargo.exe";
    #[cfg(not(windows))]
    let name = "cargo";
    platform::locate_program(name)
        .ok_or_else(|| anyhow::anyhow!("找不到 cargo；請安裝 Rust 工具鏈，或在設定指定 cargo 路徑"))
}

/// rustup normally sits beside cargo; without it the target cannot be added.
fn rustup_beside(cargo: &Path) -> Option<PathBuf> {
    let dir = cargo.parent()?;
    #[cfg(windows)]
    let name = "rustup.exe";
    #[cfg(not(windows))]
    let name = "rustup";
    let candidate = dir.join(name);
    candidate.is_file().then_some(candidate)
}

/// Builds `firmware/app` for the RP2040 and writes the packed UF2 into `root`.
fn build_firmware<R: ProcessRunner>(
    config: &UploadConfig,
    runner: &R,
    cancel: &Cancellation,
    observer: &mut dyn UploadObserver,
    root: &Path,
) -> Result<PathBuf> {
    let manifest = config.app().join("Cargo.toml");
    ensure!(
        manifest.is_file(),
        "韌體資料夾不完整（缺 app/Cargo.toml）：{}",
        config.sources().display()
    );
    observer.progress(1, "檢查 Rust 工具鏈");
    let cargo = ensure_cargo(config)?;
    if !config.skip_dependencies {
        match rustup_beside(&cargo) {
            Some(rustup) => {
                run_tool(
                    runner,
                    &rustup,
                    args(&["target", "add", TARGET]),
                    Duration::from_secs(600),
                    cancel,
                    observer,
                )?;
            }
            None => observer.log(format!(
                "找不到 rustup，跳過 target 安裝（假設 {TARGET} 已安裝）"
            )),
        }
    }
    observer.progress(2, "編譯 Rust 韌體");
    let mut build = args(&["build", "--release", "--target", TARGET, "--manifest-path"]);
    build.push(manifest.into_os_string());
    build.extend(args(&["--features", FEATURES]));
    run_tool(
        runner,
        &cargo,
        build,
        Duration::from_secs(900),
        cancel,
        observer,
    )?;

    let elf = config.elf();
    ensure!(elf.is_file(), "編譯成功但找不到 ELF：{}", elf.display());
    let image = uf2::pack(&std::fs::read(&elf)?)?;
    observer.log(format!("UF2：{} 個區塊", uf2::blocks(&image)));
    std::fs::create_dir_all(root)?;
    let out = root.join("Firmware.uf2");
    std::fs::write(&out, &image).with_context(|| format!("寫入 UF2 失敗：{}", out.display()))?;
    Ok(out)
}

pub fn upload<R: ProcessRunner>(
    config: &UploadConfig,
    selected: Option<&Device>,
    runner: &R,
    cancel: &Cancellation,
    observer: &mut dyn UploadObserver,
) -> UploadOutcome {
    match transfer(config, selected, runner, cancel, observer) {
        Err(e) => UploadOutcome::TransferFailed(format!("{e:#}")),
        Ok(existing_paths) => {
            observer.progress(5, "等待 runtime HID 重新列舉與 hello 驗證");
            match verify_runtime(selected, cancel, Duration::from_secs(15), &existing_paths) {
                Ok(message) => UploadOutcome::Verified(message),
                Err(e) => UploadOutcome::TransferredUnverified(format!("{e:#}")),
            }
        }
    }
}
fn transfer<R: ProcessRunner>(
    config: &UploadConfig,
    selected: Option<&Device>,
    runner: &R,
    cancel: &Cancellation,
    observer: &mut dyn UploadObserver,
) -> Result<Vec<String>> {
    transfer_at(
        config,
        selected,
        runner,
        cancel,
        observer,
        &platform::application_dir()?,
    )
}
fn transfer_at<R: ProcessRunner>(
    config: &UploadConfig,
    selected: Option<&Device>,
    runner: &R,
    cancel: &Cancellation,
    observer: &mut dyn UploadObserver,
    root: &Path,
) -> Result<Vec<String>> {
    cancel.check()?;
    let uf2 = build_firmware(config, runner, cancel, observer, root)?;
    observer.progress(3, "進入並選擇 bootloader");
    let explicit = !config.boot_target.trim().is_empty();
    // If rebooting a selected runtime device, never auto-select a UF2 drive that
    // was already mounted before that command.
    let previous_targets = if config.auto_bootloader && !explicit && selected.is_some() {
        platform::boot_drives()?
            .iter()
            .map(|p| p.display().to_string())
            .collect()
    } else {
        Vec::new()
    };
    if config.auto_bootloader && !explicit {
        if let Some(device) = selected {
            let mut session = hid::open(device)?;
            session.synchronize(cancel)?;
            session.hello(cancel)?;
            match session.input(input_protocol::v3::Command::Bootloader, cancel) {
                Ok(_) => observer.log("已確認 enter_bootloader；等待裝置重啟".into()),
                Err(e) => {
                    cancel.check()?;
                    observer.log(format!(
                        "bootloader 命令未收到完整確認，繼續檢查列舉結果：{e:#}"
                    ));
                }
            }
            drop(session);
        } else {
            observer.log("未選取 runtime HID，請使用實體 BOOTSEL 進入下載模式".into());
        }
    }
    let target = if explicit {
        config.boot_target.trim().to_owned()
    } else {
        wait_drives(cancel, observer, &previous_targets)?
    };
    cancel.check()?;
    // Devices already in runtime at this point are unrelated to this bootloader
    // transfer. Excluding them prevents an existing board's hello from falsely
    // verifying another board.
    let existing_paths = hid::enumerate()?.into_iter().map(|d| d.path).collect();
    observer.progress(4, "傳輸韌體");
    let drive = PathBuf::from(&target);
    ensure!(
        drive.join("INFO_UF2.TXT").is_file(),
        "指定位置不是 UF2 bootloader 磁碟"
    );
    copy_uf2(&uf2, &drive.join("Firmware.uf2"), cancel)?;
    observer.log("UF2 複製完成，等待 runtime 驗證".into());
    Ok(existing_paths)
}

pub fn copy_uf2(source: &Path, target: &Path, cancel: &Cancellation) -> Result<()> {
    use std::io::{Read, Write};
    let mut input = std::fs::File::open(source)?;
    let mut output = std::fs::File::create(target)
        .with_context(|| format!("建立 UF2 目的檔失敗：{}", target.display()))?;
    let mut buffer = [0; 16 * 1024];
    loop {
        cancel.check()?;
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        output
            .write_all(&buffer[..count])
            .with_context(|| format!("UF2 複製失敗（保留 OS 錯誤碼）：{}", target.display()))?;
    }
    output.flush()?;
    Ok(())
}

fn wait_drives(
    cancel: &Cancellation,
    observer: &mut dyn UploadObserver,
    previous_targets: &[String],
) -> Result<String> {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        cancel.check()?;
        let drives = platform::boot_drives()?
            .into_iter()
            .filter(|p| !previous_targets.contains(&p.display().to_string()))
            .collect::<Vec<_>>();
        if drives.len() == 1 {
            return Ok(drives[0].display().to_string());
        }
        if drives.len() > 1 {
            return observer.choose_target(
                drives.iter().map(|p| p.display().to_string()).collect(),
                cancel,
            );
        }
        cancel.sleep(Duration::from_millis(250))?;
    }
    bail!("15 秒內未偵測到 RP2040 UF2 磁碟；請 BOOTSEL 上電後重試")
}
fn verify_runtime(
    selected: Option<&Device>,
    cancel: &Cancellation,
    timeout: Duration,
    existing_paths: &[String],
) -> Result<String> {
    let selected = selected.filter(|device| !existing_paths.contains(&device.path));
    wait_for_runtime(
        selected,
        cancel,
        timeout,
        || Ok(runtime_candidates(hid::enumerate()?, existing_paths)),
        |device, token| {
            let mut session = hid::open(device)?;
            session.synchronize(token)?;
            if let Some(caps) = session.capabilities() {
                verify_capability(caps.board)?;
            }
            let caps = session
                .capabilities()
                .ok_or_else(|| anyhow::anyhow!("重新列舉後沒有 v3 能力"))?
                .clone();
            Ok(format!(
                "board={}, fw={}, lease={} ms",
                caps.board, caps.firmware, caps.lease_ms
            ))
        },
    )
}
fn verify_capability(reported: u8) -> Result<()> {
    ensure!(
        reported == BOARD_ID,
        "重新列舉的板型不符：{reported}（預期 {BOARD_ID}）"
    );
    Ok(())
}
fn runtime_candidates(devices: Vec<Device>, existing_paths: &[String]) -> Vec<Device> {
    devices
        .into_iter()
        .filter(|d| !existing_paths.contains(&d.path))
        .collect()
}
fn wait_for_runtime<E, H>(
    selected: Option<&Device>,
    cancel: &Cancellation,
    timeout: Duration,
    mut enumerate: E,
    mut hello: H,
) -> Result<String>
where
    E: FnMut() -> Result<Vec<Device>>,
    H: FnMut(&Device, &Cancellation) -> Result<String>,
{
    let deadline = Instant::now() + timeout;
    let mut last = String::from("尚未列舉");
    while Instant::now() < deadline {
        cancel.check()?;
        let devices = enumerate()?;
        let matched: Vec<_> = devices
            .iter()
            .filter(|d| {
                selected.is_none_or(|old| {
                    d.path == old.path
                        || (!old.serial.is_empty()
                            && old.serial == d.serial
                            && old.vid == d.vid
                            && old.pid == d.pid)
                })
            })
            .collect();
        // Duplicate firmware serials cannot prove device identity. Do not select the first match.
        if matched.len() == 1 {
            match hello(matched[0], cancel) {
                Ok(response) => return Ok(response),
                Err(e) => last = format!("{e:#}"),
            }
        } else if matched.len() > 1 {
            last = "多個符合裝置，無法確認燒錄目標；請僅連接目標板後驗證".into();
        }
        cancel.sleep(Duration::from_millis(250))?;
    }
    bail!("runtime hello 驗證逾時：{last}")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn flashed_capability_must_be_the_xiao_board_id() {
        assert!(verify_capability(BOARD_ID).is_ok());
        // board=1 (Pico) and board=3 (AVR) builds must not verify as a XIAO.
        assert!(verify_capability(1).is_err());
        assert!(verify_capability(3).is_err());
    }
    #[test]
    fn the_build_is_pinned_to_one_crate_and_target() {
        let config = UploadConfig::default();
        assert_eq!(TARGET, "thumbv6m-none-eabi");
        assert_eq!(APP_CRATE, "xiao-firmware");
        assert!(config.app().ends_with("firmware/app"));
        assert!(
            config
                .elf()
                .ends_with("firmware/app/target/thumbv6m-none-eabi/release/xiao-firmware")
        );
    }
    #[test]
    fn uf2_copy_errors_and_cancel() {
        let token = Cancellation::default();
        assert!(
            copy_uf2(
                Path::new("/not/a/source"),
                Path::new("/not/a/target"),
                &token
            )
            .is_err()
        );
        token.cancel();
        assert!(token.check().is_err());
    }
    #[test]
    fn runtime_timeout_does_not_pass() {
        let start = Instant::now();
        let result = wait_for_runtime(
            None,
            &Cancellation::default(),
            Duration::from_millis(20),
            || Ok(Vec::new()),
            |_, _| Ok("unexpected".into()),
        );
        assert!(result.is_err());
        assert!(start.elapsed() < Duration::from_secs(1));
    }
    struct FakeRunner {
        fail_on: String,
        calls: std::sync::Mutex<Vec<Vec<OsString>>>,
    }
    impl ProcessRunner for FakeRunner {
        fn run(
            &self,
            spec: &ProcessSpec,
            _: &Cancellation,
            _: &mut dyn FnMut(ProcessLine),
        ) -> Result<crate::process::ProcessResult> {
            self.calls.lock().unwrap().push(spec.args.clone());
            let joined = spec
                .args
                .iter()
                .map(|s| s.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ");
            let failed = joined.contains(&self.fail_on);
            Ok(crate::process::ProcessResult {
                stdout: String::new(),
                stderr: if failed {
                    "injected failure".into()
                } else {
                    String::new()
                },
                exit_code: Some(if failed { 9 } else { 0 }),
                timed_out: false,
                cancelled: false,
            })
        }
    }
    #[derive(Default)]
    struct FakeObserver {
        choices: usize,
    }
    impl UploadObserver for FakeObserver {
        fn log(&mut self, _: String) {}
        fn progress(&mut self, _: usize, _: &str) {}
        fn choose_target(&mut self, candidates: Vec<String>, _: &Cancellation) -> Result<String> {
            self.choices += 1;
            Ok(candidates.last().unwrap().clone())
        }
    }
    #[test]
    fn a_failing_build_leaves_no_uf2_behind() {
        let root = crate::test_support::Temp::new();
        let config = UploadConfig {
            cargo_path: std::env::current_exe().unwrap().display().to_string(),
            firmware_path: Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("firmware")
                .display()
                .to_string(),
            ..Default::default()
        };
        let runner = FakeRunner {
            fail_on: "--features".into(),
            calls: std::sync::Mutex::new(Vec::new()),
        };
        let result = build_firmware(
            &config,
            &runner,
            &Cancellation::default(),
            &mut FakeObserver::default(),
            &root.0,
        );
        assert!(result.is_err());
        assert!(!root.0.join("Firmware.uf2").exists());
        let calls = runner.calls.lock().unwrap();
        assert!(
            calls.iter().any(|args| args.iter().any(|a| a == "build")),
            "the build must be attempted"
        );
        assert!(
            calls
                .iter()
                .filter(|args| args.iter().any(|a| a == "--features"))
                .all(|args| args.iter().any(|a| a == "build")),
            "only the build itself may carry --features"
        );
    }
    #[test]
    fn a_cargo_path_that_does_not_exist_is_rejected() {
        let root = crate::test_support::Temp::new();
        let config = UploadConfig {
            cargo_path: root.0.join("no-cargo.exe").display().to_string(),
            firmware_path: Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("firmware")
                .display()
                .to_string(),
            ..Default::default()
        };
        let error = build_firmware(
            &config,
            &FakeRunner {
                fail_on: String::new(),
                calls: std::sync::Mutex::new(Vec::new()),
            },
            &Cancellation::default(),
            &mut FakeObserver::default(),
            &root.0,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("找不到") || format!("{error:#}").contains("不存在"));
    }
    #[test]
    fn a_missing_firmware_folder_is_reported_before_building() {
        let root = crate::test_support::Temp::new();
        let config = UploadConfig {
            cargo_path: std::env::current_exe().unwrap().display().to_string(),
            firmware_path: root.0.join("nowhere").display().to_string(),
            ..Default::default()
        };
        let error = transfer_at(
            &config,
            None,
            &FakeRunner {
                fail_on: String::new(),
                calls: std::sync::Mutex::new(Vec::new()),
            },
            &Cancellation::default(),
            &mut FakeObserver::default(),
            &root.0,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("app/Cargo.toml"));
    }
    #[test]
    fn uf2_destination_error_retains_os_error_and_cancel_does_not_report_success() {
        let root = crate::test_support::Temp::new();
        let source = root.0.join("source.uf2");
        std::fs::write(&source, [1, 2, 3, 4]).unwrap();
        let error = copy_uf2(
            &source,
            &root.0.join("missing/output.uf2"),
            &Cancellation::default(),
        )
        .unwrap_err();
        assert!(error.chain().any(|cause| {
            cause
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.raw_os_error().is_some())
        }));
        let token = Cancellation::default();
        token.cancel();
        assert!(copy_uf2(&source, &root.0.join("output.uf2"), &token).is_err());
    }
    #[test]
    fn runtime_reenumeration_retries_handshake_and_rejects_ambiguity() {
        let device = Device {
            path: "new".into(),
            serial: "serial".into(),
            product: "fake".into(),
            vid: 0x03f0,
            pid: 0x0024,
        };
        let old = Device {
            path: "old".into(),
            ..device.clone()
        };
        let mut tries = 0;
        let result = wait_for_runtime(
            Some(&old),
            &Cancellation::default(),
            Duration::from_secs(1),
            || Ok(vec![device.clone()]),
            |_, _| {
                tries += 1;
                if tries == 1 {
                    bail!("delayed handshake");
                }
                Ok("ok:hello,protocol=3".into())
            },
        );
        assert!(result.is_ok());
        assert_eq!(tries, 2);
        let result = wait_for_runtime(
            Some(&old),
            &Cancellation::default(),
            Duration::from_millis(1),
            || Ok(vec![device.clone(), device.clone()]),
            |_, _| panic!("must not select first device"),
        );
        assert!(result.is_err());
    }
    #[test]
    fn an_existing_runtime_board_cannot_verify_another_boards_transfer() {
        let existing = Device {
            path: "already-running".into(),
            serial: "shared-firmware-serial".into(),
            product: "fake".into(),
            vid: 0x03f0,
            pid: 0x0024,
        };
        let returning = Device {
            path: "just-reenumerated".into(),
            ..existing.clone()
        };
        let candidates = runtime_candidates(
            vec![existing.clone(), returning.clone()],
            std::slice::from_ref(&existing.path),
        );
        assert_eq!(candidates, [returning]);
    }
}
