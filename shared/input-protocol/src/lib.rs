//! Portable physical-key model and Vendor HID protocol v3.
pub mod client;
pub mod v3;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct KeyId {
    pub usage_page: u16,
    pub usage: u16,
}
pub struct KeyEntry {
    pub id: KeyId,
    pub token: &'static str,
    pub aliases: &'static [&'static str],
    pub code: Option<&'static str>,
}
include!("keys_generated.rs");
impl KeyId {
    pub fn from_token(raw: &str) -> Option<Self> {
        let token = raw.trim().to_ascii_lowercase();
        KEYS.iter()
            .find(|k| k.token == token || k.aliases.contains(&token.as_str()))
            .map(|k| k.id)
    }
    pub fn from_code(code: &str) -> Option<Self> {
        KEYS.iter().find(|k| k.code == Some(code)).map(|k| k.id)
    }
    pub fn entry(self) -> Option<&'static KeyEntry> {
        KEYS.iter().find(|k| k.id == self)
    }
    pub fn valid(self) -> bool {
        self.entry().is_some()
    }
    pub fn token(self) -> &'static str {
        self.entry().map(|k| k.token).unwrap_or("unknown")
    }
}

/// JSON accepts exactly one complete representation: named k, or physical p/u.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyInput(pub KeyId);
impl Serialize for KeyInput {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut out = s.serialize_struct("KeyInput", 2)?;
        out.serialize_field("p", &self.0.usage_page)?;
        out.serialize_field("u", &self.0.usage)?;
        out.end()
    }
}
impl<'de> Deserialize<'de> for KeyInput {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        fn present<'de, D, T>(d: D) -> std::result::Result<Option<Option<T>>, D::Error>
        where
            D: serde::Deserializer<'de>,
            T: Deserialize<'de>,
        {
            Option::<T>::deserialize(d).map(Some)
        }
        #[derive(Deserialize)]
        struct Wire {
            #[serde(default, deserialize_with = "present")]
            k: Option<Option<String>>,
            #[serde(default, deserialize_with = "present")]
            p: Option<Option<u16>>,
            #[serde(default, deserialize_with = "present")]
            u: Option<Option<u16>>,
        }
        let w = Wire::deserialize(d)?;
        let id = match (w.k, w.p, w.u) {
            (Some(Some(k)), None, None) => KeyId::from_token(&k),
            (None, Some(Some(p)), Some(Some(u))) => Some(KeyId {
                usage_page: p,
                usage: u,
            })
            .filter(|k| k.valid()),
            _ => None,
        };
        id.map(Self)
            .ok_or_else(|| serde::de::Error::custom("按鍵須使用合法 k 或完整 p/u，不能混用"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_and_generated_entries_match_without_duplicate_identities() {
        let source: serde_json::Value = serde_json::from_str(include_str!("../keys.json")).unwrap();
        assert_eq!(source["version"], 3);
        let rows = source["keys"].as_array().unwrap();
        assert_eq!(rows.len(), KEYS.len());
        let mut ids = std::collections::BTreeSet::new();
        let mut codes = std::collections::BTreeSet::new();
        let mut tokens = std::collections::BTreeSet::new();
        for (row, entry) in rows.iter().zip(KEYS) {
            assert!(ids.insert(entry.id));
            assert_eq!(row["page"], entry.id.usage_page);
            assert_eq!(row["usage"], entry.id.usage);
            assert_eq!(row["token"], entry.token);
            assert_eq!(row["code"].as_str(), entry.code);
            if let Some(code) = entry.code {
                assert!(codes.insert(code));
            }
            assert!(tokens.insert(entry.token));
            let aliases: Vec<_> = row["aliases"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect();
            assert_eq!(aliases, entry.aliases);
            for alias in entry.aliases {
                assert!(tokens.insert(alias));
            }
        }
    }
    #[test]
    fn physical_identity() {
        assert_ne!(KeyId::from_code("Digit1"), KeyId::from_code("Numpad1"));
        assert_ne!(KeyId::from_code("Enter"), KeyId::from_code("NumpadEnter"));
        assert_ne!(
            KeyId::from_token("left_ctrl"),
            KeyId::from_token("right_ctrl")
        );
        assert_eq!(KeyId::from_token("ctrl"), KeyId::from_token("left_ctrl"));
        assert_eq!(KeyId::from_code("AudioVolumeUp").unwrap().usage_page, 12);
    }
}

/// Mix a per-call counter into time and process identity for a new connection nonce.
pub fn new_session_id() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(1);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    ((now as u32)
        ^ ((now >> 32) as u32)
        ^ std::process::id()
        ^ NEXT.fetch_add(0x9e3779b9, Ordering::Relaxed))
    .max(1)
}

#[cfg(feature = "test-support")]
pub mod testing;

pub mod pointer;
