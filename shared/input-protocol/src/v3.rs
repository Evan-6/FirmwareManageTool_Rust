use crate::{KeyId, KEYS};
use anyhow::{bail, ensure, Result};
use serde::{Deserialize, Serialize};
pub const OUT_ID: u8 = 12;
pub const IN_ID: u8 = 13;
pub const FEATURE_ID: u8 = 14;
pub const REPORT_SIZE: usize = 64;
pub const PAYLOAD_SIZE: usize = 51;
pub const OPEN: u8 = 1;
pub const HEARTBEAT: u8 = 2;
pub const STATUS: u8 = 3;
pub const BARRIER: u8 = 4;
pub const RELEASE_ALL: u8 = 5;
pub const BOOTLOADER: u8 = 6;
pub const KEY: u8 = 16;
pub const SNAPSHOT: u8 = 17;
pub const MOUSE_MOVE: u8 = 32;
/// Reserved removed absolute opcode; relative-only firmware rejects it.
pub const MOUSE_ABS: u8 = 33;
pub const MOUSE_BUTTONS: u8 = 34;
pub const WHEEL: u8 = 35;
pub const EVENT: u8 = 127;
pub const CONSUMERS: [u16; 7] = [0xe2, 0xe9, 0xea, 0xcd, 0xb5, 0xb6, 0xb7];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub opcode: u8,
    pub flags: u8,
    pub session: u32,
    pub sequence: u32,
    pub payload: Vec<u8>,
}
impl Packet {
    pub fn encode(&self, id: u8) -> Result<[u8; 64]> {
        ensure!(
            matches!(id, OUT_ID | IN_ID) && self.payload.len() <= 51,
            "無效的 v3 封包"
        );
        ensure!(self.flags == 0, "不支援的 v3 flags");
        let mut r = [0; 64];
        r[0] = id;
        r[1] = 3;
        r[2] = self.opcode;
        r[3] = self.payload.len() as u8;
        r[4] = self.flags;
        r[5..9].copy_from_slice(&self.session.to_le_bytes());
        r[9..13].copy_from_slice(&self.sequence.to_le_bytes());
        r[13..13 + self.payload.len()].copy_from_slice(&self.payload);
        Ok(r)
    }
    pub fn decode(r: &[u8], id: u8) -> Result<Self> {
        ensure!(
            r.len() == 64 && r[0] == id && r[1] == 3 && r[4] == 0,
            "無效的 v3 report／版本／flags"
        );
        let n = r[3] as usize;
        ensure!(
            n <= 51 && r[13 + n..].iter().all(|b| *b == 0),
            "無效的 v3 長度／填充"
        );
        Ok(Self {
            opcode: r[2],
            flags: 0,
            session: u32::from_le_bytes(r[5..9].try_into()?),
            sequence: u32::from_le_bytes(r[9..13].try_into()?),
            payload: r[13..13 + n].to_vec(),
        })
    }
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputState {
    pub modifiers: u8,
    pub keys: [u8; 28],
    pub consumer: u8,
    pub buttons: u8,
}
impl InputState {
    pub fn set(&mut self, id: KeyId, down: bool) -> Result<()> {
        ensure!(id.valid(), "不支援的 HID usage");
        let (byte, bit) = if id.usage_page == 7 && (0xe0..=0xe7).contains(&id.usage) {
            (&mut self.modifiers, 1 << (id.usage - 0xe0))
        } else if id.usage_page == 7 {
            (&mut self.keys[id.usage as usize / 8], 1 << (id.usage % 8))
        } else {
            (
                &mut self.consumer,
                1 << CONSUMERS
                    .iter()
                    .position(|u| *u == id.usage)
                    .ok_or_else(|| anyhow::anyhow!("不支援的 Consumer usage"))?,
            )
        };
        if down {
            *byte |= bit;
        } else {
            *byte &= !bit;
        }
        Ok(())
    }
    pub fn contains(&self, id: KeyId) -> bool {
        if !id.valid() {
            return false;
        }
        if id.usage_page == 7 && (0xe0..=0xe7).contains(&id.usage) {
            self.modifiers & (1 << (id.usage - 0xe0)) != 0
        } else if id.usage_page == 7 {
            self.keys[id.usage as usize / 8] & (1 << (id.usage % 8)) != 0
        } else {
            CONSUMERS
                .iter()
                .position(|u| *u == id.usage)
                .is_some_and(|i| self.consumer & (1 << i) != 0)
        }
    }
    pub fn pressed(&self) -> Vec<KeyId> {
        KEYS.iter()
            .filter(|k| self.contains(k.id))
            .map(|k| k.id)
            .collect()
    }
    pub fn encode(&self) -> [u8; 31] {
        let mut r = [0; 31];
        r[0] = self.modifiers;
        r[1..29].copy_from_slice(&self.keys);
        r[29] = self.consumer;
        r[30] = self.buttons;
        r
    }
    pub fn decode(r: &[u8]) -> Result<Self> {
        ensure!(
            r.len() == 31 && r[29] & 0x80 == 0 && r[30] & !31 == 0,
            "無效的輸入快照"
        );
        let s = Self {
            modifiers: r[0],
            keys: r[1..29].try_into()?,
            consumer: r[29],
            buttons: r[30],
        };
        for u in 0..224 {
            if s.keys[u / 8] & (1 << (u % 8)) != 0 {
                ensure!(
                    KeyId {
                        usage_page: 7,
                        usage: u as u16
                    }
                    .valid(),
                    "快照含保留 usage"
                );
            }
        }
        Ok(s)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capabilities {
    pub board: u8,
    pub firmware: String,
    pub lease_ms: u16,
    pub poll_ms: u8,
    pub flags: u32,
    pub keyboard: [u8; 28],
    pub modifiers: u8,
    pub consumer: u8,
    pub device_id: [u8; 8],
}
impl Capabilities {
    pub fn parse(r: &[u8]) -> Result<Self> {
        ensure!(
            r.len() == 64 && r[0] == 14 && &r[1..5] == b"FMT3" && r[5] == 3,
            "無效的能力查詢回應"
        );
        ensure!(r[57..].iter().all(|b| *b == 0), "能力回應保留欄位非零");
        let s = Self {
            board: r[6],
            firmware: format!("{}.{}.{}", r[7], r[8], r[9]),
            lease_ms: u16::from_le_bytes(r[10..12].try_into()?),
            poll_ms: r[14],
            flags: u32::from_le_bytes(r[15..19].try_into()?),
            keyboard: r[19..47].try_into()?,
            modifiers: r[47],
            consumer: r[48],
            device_id: r[49..57].try_into()?,
        };
        ensure!(s.lease_ms >= 1000 && s.poll_ms > 0, "無效的租約／輪詢參數");
        Ok(s)
    }
    pub fn supports(&self, id: KeyId) -> bool {
        if !id.valid() {
            return false;
        }
        if id.usage_page == 7 && id.usage >= 0xe0 {
            self.modifiers & (1 << (id.usage - 0xe0)) != 0
        } else if id.usage_page == 7 {
            self.keyboard[id.usage as usize / 8] & (1 << (id.usage % 8)) != 0
        } else {
            CONSUMERS
                .iter()
                .position(|u| *u == id.usage)
                .is_some_and(|i| self.consumer & (1 << i) != 0)
        }
    }
}
#[derive(Debug, Clone)]
pub enum Command {
    Open,
    Heartbeat,
    Status,
    Barrier,
    ReleaseAll,
    Bootloader,
    Key(KeyId, bool),
    Snapshot(InputState),
    MouseMove(i16, i16),
    MouseButtons(u8),
    Wheel(i16, i16),
}
impl Command {
    pub fn wire(&self) -> Result<(u8, Vec<u8>)> {
        fn pair(a: [u8; 2], b: [u8; 2]) -> Vec<u8> {
            [a, b].concat()
        }
        Ok(match self {
            Self::Open => (OPEN, vec![]),
            Self::Heartbeat => (HEARTBEAT, vec![]),
            Self::Status => (STATUS, vec![]),
            Self::Barrier => (BARRIER, vec![]),
            Self::ReleaseAll => (RELEASE_ALL, vec![]),
            Self::Bootloader => (BOOTLOADER, vec![]),
            Self::Key(k, d) => {
                ensure!(k.valid(), "不支援的按鍵");
                let mut p = pair(k.usage_page.to_le_bytes(), k.usage.to_le_bytes());
                p.push(u8::from(*d));
                (KEY, p)
            }
            Self::Snapshot(s) => {
                InputState::decode(&s.encode())?;
                (SNAPSHOT, s.encode().to_vec())
            }
            Self::MouseMove(x, y) => {
                ensure!(
                    x.abs_diff(0) <= 1024 && y.abs_diff(0) <= 1024,
                    "滑鼠位移超出範圍"
                );
                (MOUSE_MOVE, pair(x.to_le_bytes(), y.to_le_bytes()))
            }
            Self::MouseButtons(b) => {
                ensure!(b & !31 == 0, "無效的滑鼠按鈕");
                (MOUSE_BUTTONS, vec![*b])
            }
            Self::Wheel(v, h) => {
                ensure!(
                    v.abs_diff(0) <= 1024 && h.abs_diff(0) <= 1024,
                    "滾輪超出範圍"
                );
                (WHEEL, pair(v.to_le_bytes(), h.to_le_bytes()))
            }
        })
    }
    pub fn expects_reply(&self) -> bool {
        matches!(
            self,
            Self::Open | Self::Status | Self::Barrier | Self::ReleaseAll | Self::Bootloader
        )
    }
}
#[derive(Debug, Clone)]
pub struct Status {
    pub received: u32,
    pub completed: u32,
    pub pending: u8,
    pub lease_ms: u16,
    pub rx: u16,
    pub tx: u16,
    pub hid: u16,
    pub failsafe: u16,
    pub state: InputState,
}
impl Status {
    pub fn parse(p: &[u8]) -> Result<Self> {
        ensure!(p.len() == 51 && p[0] == 0, "無效的 v3 status");
        let u16at = |i| u16::from_le_bytes([p[i], p[i + 1]]);
        Ok(Self {
            received: u32::from_le_bytes(p[1..5].try_into()?),
            completed: u32::from_le_bytes(p[5..9].try_into()?),
            pending: p[9],
            lease_ms: u16at(10),
            rx: u16at(12),
            tx: u16at(14),
            hid: u16at(16),
            failsafe: u16at(18),
            state: InputState::decode(&p[20..])?,
        })
    }
}
pub fn check_response(p: &Packet, session: u32, sequence: u32, opcode: u8) -> Result<()> {
    ensure!(
        p.session == session && p.sequence == sequence && p.opcode == opcode | 0x80,
        "回應 session／sequence／opcode 不相符"
    );
    ensure!(
        p.payload.len() == if opcode == STATUS { 51 } else { 1 },
        "控制回應長度錯誤"
    );
    let code = *p.payload.first().ok_or_else(|| anyhow::anyhow!("空回應"))?;
    if code != 0 {
        bail!("裝置 v3 錯誤代碼 {code}");
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn report_roundtrip_and_rejection() {
        let p = Packet {
            opcode: KEY,
            flags: 0,
            session: 0x1020304,
            sequence: 7,
            payload: vec![7, 0, 30, 0, 1],
        };
        let mut r = p.encode(OUT_ID).unwrap();
        assert_eq!(Packet::decode(&r, OUT_ID).unwrap(), p);
        r[3] = 52;
        assert!(Packet::decode(&r, OUT_ID).is_err());
        r[3] = 5;
        r[63] = 1;
        assert!(Packet::decode(&r, OUT_ID).is_err());
        r[63] = 0;
        r[1] = 2;
        assert!(Packet::decode(&r, OUT_ID).is_err());
    }
    #[test]
    fn nkro_and_media() {
        let mut s = InputState::default();
        for t in [
            "a",
            "b",
            "c",
            "d",
            "e",
            "f",
            "g",
            "kp_1",
            "1",
            "right_ctrl",
            "volume_up",
        ] {
            s.set(KeyId::from_token(t).unwrap(), true).unwrap();
        }
        assert_eq!(s.pressed().len(), 11);
        assert_eq!(InputState::decode(&s.encode()).unwrap(), s);
        s.keys[0] = 1;
        assert!(InputState::decode(&s.encode()).is_err());
    }
    #[test]
    fn stale_response() {
        let p = Packet {
            opcode: BARRIER | 128,
            flags: 0,
            session: 1,
            sequence: 4,
            payload: vec![0],
        };
        assert!(check_response(&p, 2, 4, BARRIER).is_err());
        assert!(check_response(&p, 1, 5, BARRIER).is_err());
        assert!(check_response(&p, 1, 4, BARRIER).is_ok());
    }
}

#[cfg(test)]
mod wire_vectors {
    use super::*;
    #[test]
    fn golden_packets() {
        let vectors: serde_json::Value =
            serde_json::from_str(include_str!("../vectors.json")).unwrap();
        for vector in vectors.as_array().unwrap() {
            let bytes: Vec<u8> = vector["bytes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u8)
                .collect();
            let p = Packet::decode(&bytes, OUT_ID).unwrap();
            assert_eq!(p.opcode, vector["opcode"].as_u64().unwrap() as u8);
            assert_eq!(p.session, 0x12345678);
            assert_eq!(p.sequence, vector["sequence"].as_u64().unwrap() as u32);
            assert_eq!(p.encode(OUT_ID).unwrap().as_slice(), bytes);
        }
    }
    #[test]
    fn key_input_representation_is_exclusive() {
        for text in [r#"{"k":"kp_1"}"#, r#"{"p":7,"u":89}"#] {
            let k: crate::KeyInput = serde_json::from_str(text).unwrap();
            assert_eq!(k.0, crate::KeyId::from_code("Numpad1").unwrap());
        }
        for text in [
            r#"{"k":"1","p":7,"u":30}"#,
            r#"{"p":7}"#,
            r#"{"p":12,"u":65535}"#,
        ] {
            assert!(serde_json::from_str::<crate::KeyInput>(text).is_err());
        }
    }
}
