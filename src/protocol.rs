//! The wire types the UI shows: `Hello` and `Status`. Identity and usage live in `gdb`.
pub use input_protocol::{KeyId, v3};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: u8,
    pub nkro: bool,
    pub media: bool,
    pub firmware: String,
    pub lease_ms: u64,
    pub mouse: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub fields: BTreeMap<String, String>,
    pub raw: String,
}
impl Status {
    /// Check shared transport failures. `fs` also counts other clients' lease
    /// expiry; our own faults are rejected by Session's session-tagged EVENT.
    pub fn check_transport_since(&self, before: &Self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.count("hid") == before.count("hid") && self.count("tx") == before.count("tx"),
            "裝置傳輸失敗計數增加：hid {}→{}，tx {}→{}",
            before.count("hid"),
            self.count("hid"),
            before.count("tx"),
            self.count("tx")
        );
        Ok(())
    }
    pub fn count(&self, name: &str) -> u64 {
        self.fields
            .get(name)
            .and_then(|n| n.parse().ok())
            .unwrap_or(0)
    }
    pub fn buttons(&self) -> u8 {
        self.fields
            .get("mb")
            .and_then(|n| u8::from_str_radix(n, 16).ok())
            .unwrap_or(0)
    }
    pub fn released(&self) -> bool {
        self.count("p") == 0 && self.buttons() == 0
    }
}
