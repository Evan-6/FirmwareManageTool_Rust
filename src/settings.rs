use crate::{firmware::UploadConfig, latency::LatencyConfig, mouse::HumanProfile};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub upload: UploadConfig,
    pub latency: LatencyConfig,
    /// Humanised movement shape, shared by every mouse action.
    pub mouse: HumanProfile,
    /// Debug mode polls STATUS every 150 ms and forwards every HID line. Off means
    /// game mode: heartbeat only, nothing else touches the bus.
    pub debug: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            upload: UploadConfig::default(),
            latency: LatencyConfig::default(),
            mouse: HumanProfile::default(),
            debug: true,
        }
    }
}
impl Settings {
    fn path() -> Result<PathBuf> {
        Ok(crate::platform::application_dir()?.join("settings.json"))
    }
    pub fn load() -> Self {
        Self::path()
            .ok()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default()
    }
    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(path, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }
}
