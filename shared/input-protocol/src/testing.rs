//! Test I/O model. Firmware scheduling is tested separately against the real C++ engine.
use crate::{v3::*, KEYS};
use std::collections::VecDeque;
/// Reported by the fake STATUS_EXT and by nothing else, so tests can tell a real
/// device apart from this model.
pub const TEST_BUILD: u32 = 0x00a1_b2c3;
#[derive(Default)]
pub struct UsbModel {
    pub writes: Vec<Packet>,
    pub reads: VecDeque<[u8; 64]>,
    pub state: InputState,
    pub short: bool,
    pub silent: bool,
}
impl UsbModel {
    pub fn feature() -> [u8; 64] {
        let mut r = [0; 64];
        r[0] = 14;
        r[1..5].copy_from_slice(b"FMT3");
        r[5] = 3;
        r[6] = 2;
        r[7] = 3;
        r[8] = 2;
        r[15..19].copy_from_slice(&15u32.to_le_bytes());
        r[10..12].copy_from_slice(&2000u16.to_le_bytes());
        r[14] = 1;
        for entry in KEYS {
            let u = entry.id.usage as usize;
            if entry.id.usage_page == 7 && u < 224 {
                r[19 + u / 8] |= 1 << (u % 8);
            }
        }
        r[47] = 255;
        r[48] = 127;
        r
    }
    pub fn write(&mut self, r: &[u8]) -> anyhow::Result<usize> {
        if self.short {
            return Ok(63);
        }
        let p = Packet::decode(r, OUT_ID)?;
        match p.opcode {
            OPEN | CLOSE | RELEASE_ALL | BOOTLOADER => self.state = InputState::default(),
            KEY => {
                let k = crate::KeyId {
                    usage_page: u16::from_le_bytes(p.payload[0..2].try_into()?),
                    usage: u16::from_le_bytes(p.payload[2..4].try_into()?),
                };
                self.state.set(k, p.payload[4] != 0)?;
            }
            SNAPSHOT => self.state = InputState::decode(&p.payload)?,
            MOUSE_BUTTONS => self.state.buttons = p.payload[0],
            _ => {}
        }
        if !self.silent
            && matches!(
                p.opcode,
                OPEN | CLOSE
                    | STATUS
                    | STATUS_EXT
                    | SESSION_LIST
                    | BARRIER
                    | RELEASE_ALL
                    | BOOTLOADER
            )
        {
            let mut payload = vec![0];
            match p.opcode {
                STATUS => {
                    payload.resize(51, 0);
                    payload[1..5].copy_from_slice(&p.sequence.to_le_bytes());
                    payload[5..9].copy_from_slice(&p.sequence.to_le_bytes());
                    payload[10..12].copy_from_slice(&2000u16.to_le_bytes());
                    payload[20..].copy_from_slice(&self.state.encode());
                }
                STATUS_EXT => {
                    payload.resize(STATUS_EXT_BYTES, 0);
                    payload[1..5].copy_from_slice(&1_000u32.to_le_bytes());
                    payload[10] = 1;
                    payload[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
                    payload[16] = 3;
                    payload[17..19].copy_from_slice(&1u16.to_le_bytes());
                    payload[19..23].copy_from_slice(&TEST_BUILD.to_le_bytes());
                }
                SESSION_LIST => {
                    // Fixed eight-slot table with no result byte: this session owns slot 0.
                    payload.resize(SESSION_LIST_BYTES, 0);
                    payload[..2].copy_from_slice(&(p.session as u16).to_le_bytes());
                    payload[2..4].copy_from_slice(&2000u16.to_le_bytes());
                    payload[4] = self.state.pressed().len() as u8;
                }
                _ => {}
            }
            self.reads.push_back(
                Packet {
                    opcode: p.opcode | 128,
                    flags: 0,
                    session: p.session,
                    sequence: p.sequence,
                    payload,
                }
                .encode(IN_ID)?,
            );
        }
        self.writes.push(p);
        Ok(64)
    }
    pub fn read(&mut self, r: &mut [u8]) -> usize {
        if let Some(data) = self.reads.pop_front() {
            r[..64].copy_from_slice(&data);
            64
        } else {
            0
        }
    }
    pub fn fault(&mut self, code: u8) {
        let p = self.writes.last().unwrap();
        self.reads.push_back(
            Packet {
                opcode: EVENT,
                flags: 0,
                session: p.session,
                sequence: p.sequence,
                payload: vec![code],
            }
            .encode(IN_ID)
            .unwrap(),
        );
    }
}
