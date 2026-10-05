use crate::{
    cancel::Cancellation,
    hid::{self, Device},
    platform,
    process::{ProcessLine, ProcessRunner, ProcessSpec},
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Board {
    Leonardo,
    #[default]
    XiaoRp2040,
    Pico,
    CustomRp2040,
}
impl Board {
    pub fn label(self) -> &'static str {
        match self {
            Self::Leonardo => "Leonardo / AVR",
            Self::XiaoRp2040 => "Seeed XIAO RP2040",
            Self::Pico => "Raspberry Pi Pico",
            Self::CustomRp2040 => "其他 RP2040",
        }
    }
    pub fn is_avr(self) -> bool {
        self == Self::Leonardo
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UploadConfig {
    pub board: Board,
    pub custom_board: String,
    pub custom_neopixel: bool,
    pub cli_path: String,
    pub firmware_path: String,
    pub skip_dependencies: bool,
    pub auto_bootloader: bool,
    pub boot_target: String,
}
impl Default for UploadConfig {
    fn default() -> Self {
        Self {
            board: Board::default(),
            custom_board: "rpipico".into(),
            custom_neopixel: false,
            cli_path: String::new(),
            firmware_path: String::new(),
            skip_dependencies: false,
            auto_bootloader: true,
            boot_target: String::new(),
        }
    }
}
impl UploadConfig {
    pub fn fqbn(&self) -> Result<String> {
        if self.board.is_avr() {
            return Ok("goosedevil:avr:keyboard".into());
        }
        let id = match self.board {
            Board::XiaoRp2040 => "seeed_xiao_rp2040",
            Board::Pico => "rpipico",
            _ => self.custom_board.trim(),
        };
        ensure!(
            !id.is_empty() && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_'),
            "板卡代號僅允許英數與底線"
        );
        Ok(format!("rp2040:rp2040:{id}:usbstack=tinyusb"))
    }
    pub fn needs_neopixel(&self) -> bool {
        self.board == Board::XiaoRp2040
            || (self.board == Board::CustomRp2040 && self.custom_neopixel)
    }
    pub fn sources(&self) -> PathBuf {
        if self.firmware_path.trim().is_empty() {
            platform::firmware_dir()
        } else {
            PathBuf::from(&self.firmware_path)
        }
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
struct Cli<'a, R: ProcessRunner> {
    runner: &'a R,
    program: PathBuf,
    config: PathBuf,
}
impl<R: ProcessRunner> Cli<'_, R> {
    fn run(
        &self,
        args: Vec<OsString>,
        timeout: Duration,
        cancel: &Cancellation,
        observer: &mut dyn UploadObserver,
    ) -> Result<String> {
        let mut full = vec!["--config-file".into(), self.config.as_os_str().into()];
        full.extend(args);
        observer.log(format!(
            "arduino-cli {}",
            full.iter()
                .map(|s| s.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ")
        ));
        let result = self.runner.run(
            &ProcessSpec {
                program: self.program.clone(),
                args: full,
                timeout,
            },
            cancel,
            &mut |line: ProcessLine| observer.log(format!("{:?}: {}", line.stream, line.text)),
        )?;
        result.check()?;
        Ok(result.stdout)
    }
}
fn yaml_string(path: &Path) -> Result<String> {
    let path = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("路徑不是有效 Unicode"))?;
    Ok(serde_json::to_string(path)?) // JSON quoted strings are valid YAML scalars.
}
pub fn write_cli_config(path: &Path, data: &Path, user: &Path) -> Result<()> {
    let downloads = data.join("staging");
    std::fs::create_dir_all(&downloads)?;
    std::fs::write(
        path,
        format!(
            "directories:\n  data: {}\n  downloads: {}\n  user: {}\nboard_manager:\n  additional_urls:\n    - https://github.com/earlephilhower/arduino-pico/releases/download/global/package_rp2040_index.json\n",
            yaml_string(data)?,
            yaml_string(&downloads)?,
            yaml_string(user)?
        ),
    )?;
    Ok(())
}
fn ensure_cli<R: ProcessRunner>(
    config: &UploadConfig,
    runner: &R,
    cancel: &Cancellation,
    observer: &mut dyn UploadObserver,
) -> Result<PathBuf> {
    if !config.cli_path.trim().is_empty() {
        let path = PathBuf::from(config.cli_path.trim());
        ensure!(path.is_file(), "指定的 arduino-cli 不存在");
        return Ok(path);
    }
    if let Some(path) = platform::locate_program("arduino-cli.exe") {
        return Ok(path);
    }
    let winget = platform::locate_program("winget.exe").ok_or_else(|| {
        anyhow::anyhow!(
            "找不到 arduino-cli 與 winget，請安裝 App Installer 或指定 arduino-cli 路徑"
        )
    })?;
    observer.log("缺少 arduino-cli，正在透過 winget 自動安裝…".into());
    let args = [
        "install",
        "--id",
        "ArduinoSA.CLI",
        "--exact",
        "--silent",
        "--accept-source-agreements",
        "--accept-package-agreements",
        "--disable-interactivity",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    runner
        .run(
            &ProcessSpec {
                program: winget,
                args,
                timeout: Duration::from_secs(900),
            },
            cancel,
            &mut |line| observer.log(line.text),
        )?
        .check()?;
    platform::locate_program("arduino-cli.exe")
        .ok_or_else(|| anyhow::anyhow!("安裝完成但找不到 arduino-cli；請在設定指定執行檔路徑"))
}
fn args(items: &[&str]) -> Vec<OsString> {
    items.iter().map(OsString::from).collect()
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
    let fqbn = config.fqbn()?;
    let sources = config.sources();
    let name = if config.board.is_avr() {
        "leonardo_avr"
    } else {
        "rp2040"
    };
    let sketch = sources.join("boards").join(name);
    ensure!(
        sketch.join(format!("{name}.ino")).is_file() && sketch.join("Firmware.cpp").is_file(),
        "韌體資料夾不完整：{}",
        sketch.display()
    );
    if config.board.is_avr() {
        ensure!(
            sketch.join("hardware/goosedevil/avr/boards.txt").is_file(),
            "缺少 Leonardo 自訂 board package"
        );
    }
    observer.progress(1, "檢查工具與安裝依賴");
    let program = ensure_cli(config, runner, cancel, observer)?;
    std::fs::create_dir_all(root)?;
    let working = root.join("sketches").join(name);
    copy_tree(&sketch, &working)?;
    let cli_config = root.join(format!("arduino-{name}.yaml"));
    write_cli_config(&cli_config, &root.join("ArduinoData"), &working)?;
    let cli = Cli {
        runner,
        program,
        config: cli_config,
    };
    let install_limit = Duration::from_secs(900);
    if !config.skip_dependencies {
        cli.run(
            args(&["core", "update-index"]),
            install_limit,
            cancel,
            observer,
        )?;
        cli.run(
            args(&[
                "core",
                "install",
                if config.board.is_avr() {
                    "arduino:avr"
                } else {
                    "rp2040:rp2040"
                },
            ]),
            install_limit,
            cancel,
            observer,
        )?;
        if config.needs_neopixel() && !working.join("libraries/Adafruit_NeoPixel").is_dir() {
            cli.run(
                args(&["lib", "install", "Adafruit NeoPixel"]),
                install_limit,
                cancel,
                observer,
            )?;
        }
    }
    observer.progress(2, "編譯韌體");
    let build = root.join("build").join(name);
    std::fs::create_dir_all(&build)?;
    let mut compile = args(&["compile", "--fqbn", &fqbn, "--clean", "--build-path"]);
    compile.push(build.as_os_str().into());
    compile.push(working.as_os_str().into());
    cli.run(compile, Duration::from_secs(600), cancel, observer)?;
    // Resolve the exact sketch artifact before asking the device to reboot.
    let uf2 = if config.board.is_avr() {
        None
    } else {
        Some(find_uf2(&build, name)?)
    };
    observer.progress(3, "進入並選擇 bootloader");
    let explicit = !config.boot_target.trim().is_empty();
    // If rebooting a selected runtime device, never auto-select another board that
    // was already in bootloader before that command.
    let previous_targets = if config.auto_bootloader && !explicit && selected.is_some() {
        if config.board.is_avr() {
            let json = cli.run(
                args(&["board", "list", "--format", "json"]),
                Duration::from_secs(2),
                cancel,
                observer,
            )?;
            parse_ports(&json)?
                .into_iter()
                .map(|p| p.address)
                .collect::<Vec<_>>()
        } else {
            platform::boot_drives()?
                .iter()
                .map(|p| p.display().to_string())
                .collect()
        }
    } else {
        Vec::new()
    };
    if config.auto_bootloader && !explicit {
        if let Some(device) = selected {
            let mut session = hid::open(device)?;
            session.synchronize(cancel)?;
            session.hello(cancel)?;
            match session.command("enter_bootloader", "ok:enter_bootloader", cancel) {
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
            observer.log("未選取 runtime HID，請使用實體 reset / BOOTSEL 進入下載模式".into());
        }
    }
    let target = if explicit {
        config.boot_target.trim().to_owned()
    } else if config.board.is_avr() {
        wait_ports(&cli, cancel, observer, &previous_targets)?
    } else {
        wait_drives(cancel, observer, &previous_targets)?
    };
    cancel.check()?;
    // Devices already in runtime at this point are unrelated to this bootloader transfer.
    // Excluding them prevents an existing board's hello from falsely verifying another board.
    let existing_paths = hid::enumerate()?.into_iter().map(|d| d.path).collect();
    observer.progress(4, "傳輸韌體");
    if config.board.is_avr() {
        ensure!(
            target.to_ascii_uppercase().starts_with("COM")
                && target[3..].chars().all(|c| c.is_ascii_digit())
                && target.len() > 3,
            "請選擇 bootloader COM 埠"
        );
        let mut upload = args(&[
            "upload",
            "-p",
            &target,
            "--fqbn",
            &fqbn,
            "--verify",
            "--upload-property",
            "upload.use_1200bps_touch=false",
            "--upload-property",
            "upload.wait_for_upload_port=false",
            "--build-path",
        ]);
        upload.push(build.as_os_str().into());
        upload.push(working.as_os_str().into());
        cli.run(upload, Duration::from_secs(120), cancel, observer)?;
    } else {
        let root = PathBuf::from(&target);
        ensure!(
            root.join("INFO_UF2.TXT").is_file(),
            "指定位置不是 UF2 bootloader 磁碟"
        );
        copy_uf2(uf2.as_ref().unwrap(), &root.join("Firmware.uf2"), cancel)?;
        observer.log("UF2 複製完成，等待 runtime 驗證".into());
    }
    Ok(existing_paths)
}
fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let path = entry.path();
        let dest = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&path, &dest)?;
        } else if entry.file_type()?.is_file() {
            std::fs::copy(path, dest)?;
        }
    }
    Ok(())
}
pub fn find_uf2(build: &Path, sketch: &str) -> Result<PathBuf> {
    let artifact = build.join(format!("{sketch}.ino.uf2"));
    ensure!(
        artifact.is_file(),
        "找不到本次 sketch 的 UF2：{}",
        artifact.display()
    );
    Ok(artifact)
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootPort {
    pub address: String,
    pub recognized: bool,
}
pub fn parse_ports(json: &str) -> Result<Vec<BootPort>> {
    let value: serde_json::Value = serde_json::from_str(json)?;
    let entries = value
        .as_array()
        .or_else(|| value.get("detected_ports").and_then(|v| v.as_array()))
        .ok_or_else(|| anyhow::anyhow!("board list JSON 沒有 detected_ports"))?;
    let mut result = Vec::new();
    for entry in entries {
        let port = entry.get("port").unwrap_or(entry);
        let Some(address) = port.get("address").and_then(|v| v.as_str()) else {
            continue;
        };
        if !address.to_ascii_uppercase().starts_with("COM") {
            continue;
        }
        let recognized = [entry, port].iter().any(|object| {
            let boards_match = ["matching_boards", "boards"].iter().any(|key| {
                object
                    .get(key)
                    .and_then(|v| v.as_array())
                    .is_some_and(|boards| {
                        boards.iter().any(|b| {
                            ["name", "fqbn"].iter().any(|field| {
                                b.get(field).and_then(|v| v.as_str()).is_some_and(|s| {
                                    let s = s.to_ascii_lowercase();
                                    s.contains("leonardo") || s.contains("arduino micro")
                                })
                            })
                        })
                    })
            });
            let label_match = object
                .get("label")
                .and_then(|v| v.as_str())
                .is_some_and(|s| s.to_ascii_lowercase().contains("leonardo"));
            boards_match || label_match
        });
        result.push(BootPort {
            address: address.into(),
            recognized,
        });
    }
    result.sort_by(|a, b| a.address.cmp(&b.address));
    result.dedup_by(|a, b| a.address == b.address);
    Ok(result)
}
fn select_ports(
    ports: &[BootPort],
    cancel: &Cancellation,
    observer: &mut dyn UploadObserver,
) -> Result<Option<String>> {
    let recognized: Vec<_> = ports.iter().filter(|p| p.recognized).collect();
    if recognized.len() == 1 {
        return Ok(Some(recognized[0].address.clone()));
    }
    if ports.is_empty() {
        return Ok(None);
    }
    let candidates = if recognized.is_empty() {
        ports.iter().collect::<Vec<_>>()
    } else {
        recognized
    };
    Ok(Some(observer.choose_target(
        candidates.iter().map(|p| p.address.clone()).collect(),
        cancel,
    )?))
}
fn wait_ports<R: ProcessRunner>(
    cli: &Cli<'_, R>,
    cancel: &Cancellation,
    observer: &mut dyn UploadObserver,
    previous_targets: &[String],
) -> Result<String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        cancel.check()?;
        let json = cli.run(
            args(&["board", "list", "--format", "json"]),
            remaining.min(Duration::from_secs(2)),
            cancel,
            observer,
        )?;
        let ports = parse_ports(&json)?
            .into_iter()
            .filter(|p| !previous_targets.contains(&p.address))
            .collect::<Vec<_>>();
        if let Some(port) = select_ports(&ports, cancel, observer)? {
            return Ok(port);
        }
        cancel.sleep(Duration::from_millis(250))?;
    }
    bail!("10 秒內未偵測到 bootloader；請實體 reset，並指定 bootloader COM 後重試")
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
            let hello = session.hello(token)?;
            Ok(hello.raw)
        },
    )
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
    fn board_variants() {
        let mut config = UploadConfig::default();
        assert!(config.fqbn().unwrap().contains("seeed_xiao_rp2040"));
        assert!(config.needs_neopixel());
        config.board = Board::Pico;
        assert!(!config.needs_neopixel());
        config.board = Board::CustomRp2040;
        config.custom_board = "rpipico:usb=bad".into();
        assert!(config.fqbn().is_err());
    }
    #[test]
    fn multiple_and_old_json_ports() {
        let ports = parse_ports(r#"{"detected_ports":[{"port":{"address":"COM3"},"matching_boards":[{"name":"Arduino Leonardo"}]},{"port":{"address":"COM5"},"matching_boards":[{"fqbn":"arduino:avr:leonardo"}]}]}"#).unwrap();
        assert_eq!(ports.len(), 2);
        assert!(ports.iter().all(|p| p.recognized));
        assert_eq!(
            parse_ports(r#"[{"address":"COM7","boards":[{"name":"Arduino Leonardo"}]}]"#).unwrap()
                [0]
            .address,
            "COM7"
        );
    }
    #[test]
    fn unicode_and_spaces_in_yaml() {
        assert_eq!(
            yaml_string(Path::new("C:\\中文 空白\\firmware")).unwrap(),
            r#""C:\\中文 空白\\firmware""#
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
    fn dependency_and_compile_failures_stop_before_reboot_or_upload() {
        for failure in ["core install", "compile"] {
            let root = crate::test_support::Temp::new();
            let config = UploadConfig {
                board: Board::Leonardo,
                cli_path: std::env::current_exe().unwrap().display().to_string(),
                boot_target: "COM3".into(),
                ..Default::default()
            };
            let runner = FakeRunner {
                fail_on: failure.into(),
                calls: std::sync::Mutex::new(Vec::new()),
            };
            let result = transfer_at(
                &config,
                None,
                &runner,
                &Cancellation::default(),
                &mut FakeObserver::default(),
                &root.0,
            );
            assert!(result.is_err());
            let calls = runner.calls.lock().unwrap();
            assert!(!calls.iter().any(|args| args.iter().any(|a| a == "upload")));
            if failure == "core install" {
                assert!(!calls.iter().any(|args| args.iter().any(|a| a == "compile")));
            }
        }
    }
    #[test]
    fn multiple_or_unrecognized_bootloaders_require_selection() {
        let token = Cancellation::default();
        let mut observer = FakeObserver::default();
        let ports = vec![
            BootPort {
                address: "COM3".into(),
                recognized: true,
            },
            BootPort {
                address: "COM5".into(),
                recognized: true,
            },
        ];
        assert_eq!(
            select_ports(&ports, &token, &mut observer).unwrap(),
            Some("COM5".into())
        );
        assert_eq!(observer.choices, 1);
        let ports = vec![BootPort {
            address: "COM9".into(),
            recognized: false,
        }];
        select_ports(&ports, &token, &mut observer).unwrap();
        assert_eq!(observer.choices, 2);
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
                Ok("ok:hello,protocol=2".into())
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
