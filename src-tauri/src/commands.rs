//! The command surface the React UI calls. Every command maps to one
//! `AppCommand` on the existing worker, so the hardware paths stay in one place.

use crate::dto::{AppInfo, DeviceDto};
use crate::shell::State;
use firmware_manage_tool::{
    backend::AppCommand,
    firmware,
    firmware::UploadConfig,
    hid,
    latency::LatencyConfig,
    mouse::{HumanProfile, MouseRequest},
    platform,
    settings::Settings,
    typing::{ChordConfig, TypeTextConfig},
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
pub fn mouse(state: TauriState<'_, State>, request: MouseRequest) -> Result<(), String> {
    state.submit(AppCommand::Mouse(request))
}

#[tauri::command]
pub fn type_text(state: TauriState<'_, State>, config: TypeTextConfig) -> Result<(), String> {
    state.submit(AppCommand::TypeText(config))
}

#[tauri::command]
pub fn chord(state: TauriState<'_, State>, config: ChordConfig) -> Result<(), String> {
    state.submit(AppCommand::Chord(config))
}

/// Nodes the same `trajectory_with` would produce, so the UI preview is the
/// actual path and not a decorative approximation.
#[tauri::command]
pub fn trajectory_preview(
    target_x: i32,
    target_y: i32,
    duration_ms: u32,
    profile: HumanProfile,
) -> Result<serde_json::Value, String> {
    let (screen, start) = platform::screen_and_cursor().map_err(err)?;
    let nodes = firmware_manage_tool::mouse::trajectory_with(
        start,
        (target_x, target_y),
        screen,
        duration_ms,
        &profile,
        &mut rand::rng(),
    )
    .map_err(err)?;
    Ok(serde_json::json!({
        "startX": start.0,
        "startY": start.1,
        "width": screen.width,
        "height": screen.height,
        "nodes": nodes
            .iter()
            .map(|node| serde_json::json!({ "atMs": node.at_ms, "x": node.x, "y": node.y }))
            .collect::<Vec<_>>(),
    }))
}

/// Macros are plain JSON next to the settings so they can be edited by hand.
fn macro_path(name: &str) -> Result<std::path::PathBuf, String> {
    let safe: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ' '))
        .collect();
    if safe.trim().is_empty() {
        return Err("巨集名稱不可為空".into());
    }
    platform::application_dir()
        .map(|dir| dir.join("macros").join(format!("{}.json", safe.trim())))
        .map_err(err)
}

#[tauri::command]
pub fn macro_save(name: String, steps: serde_json::Value) -> Result<String, String> {
    let path = macro_path(&name)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(err)?;
    }
    let body = serde_json::to_vec_pretty(&steps).map_err(err)?;
    std::fs::write(&path, body).map_err(err)?;
    Ok(path.display().to_string())
}

#[tauri::command]
pub fn macro_load(name: String) -> Result<serde_json::Value, String> {
    let path = macro_path(&name)?;
    let data = std::fs::read(&path).map_err(err)?;
    serde_json::from_slice(&data).map_err(err)
}

#[tauri::command]
pub fn macro_list() -> Result<Vec<String>, String> {
    let dir = platform::application_dir().map_err(err)?.join("macros");
    let mut names = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json")
                && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
            {
                names.push(stem.to_string());
            }
        }
    }
    names.sort();
    Ok(names)
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

#[tauri::command]
pub fn settings_load() -> Settings {
    Settings::load()
}

#[tauri::command]
pub fn settings_save(settings: Settings) -> Result<(), String> {
    settings.save().map_err(err)
}
