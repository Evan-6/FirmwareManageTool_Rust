//! Firmware upload for the window.
//!
//! The build, the BOOTSEL hand-over, the copy and the re-enumeration handshake
//! live in the SDK (`gdb::flash`); this module owns what belongs to the tool:
//! the persisted configuration shape, the observer that streams stages and log
//! lines to the window, and the UF2 copy kept beside the settings so the image
//! can also be flashed by hand.
//!
//! Nothing here needs the Arduino toolchain. The only external requirement is a
//! Rust toolchain with the `thumbv6m-none-eabi` target; `skip_dependencies`
//! avoids even the `rustup target add` when the machine already has it.

use crate::{
    cancel::Cancellation,
    hid::{self, Device},
    platform,
    process::{ProcessRunner, ProcessSpec},
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::Duration,
};

pub use gdb::flash::{APP_CRATE, BOARD_ID, FEATURES, TARGET};

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
    /// The SDK's view of the same settings: an empty override means "look it up".
    fn to_flash(&self) -> gdb::flash::Config {
        let path = |text: &str| (!text.trim().is_empty()).then_some(PathBuf::from(text.trim()));
        gdb::flash::Config {
            cargo: path(&self.cargo_path),
            firmware: self.sources(),
            skip_dependencies: self.skip_dependencies,
            auto_bootloader: self.auto_bootloader,
            boot_drive: path(&self.boot_target),
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

/// The worker's token is what the SDK polls; there is no second flag.
impl gdb::flash::Cancel for Cancellation {
    fn cancelled(&self) -> bool {
        self.is_cancelled()
    }
}

/// Streams SDK progress into the window, including the drive picker.
struct Progress<'a> {
    observer: &'a mut dyn UploadObserver,
    cancel: &'a Cancellation,
}

impl gdb::flash::Reporter for Progress<'_> {
    fn log(&mut self, line: String) {
        self.observer.log(line);
    }

    fn stage(&mut self, stage: u8, message: &str) {
        self.observer.progress(stage.into(), message);
    }

    fn choose_drive(&mut self, candidates: Vec<String>) -> gdb::Result<String> {
        self.observer
            .choose_target(candidates, self.cancel)
            .map_err(|error| gdb::Error::new(gdb::ErrorCode::Invalid, format!("{error:#}")))
    }
}

/// Bridges the tool's process runner — a whole process tree, no console window —
/// into the SDK's runner seam. The tool's runner takes the concrete
/// `Cancellation`, so the bridge carries the token it was built with; it is the
/// same token the SDK passes in, which is why that argument is unused here.
struct Runner<'a, R: ProcessRunner> {
    runner: &'a R,
    cancel: &'a Cancellation,
}

impl<R: ProcessRunner> gdb::flash::Runner for Runner<'_, R> {
    fn run(
        &self,
        program: &Path,
        args: &[OsString],
        timeout: Duration,
        _: &dyn gdb::flash::Cancel,
        log: &mut dyn FnMut(gdb::flash::Stream, &str),
    ) -> gdb::Result<gdb::flash::Outcome> {
        let result = self
            .runner
            .run(
                &ProcessSpec {
                    program: program.to_path_buf(),
                    args: args.to_vec(),
                    timeout,
                },
                self.cancel,
                &mut |line: crate::process::ProcessLine| {
                    let stream = match line.stream {
                        crate::process::Stream::Stdout => gdb::flash::Stream::Stdout,
                        crate::process::Stream::Stderr => gdb::flash::Stream::Stderr,
                    };
                    log(stream, &line.text);
                },
            )
            .map_err(|error| gdb::Error::new(gdb::ErrorCode::Io, format!("{error:#}")))?;
        Ok(gdb::flash::Outcome {
            stdout: result.stdout,
            stderr: result.stderr,
            exit: result.exit_code,
            timed_out: result.timed_out,
            cancelled: result.cancelled,
        })
    }
}

pub fn upload<R: ProcessRunner>(
    config: &UploadConfig,
    selected: Option<&Device>,
    runner: &R,
    cancel: &Cancellation,
    observer: &mut dyn UploadObserver,
) -> UploadOutcome {
    let sdk = config.to_flash();
    let mut progress = Progress { observer, cancel };
    let bridge = Runner { runner, cancel };
    let image = match gdb::flash::build(&sdk, &bridge, cancel, &mut progress) {
        Ok(image) => image,
        Err(error) => return UploadOutcome::TransferFailed(error.message),
    };
    if let Err(error) = keep_copy(&image) {
        return UploadOutcome::TransferFailed(format!("{error:#}"));
    }
    // The SDK closes this handle before it waits for the drive: the board refuses
    // BOOTLOADER while another host keeps polling it.
    let device = match selected {
        Some(device) => match hid::open_sdk(device) {
            Ok(device) => Some(device),
            Err(error) => return UploadOutcome::TransferFailed(format!("{error:#}")),
        },
        None => None,
    };
    match gdb::flash::push(&sdk, device.as_ref(), &image, cancel, &mut progress) {
        Err(error) => UploadOutcome::TransferFailed(error.message),
        Ok(gdb::flash::Handshake::Verified(verified)) => {
            UploadOutcome::Verified(verified.to_string())
        }
        Ok(gdb::flash::Handshake::Unverified(error)) => {
            UploadOutcome::TransferredUnverified(error.message)
        }
    }
}

/// Keeps the packed image next to the settings, so a failed transfer can still be
/// finished by dragging one file onto the drive.
fn keep_copy(image: &gdb::flash::Image) -> Result<()> {
    let root = platform::application_dir()?;
    std::fs::create_dir_all(&root)?;
    let out = root.join("Firmware.uf2");
    std::fs::write(&out, &image.bytes)
        .with_context(|| format!("寫入 UF2 失敗：{}", out.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn empty_overrides_mean_look_them_up_and_settings_are_trimmed() {
        let sdk = UploadConfig::default().to_flash();
        assert!(sdk.cargo.is_none());
        assert!(sdk.boot_drive.is_none());
        assert!(sdk.firmware.ends_with("firmware"));
        let sdk = UploadConfig {
            cargo_path: " C:\\tools\\cargo.exe ".into(),
            boot_target: " E:\\ ".into(),
            skip_dependencies: true,
            auto_bootloader: false,
            ..UploadConfig::default()
        }
        .to_flash();
        assert_eq!(sdk.cargo, Some(PathBuf::from("C:\\tools\\cargo.exe")));
        assert_eq!(sdk.boot_drive, Some(PathBuf::from("E:\\")));
        assert!(sdk.skip_dependencies);
        assert!(!sdk.auto_bootloader);
    }
}
