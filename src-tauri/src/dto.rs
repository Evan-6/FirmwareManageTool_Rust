//! Typed payloads for the webview. The egui build read these structs directly;
//! the React UI gets the same values through `serde`, field names in camelCase.

use firmware_manage_tool::{
    backend::AppEvent,
    firmware::UploadOutcome,
    hid::Device,
    latency::{Report, Round},
    protocol::{Hello, Status},
};
use serde::Serialize;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DeviceDto {
    /// Position in the last `scan`, so the UI can select by list row.
    pub index: Option<usize>,
    pub product: String,
    pub serial: String,
    pub vid: u16,
    pub pid: u16,
    pub label: String,
}

impl DeviceDto {
    pub fn new(index: Option<usize>, device: &Device) -> Self {
        Self {
            index,
            product: device.product.clone(),
            serial: device.serial.clone(),
            vid: device.vid,
            pid: device.pid,
            label: device.label(),
        }
    }
}

/// STATUS with the stringly-typed field map resolved once, so the UI can plot
/// numbers instead of parsing them again per frame.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StatusDto {
    pub received: u32,
    pub completed: u32,
    pub pending: u8,
    pub lease_ms: u16,
    pub rx: u16,
    pub tx: u16,
    pub hid: u16,
    pub failsafe: u16,
    pub held: usize,
    pub buttons: u8,
    pub keys: Vec<String>,
    pub raw: String,
}

impl StatusDto {
    pub fn new(status: &Status) -> Self {
        let keys = status
            .fields
            .get("keys")
            .filter(|keys| !keys.is_empty())
            .map(|keys| keys.split(',').map(|k| k.trim().to_string()).collect())
            .unwrap_or_default();
        Self {
            received: status.count("received") as u32,
            completed: status.count("completed") as u32,
            pending: status.count("pending") as u8,
            lease_ms: status.count("lease") as u16,
            rx: status.count("rx") as u16,
            tx: status.count("tx") as u16,
            hid: status.count("hid") as u16,
            failsafe: status.count("fs") as u16,
            held: status.count("p") as usize,
            buttons: status.buttons(),
            keys,
            raw: status.raw.clone(),
        }
    }
}

#[derive(Serialize, Clone)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum UiEvent {
    Devices {
        devices: Vec<DeviceDto>,
    },
    Connected {
        device: DeviceDto,
        hello: Hello,
    },
    Disconnected,
    Status {
        status: StatusDto,
    },
    Ext {
        ext: input_protocol::v3::StatusExt,
    },
    Sessions {
        sessions: Vec<input_protocol::v3::SessionInfo>,
    },
    Log {
        line: String,
    },
    Error {
        message: String,
    },
    Progress {
        stage: usize,
        message: String,
    },
    BootChoices {
        candidates: Vec<String>,
    },
    Round {
        round: Round,
    },
    Report {
        report: Report,
    },
    Upload {
        outcome: UploadOutcome,
    },
    Cleanup {
        confirmed: bool,
    },
    Done,
    Stopped,
}

impl From<AppEvent> for UiEvent {
    /// Only the events the UI actually renders are converted; the log and the
    /// counters cover the rest.
    fn from(event: AppEvent) -> Self {
        match event {
            AppEvent::Devices(devices) => Self::Devices {
                devices: devices
                    .iter()
                    .enumerate()
                    .map(|(index, device)| DeviceDto::new(Some(index), device))
                    .collect(),
            },
            AppEvent::Connected(device, hello) => Self::Connected {
                device: DeviceDto::new(None, &device),
                hello,
            },
            AppEvent::Disconnected => Self::Disconnected,
            AppEvent::Status(status) => Self::Status {
                status: StatusDto::new(&status),
            },
            AppEvent::Ext(ext) => Self::Ext { ext },
            AppEvent::Sessions(sessions) => Self::Sessions { sessions },
            AppEvent::Log(line) => Self::Log { line },
            AppEvent::Error(message) => Self::Error { message },
            AppEvent::Progress(stage, message) => Self::Progress { stage, message },
            AppEvent::BootChoices(candidates) => Self::BootChoices { candidates },
            AppEvent::Round(round) => Self::Round { round },
            AppEvent::Report(report) => Self::Report { report },
            AppEvent::Upload(outcome) => Self::Upload { outcome },
            AppEvent::Cleanup(confirmed) => Self::Cleanup { confirmed },
            AppEvent::Done => Self::Done,
            AppEvent::Stopped => Self::Stopped,
        }
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub product: String,
    pub commit: String,
    pub target: String,
    /// Where the firmware sources were found, so the UI can show the real path.
    pub firmware_dir: String,
    /// Emergency release hotkey registered (Ctrl+Alt+Shift+Esc).
    pub hotkey: bool,
}
