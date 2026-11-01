//! Firmware build and flashing for the XIAO RP2040, host side.
//!
//! Three things learned on hardware decide the shape of this module:
//!
//! * the board refuses BOOTLOADER while a host still polls it, so the runtime
//!   session is closed *before* the drive is waited for;
//! * the UF2 drive only appears after that reboot, so "was it mounted before
//!   this command" — not its letter — is what identifies the right one;
//! * verification is a second enumeration plus a Feature 14 read, not the reply
//!   of the copy: the old handle is gone by the time the board is back.
//!
//! Nothing here knows about a GUI. cargo comes in through [`Runner`], progress
//! through [`Reporter`] and cancellation through [`Cancel`], so the same
//! sequence runs from any host program; the messages are the ones a person
//! reads while flashing, hence Chinese.

use crate::device::{Device, DeviceOptions};
use crate::error::{Error, ErrorCode, Result};
use crate::manager::{DeviceEntry, Manager};
use std::ffi::OsString;
use std::fmt;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub mod uf2;

/// Crate name of the firmware binary this module builds.
pub const APP_CRATE: &str = "xiao-firmware";
/// Only supported firmware target: Seeed XIAO RP2040 (Cortex-M0+).
pub const TARGET: &str = "thumbv6m-none-eabi";
/// Feature that turns on the embassy USB device; without it the crate is a lib.
pub const FEATURES: &str = "hardware";
/// Feature `board` id reported by the XIAO firmware build.
pub const BOARD_ID: u8 = 2;

/// How long the UF2 drive may take to appear after the reboot.
const DRIVE_WAIT: Duration = Duration::from_secs(15);
/// How long the runtime HID may take to come back and answer Feature 14.
const HANDSHAKE_WAIT: Duration = Duration::from_secs(15);
/// Copy chunk: a cancelled transfer stops between chunks, not mid-block.
const COPY_CHUNK: usize = 16 * 1024;
/// Poll interval for "is it there yet" loops.
const POLL: Duration = Duration::from_millis(250);

/// Where the firmware lives and how the board should be put into BOOTSEL.
#[derive(Clone, Debug)]
pub struct Config {
    /// Explicit cargo binary; `None` looks it up on PATH.
    pub cargo: Option<PathBuf>,
    /// Firmware root: the folder that contains `app/`.
    pub firmware: PathBuf,
    /// Skip `rustup target add` when the target is known to be installed.
    pub skip_dependencies: bool,
    /// Send BOOTLOADER to the selected board instead of asking for BOOTSEL.
    pub auto_bootloader: bool,
    /// An explicit UF2 drive; `None` waits for the one this command created.
    pub boot_drive: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            cargo: None,
            firmware: PathBuf::from("firmware"),
            skip_dependencies: false,
            auto_bootloader: true,
            boot_drive: None,
        }
    }
}

impl Config {
    pub fn new(firmware: impl Into<PathBuf>) -> Self {
        Self {
            firmware: firmware.into(),
            ..Self::default()
        }
    }

    pub fn app(&self) -> PathBuf {
        self.firmware.join("app")
    }

    pub fn elf(&self) -> PathBuf {
        self.app()
            .join("target")
            .join(TARGET)
            .join("release")
            .join(APP_CRATE)
    }
}

/// A packed image: the bytes the bootloader drive expects and their block count.
#[derive(Debug)]
pub struct Image {
    pub bytes: Vec<u8>,
    pub blocks: usize,
}

/// Cancellation as the flashing sequence needs it: one question, asked often.
///
/// The host program owns the token (an atomic, a channel, a GUI flag) and this
/// module never sleeps through it.
pub trait Cancel {
    fn cancelled(&self) -> bool;
}

impl<T: Cancel + ?Sized> Cancel for &T {
    fn cancelled(&self) -> bool {
        (**self).cancelled()
    }
}

/// Progress, in the order the user sees it: stages 1-5 and a log of what ran.
pub trait Reporter {
    fn log(&mut self, line: String);
    fn stage(&mut self, stage: u8, message: &str);
    /// Several UF2 drives are mounted; the answer must be one of `candidates`.
    ///
    /// Refusing by default is deliberate: guessing here flashes the wrong board.
    fn choose_drive(&mut self, candidates: Vec<String>) -> Result<String> {
        Err(Error::invalid(format!(
            "同時偵測到多個 UF2 磁碟（{}），請指定 boot 磁碟",
            candidates.join("、")
        )))
    }
}

/// One line of child output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

/// What a finished child program reported.
#[derive(Clone, Debug, Default)]
pub struct Outcome {
    pub stdout: String,
    pub stderr: String,
    pub exit: Option<i32>,
    pub timed_out: bool,
    pub cancelled: bool,
}

impl Outcome {
    /// Returns stdout when the program succeeded, a usable message when not.
    pub fn check(self) -> Result<String> {
        if self.cancelled {
            return Err(Error::new(ErrorCode::Cancelled, "操作已取消"));
        }
        if self.timed_out {
            return Err(Error::timeout("子程序逾時"));
        }
        if self.exit != Some(0) {
            return Err(Error::new(
                ErrorCode::Io,
                format!(
                    "子程序失敗，exit={:?}\n{}\n{}",
                    self.exit, self.stderr, self.stdout
                ),
            ));
        }
        Ok(self.stdout)
    }
}

/// Runs an external program; the host supplies its own when it already has one
/// (the desktop tool kills a whole process tree, which a library cannot assume).
pub trait Runner {
    fn run(
        &self,
        program: &Path,
        args: &[OsString],
        timeout: Duration,
        cancel: &dyn Cancel,
        log: &mut dyn FnMut(Stream, &str),
    ) -> Result<Outcome>;
}

/// The plain implementation: spawn, stream, kill on cancel or timeout.
pub struct SystemRunner;

impl Runner for SystemRunner {
    fn run(
        &self,
        program: &Path,
        args: &[OsString],
        timeout: Duration,
        cancel: &dyn Cancel,
        log: &mut dyn FnMut(Stream, &str),
    ) -> Result<Outcome> {
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|error| Error::io(format!("無法啟動 {}：{error}", program.display())))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::io("子程序沒有 stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| Error::io("子程序沒有 stderr"))?;
        let (sender, receiver) = mpsc::channel();
        let readers = [
            read_lines(stdout, Stream::Stdout, sender.clone()),
            read_lines(stderr, Stream::Stderr, sender.clone()),
        ];
        drop(sender);

        let mut outcome = Outcome::default();
        let started = Instant::now();
        loop {
            drain(&receiver, Duration::from_millis(20), &mut outcome, log);
            if cancel.cancelled() || started.elapsed() >= timeout {
                outcome.cancelled = cancel.cancelled();
                outcome.timed_out = !outcome.cancelled;
                let _ = child.kill();
                outcome.exit = child.wait().map(|status| status.code()).ok().flatten();
                break;
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    outcome.exit = status.code();
                    break;
                }
                Ok(None) => {}
                Err(error) => return Err(Error::io(error.to_string())),
            }
        }
        // A grandchild can keep the pipes open after the child is gone, so the
        // readers are given a moment and then left to finish on their own.
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            if !drain(&receiver, Duration::from_millis(20), &mut outcome, log) {
                break;
            }
        }
        drop(readers);
        Ok(outcome)
    }
}

/// Moves whatever the readers produced into `outcome`; `false` once they are all
/// gone, so a caller can stop waiting.
fn drain(
    receiver: &mpsc::Receiver<Line>,
    wait: Duration,
    outcome: &mut Outcome,
    log: &mut dyn FnMut(Stream, &str),
) -> bool {
    loop {
        match receiver.recv_timeout(wait) {
            Ok(line) => {
                let sink = match line.stream {
                    Stream::Stdout => &mut outcome.stdout,
                    Stream::Stderr => &mut outcome.stderr,
                };
                // Bound what is retained; every line still reaches the log.
                if sink.len() + line.text.len() <= 10 * 1024 * 1024 {
                    sink.push_str(&line.text);
                    sink.push('\n');
                }
                log(line.stream, &line.text);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return false,
            Err(mpsc::RecvTimeoutError::Timeout) => return true,
        }
    }
}

struct Line {
    stream: Stream,
    text: String,
}

fn read_lines<R: std::io::Read + Send + 'static>(
    reader: R,
    stream: Stream,
    sender: mpsc::Sender<Line>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut buffer = Vec::new();
        loop {
            buffer.clear();
            match reader.read_until(b'\n', &mut buffer) {
                Ok(0) => break,
                Ok(_) => {
                    let text = String::from_utf8_lossy(&buffer)
                        .trim_end_matches(['\r', '\n'])
                        .to_string();
                    if sender.send(Line { stream, text }).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    })
}

/// Builds `app/` for the RP2040 and packs the result into a UF2 image.
pub fn build<R: Runner + ?Sized>(
    config: &Config,
    runner: &R,
    cancel: &dyn Cancel,
    report: &mut dyn Reporter,
) -> Result<Image> {
    check(cancel)?;
    let manifest = config.app().join("Cargo.toml");
    if !manifest.is_file() {
        return Err(Error::invalid(format!(
            "韌體資料夾不完整（缺 app/Cargo.toml）：{}",
            config.firmware.display()
        )));
    }
    report.stage(1, "檢查 Rust 工具鏈");
    let cargo = cargo_path(config)?;
    if !config.skip_dependencies {
        match rustup_beside(&cargo) {
            Some(rustup) => {
                run(
                    runner,
                    &rustup,
                    vec!["target".into(), "add".into(), TARGET.into()],
                    Duration::from_secs(600),
                    cancel,
                    report,
                )?;
            }
            None => report.log(format!(
                "找不到 rustup，跳過 target 安裝（假設 {TARGET} 已安裝）"
            )),
        }
    }
    report.stage(2, "編譯 Rust 韌體");
    run(
        runner,
        &cargo,
        vec![
            "build".into(),
            "--release".into(),
            "--target".into(),
            TARGET.into(),
            "--manifest-path".into(),
            manifest.into_os_string(),
            "--features".into(),
            FEATURES.into(),
        ],
        Duration::from_secs(900),
        cancel,
        report,
    )?;

    let elf = config.elf();
    if !elf.is_file() {
        return Err(Error::invalid(format!(
            "編譯成功但找不到 ELF：{}",
            elf.display()
        )));
    }
    let bytes = std::fs::read(&elf)
        .map_err(|error| Error::io(format!("讀取 ELF 失敗：{}：{error}", elf.display())))?;
    let image = uf2::pack(&bytes)?;
    let blocks = uf2::blocks(&image);
    report.log(format!("UF2：{blocks} 個區塊"));
    Ok(Image {
        bytes: image,
        blocks,
    })
}

/// What the runtime board answered after the reboot.
#[derive(Clone, Debug)]
pub struct Verified {
    pub board: u8,
    pub firmware: String,
    pub lease_ms: u16,
}

impl fmt::Display for Verified {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "board={}, fw={}, lease={} ms",
            self.board, self.firmware, self.lease_ms
        )
    }
}

/// The two outcomes after a successful copy: the board came back and answered,
/// or it did not. Either way the image is on the flash.
pub enum Handshake {
    Verified(Verified),
    Unverified(Error),
}

/// Sends BOOTLOADER, copies the image onto the UF2 drive and waits for the board
/// to come back.
///
/// `Err` means the transfer itself failed; `Ok(Handshake::Unverified)` means the
/// copy finished but the board never answered — the distinction the caller
/// reports to the user.
pub fn push(
    config: &Config,
    device: Option<&Device>,
    image: &Image,
    cancel: &dyn Cancel,
    report: &mut dyn Reporter,
) -> Result<Handshake> {
    check(cancel)?;
    report.stage(3, "進入並選擇 bootloader");
    // Never auto-select a drive that was already mounted before this command:
    // it belongs to some other board.
    let previous = match (config.auto_bootloader, &config.boot_drive) {
        (true, None) if device.is_some() => drives()?
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
        _ => Vec::new(),
    };
    if config.auto_bootloader && config.boot_drive.is_none() {
        match device {
            Some(device) => {
                match device.bootloader() {
                    Ok(()) => report.log("已確認 enter_bootloader；等待裝置重啟".into()),
                    Err(error) => {
                        check(cancel)?;
                        report.log(format!(
                            "bootloader 命令未收到完整確認，繼續檢查列舉結果：{}",
                            error.message
                        ));
                    }
                }
                // The board refuses BOOTLOADER while a host keeps polling it, and
                // the runtime session has to be gone before the drive shows up.
                let _ = device.close();
            }
            None => report.log("未選取 runtime HID，請使用實體 BOOTSEL 進入下載模式".into()),
        }
    }
    let drive = match &config.boot_drive {
        Some(path) => path.clone(),
        None => wait_drive(cancel, report, &previous)?,
    };
    check(cancel)?;
    // Boards already in runtime here are unrelated to this transfer: excluding
    // them stops an existing board's hello from verifying another board.
    let existing: Vec<String> = Manager::list()?.into_iter().map(|e| e.path).collect();
    report.stage(4, "傳輸韌體");
    if !drive.join("INFO_UF2.TXT").is_file() {
        return Err(Error::invalid("指定位置不是 UF2 bootloader 磁碟"));
    }
    write_image(&drive.join("Firmware.uf2"), &image.bytes, cancel)?;
    report.log("UF2 複製完成，等待 runtime 驗證".into());
    report.stage(5, "等待 runtime HID 重新列舉與 hello 驗證");
    let serial = device
        .map(|device| device.info().serial)
        .filter(|serial| !serial.is_empty());
    Ok(
        match verify_runtime(serial.as_deref(), cancel, HANDSHAKE_WAIT, &existing) {
            Ok(verified) => Handshake::Verified(verified),
            Err(error) => Handshake::Unverified(error),
        },
    )
}

/// Writes the image to `target` in chunks so a cancelled copy stops early.
pub fn write_image(target: &Path, bytes: &[u8], cancel: &dyn Cancel) -> Result<()> {
    check(cancel)?;
    let mut output = std::fs::File::create(target).map_err(|error| {
        Error::new(
            ErrorCode::Io,
            format!("建立 UF2 目的檔失敗：{}：{error}", target.display()),
        )
    })?;
    for chunk in bytes.chunks(COPY_CHUNK) {
        check(cancel)?;
        output.write_all(chunk).map_err(|error| {
            Error::new(
                ErrorCode::Io,
                format!(
                    "UF2 複製失敗（保留 OS 錯誤碼）：{}：{error}",
                    target.display()
                ),
            )
        })?;
    }
    output
        .flush()
        .map_err(|error| Error::io(format!("UF2 重新整理失敗：{}：{error}", target.display())))?;
    Ok(())
}

/// Every mounted RP2040 bootloader drive.
pub fn drives() -> Result<Vec<PathBuf>> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::GetLogicalDrives;
        let mask = unsafe { GetLogicalDrives() };
        let mut drives = Vec::new();
        for bit in 0..26 {
            if mask & (1 << bit) != 0 {
                let root = PathBuf::from(format!("{}:\\", (b'A' + bit) as char));
                if let Ok(info) = std::fs::read_to_string(root.join("INFO_UF2.TXT"))
                    && (info.contains("RP2040") || info.contains("RPI-RP2"))
                {
                    drives.push(root);
                }
            }
        }
        Ok(drives)
    }
    #[cfg(not(windows))]
    {
        Err(Error::unsupported("UF2 磁碟偵測僅支援 Windows"))
    }
}

fn wait_drive(
    cancel: &dyn Cancel,
    report: &mut dyn Reporter,
    previous: &[String],
) -> Result<PathBuf> {
    let deadline = Instant::now() + DRIVE_WAIT;
    while Instant::now() < deadline {
        check(cancel)?;
        let drives: Vec<PathBuf> = drives()?
            .into_iter()
            .filter(|path| !previous.contains(&path.display().to_string()))
            .collect();
        match drives.len() {
            0 => {}
            1 => return Ok(drives.into_iter().next().expect("checked len")),
            _ => {
                let chosen = report.choose_drive(
                    drives
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect(),
                )?;
                return Ok(PathBuf::from(chosen));
            }
        }
        pause(cancel)?;
    }
    Err(Error::timeout(
        "15 秒內未偵測到 RP2040 UF2 磁碟；請 BOOTSEL 上電後重試",
    ))
}

fn verify_runtime(
    serial: Option<&str>,
    cancel: &dyn Cancel,
    timeout: Duration,
    existing: &[String],
) -> Result<Verified> {
    wait_for_board(
        cancel,
        timeout,
        serial,
        || Ok(runtime_candidates(Manager::list()?, existing)),
        open_and_check,
    )
}

fn open_and_check(entry: &DeviceEntry) -> Result<Verified> {
    let device = Manager::open(
        Some(&entry.path),
        DeviceOptions {
            auto_reconnect: false,
            timeout_ms: 1000,
            heartbeat: false,
        },
    )?;
    let info = device.info();
    let result = check_board(info.board).map(|()| Verified {
        board: info.board,
        firmware: info.firmware.clone(),
        lease_ms: info.lease_ms,
    });
    let _ = device.close();
    result
}

fn check_board(reported: u8) -> Result<()> {
    if reported == BOARD_ID {
        return Ok(());
    }
    Err(Error::invalid(format!(
        "重新列舉的板型不符：{reported}（預期 {BOARD_ID}）"
    )))
}

/// Devices that appeared after the copy: an already-running board cannot stand
/// in for the one that was just flashed.
fn runtime_candidates(devices: Vec<DeviceEntry>, existing: &[String]) -> Vec<DeviceEntry> {
    devices
        .into_iter()
        .filter(|entry| !existing.contains(&entry.path))
        .collect()
}

fn wait_for_board<E, O>(
    cancel: &dyn Cancel,
    timeout: Duration,
    serial: Option<&str>,
    mut enumerate: E,
    mut open: O,
) -> Result<Verified>
where
    E: FnMut() -> Result<Vec<DeviceEntry>>,
    O: FnMut(&DeviceEntry) -> Result<Verified>,
{
    let deadline = Instant::now() + timeout;
    let mut last = String::from("尚未列舉");
    while Instant::now() < deadline {
        check(cancel)?;
        let matched: Vec<DeviceEntry> = enumerate()?
            .into_iter()
            .filter(|entry| serial.is_none_or(|serial| entry.serial == serial))
            .collect();
        // Duplicate firmware serials cannot prove identity: do not pick the first.
        match matched.len() {
            1 => match open(&matched[0]) {
                Ok(verified) => return Ok(verified),
                Err(error) => last = error.message,
            },
            0 => {}
            _ => last = "多個符合裝置，無法確認燒錄目標；請僅連接目標板後驗證".into(),
        }
        pause(cancel)?;
    }
    Err(Error::timeout(format!("runtime hello 驗證逾時：{last}")))
}

/// `cargo`, preferring an explicit setting.
fn cargo_path(config: &Config) -> Result<PathBuf> {
    if let Some(path) = &config.cargo {
        if !path.is_file() {
            return Err(Error::new(
                ErrorCode::Invalid,
                format!("指定的 cargo 不存在：{}", path.display()),
            ));
        }
        return Ok(path.clone());
    }
    crate::platform::locate_program(CARGO).ok_or_else(|| {
        Error::new(
            ErrorCode::Disconnected,
            "找不到 cargo；請安裝 Rust 工具鏈，或在設定指定 cargo 路徑",
        )
    })
}

#[cfg(windows)]
const CARGO: &str = "cargo.exe";
#[cfg(not(windows))]
const CARGO: &str = "cargo";

/// rustup normally sits beside cargo; without it the target cannot be added.
fn rustup_beside(cargo: &Path) -> Option<PathBuf> {
    #[cfg(windows)]
    let name = "rustup.exe";
    #[cfg(not(windows))]
    let name = "rustup";
    let candidate = cargo.parent()?.join(name);
    candidate.is_file().then_some(candidate)
}

fn run<R: Runner + ?Sized>(
    runner: &R,
    program: &Path,
    args: Vec<OsString>,
    timeout: Duration,
    cancel: &dyn Cancel,
    report: &mut dyn Reporter,
) -> Result<String> {
    check(cancel)?;
    report.log(format!(
        "{} {}",
        program.display(),
        args.iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    ));
    let outcome = runner.run(program, &args, timeout, cancel, &mut |stream, text| {
        report.log(format!("{stream:?}: {text}"));
    })?;
    check(cancel)?;
    outcome.check()
}

fn check(cancel: &dyn Cancel) -> Result<()> {
    if cancel.cancelled() {
        return Err(Error::new(ErrorCode::Cancelled, "操作已取消"));
    }
    Ok(())
}

/// Sleeps in slices so cancellation is honoured inside a wait.
fn pause(cancel: &dyn Cancel) -> Result<()> {
    let end = Instant::now() + POLL;
    while Instant::now() < end {
        check(cancel)?;
        std::thread::sleep(
            end.saturating_duration_since(Instant::now())
                .min(Duration::from_millis(10)),
        );
    }
    check(cancel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct Temp(PathBuf);

    impl Temp {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "gdb-flash-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[derive(Default)]
    struct Token(AtomicBool);

    impl Cancel for Token {
        fn cancelled(&self) -> bool {
            self.0.load(Ordering::Acquire)
        }
    }

    impl Token {
        fn cancel(&self) {
            self.0.store(true, Ordering::Release);
        }
    }

    #[derive(Default)]
    struct Quiet;

    impl Reporter for Quiet {
        fn log(&mut self, _: String) {}
        fn stage(&mut self, _: u8, _: &str) {}
    }

    struct FakeRunner {
        fail_on: String,
        calls: std::sync::Mutex<Vec<Vec<String>>>,
    }

    impl Runner for FakeRunner {
        fn run(
            &self,
            _: &Path,
            args: &[OsString],
            _: Duration,
            _: &dyn Cancel,
            _: &mut dyn FnMut(Stream, &str),
        ) -> Result<Outcome> {
            let joined: Vec<String> = args.iter().map(|a| a.to_string_lossy().into()).collect();
            let failed = joined.join(" ").contains(&self.fail_on);
            self.calls.lock().unwrap().push(joined);
            Ok(Outcome {
                exit: Some(if failed { 9 } else { 0 }),
                stderr: if failed {
                    "injected failure".into()
                } else {
                    String::new()
                },
                ..Outcome::default()
            })
        }
    }

    fn entry(path: &str, serial: &str) -> DeviceEntry {
        DeviceEntry {
            path: path.into(),
            serial: serial.into(),
            product: "fake".into(),
            manufacturer: String::new(),
            vid: crate::VENDOR_ID,
            pid: crate::PRODUCT_ID,
        }
    }

    #[test]
    fn flashed_capability_must_be_the_xiao_board_id() {
        assert!(check_board(BOARD_ID).is_ok());
        // board=1 (Pico) and board=3 (AVR) builds must not verify as a XIAO.
        assert!(check_board(1).is_err());
        assert!(check_board(3).is_err());
    }

    #[test]
    fn the_build_is_pinned_to_one_crate_and_target() {
        let config = Config::default();
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
    fn a_failing_build_is_reported_and_only_the_build_carries_features() {
        let root = Temp::new();
        let config = Config {
            cargo: Some(std::env::current_exe().unwrap()),
            firmware: root.0.join("firmware"),
            ..Config::default()
        };
        std::fs::create_dir_all(config.app()).unwrap();
        std::fs::write(config.app().join("Cargo.toml"), "").unwrap();
        let runner = FakeRunner {
            fail_on: "--features".into(),
            calls: std::sync::Mutex::new(Vec::new()),
        };
        let error = build(&config, &runner, &Token::default(), &mut Quiet).unwrap_err();
        assert!(error.message.contains("子程序失敗"), "{}", error.message);
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
        let root = Temp::new();
        let config = Config {
            cargo: Some(root.0.join("no-cargo.exe")),
            firmware: root.0.join("firmware"),
            ..Config::default()
        };
        std::fs::create_dir_all(config.app()).unwrap();
        std::fs::write(config.app().join("Cargo.toml"), "").unwrap();
        let error = build(
            &config,
            &FakeRunner {
                fail_on: String::new(),
                calls: std::sync::Mutex::new(Vec::new()),
            },
            &Token::default(),
            &mut Quiet,
        )
        .unwrap_err();
        assert!(
            error.message.contains("不存在") || error.message.contains("找不到"),
            "{}",
            error.message
        );
    }

    #[test]
    fn a_missing_firmware_folder_is_reported_before_building() {
        let root = Temp::new();
        let config = Config {
            cargo: Some(std::env::current_exe().unwrap()),
            firmware: root.0.join("nowhere"),
            ..Config::default()
        };
        let error = build(
            &config,
            &FakeRunner {
                fail_on: String::new(),
                calls: std::sync::Mutex::new(Vec::new()),
            },
            &Token::default(),
            &mut Quiet,
        )
        .unwrap_err();
        assert!(
            error.message.contains("app/Cargo.toml"),
            "{}",
            error.message
        );
    }

    #[test]
    fn an_image_write_keeps_the_os_error_and_cancel_does_not_report_success() {
        let root = Temp::new();
        let error = write_image(
            &root.0.join("missing/output.uf2"),
            &[1, 2, 3, 4],
            &Token::default(),
        )
        .unwrap_err();
        assert!(error.message.contains("UF2"), "{}", error.message);
        assert!(
            error.message.contains("os error"),
            "the OS error must survive: {}",
            error.message
        );
        let target = root.0.join("output.uf2");
        let token = Token::default();
        token.cancel();
        assert!(write_image(&target, &[1, 2, 3, 4], &token).is_err());
        assert!(!target.exists(), "a cancelled copy must not look finished");
    }

    #[test]
    fn runtime_reenumeration_retries_the_handshake_and_rejects_ambiguity() {
        let mut tries = 0;
        let result = wait_for_board(
            &Token::default(),
            Duration::from_secs(1),
            Some("serial"),
            || Ok(vec![entry("new", "serial")]),
            |_| {
                tries += 1;
                if tries == 1 {
                    Err(Error::timeout("delayed handshake"))
                } else {
                    Ok(Verified {
                        board: BOARD_ID,
                        firmware: "3.2.0".into(),
                        lease_ms: 2000,
                    })
                }
            },
        )
        .unwrap();
        assert_eq!(result.board, BOARD_ID);
        assert_eq!(tries, 2);

        let result = wait_for_board(
            &Token::default(),
            Duration::from_millis(20),
            None,
            || Ok(vec![entry("a", "shared"), entry("b", "shared")]),
            |_| panic!("must not select the first device"),
        );
        assert!(result.is_err());
    }

    #[test]
    fn an_existing_runtime_board_cannot_verify_another_boards_transfer() {
        let existing = entry("already-running", "shared-firmware-serial");
        let returning = entry("just-reenumerated", "shared-firmware-serial");
        let candidates = runtime_candidates(
            vec![existing.clone(), returning.clone()],
            std::slice::from_ref(&existing.path),
        );
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].path, returning.path);
    }

    #[test]
    fn a_handshake_timeout_reports_instead_of_passing() {
        let start = Instant::now();
        let error = wait_for_board(
            &Token::default(),
            Duration::from_millis(20),
            None,
            || Ok(Vec::new()),
            |_| panic!("nothing to open"),
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::Timeout);
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}
