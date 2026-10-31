//! The command surface the React UI calls. Every command maps to one
//! `AppCommand` on the existing worker, so the hardware paths stay in one place.

use crate::dto::{AppInfo, DeviceDto};
use crate::shell::State;
use std::path::PathBuf;

use firmware_manage_tool::{
    backend::AppCommand,
    firmware,
    firmware::UploadConfig,
    hid,
    latency::{LatencyConfig, Report as LatencyReport},
    mouse::MouseAction,
    platform,
    settings::Settings,
};
use tauri::State as TauriState;

fn err(error: impl std::fmt::Display) -> String {
    format!("{error:#}")
}

#[tauri::command]
pub fn app_info() -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        product: "FirmwareManageTool".to_string(),
        commit: option_env!("APP_COMMIT").unwrap_or("unknown").to_string(),
        target: firmware::TARGET.to_string(),
        firmware_dir: UploadConfig::default().sources().display().to_string(),
        hotkey: crate::shell::HOTKEY.load(std::sync::atomic::Ordering::Relaxed),
    }
}

#[tauri::command]
pub fn scan() -> Result<Vec<DeviceDto>, String> {
    let devices = hid::enumerate().map_err(err)?;
    Ok(devices
        .iter()
        .enumerate()
        .map(|(index, device)| DeviceDto::new(Some(index), device))
        .collect())
}

#[tauri::command]
pub fn connect(state: TauriState<'_, State>, index: usize) -> Result<(), String> {
    let devices = hid::enumerate().map_err(err)?;
    let device = devices
        .get(index)
        .ok_or_else(|| "裝置已移除，請重新掃描".to_string())?;
    state.submit(AppCommand::Connect(device.clone()))
}

#[tauri::command]
pub fn disconnect(state: TauriState<'_, State>) -> Result<(), String> {
    state.submit(AppCommand::Disconnect)
}

#[tauri::command]
pub fn request_status(state: TauriState<'_, State>) -> Result<(), String> {
    state.submit(AppCommand::Status)
}

#[tauri::command]
pub fn reset_input(state: TauriState<'_, State>) -> Result<(), String> {
    state.submit(AppCommand::Reset)
}

#[tauri::command]
pub fn upload(state: TauriState<'_, State>, config: UploadConfig) -> Result<(), String> {
    state.submit(AppCommand::Upload(config))
}

#[tauri::command]
pub fn latency(state: TauriState<'_, State>, config: LatencyConfig) -> Result<(), String> {
    state.submit(AppCommand::Latency(config))
}

#[tauri::command]
pub fn mouse(state: TauriState<'_, State>, action: MouseAction) -> Result<(), String> {
    state.submit(AppCommand::Mouse(action))
}

#[tauri::command]
pub fn choose_target(state: TauriState<'_, State>, target: String) -> Result<(), String> {
    state.choose(target)
}

#[tauri::command]
pub fn cancel(state: TauriState<'_, State>) -> Result<(), String> {
    state.cancel();
    Ok(())
}

/// Writes the latency report as the CSV the egui build used to produce. The
/// report arrives from the UI so the worker does not have to retain it.
#[tauri::command]
pub fn export_report_csv(report: LatencyReport, path: Option<String>) -> Result<String, String> {
    let path = match path.as_deref().map(str::trim) {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_secs())
                .unwrap_or_default();
            platform::application_dir()
                .map_err(err)?
                .join(format!("latency-{stamp}.csv"))
        }
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(err)?;
    }
    report.export(&path).map_err(err)?;
    Ok(path.display().to_string())
}

#[tauri::command]
pub fn settings_load() -> Settings {
    Settings::load()
}

#[tauri::command]
pub fn settings_save(settings: Settings) -> Result<(), String> {
    settings.save().map_err(err)
}
