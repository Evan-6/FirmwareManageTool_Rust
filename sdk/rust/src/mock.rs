//! In-memory device used by tests and by every binding's self-test. It speaks
//! the same wire protocol as the firmware, including most error paths.

use crate::error::{Error, ErrorCode, Result};
use crate::transport::Transport;
use input_protocol::v3::{self, InputState, Packet};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Debug, Default)]
pub struct MockState {
    pub writes: Vec<Packet>,
    pub state: InputState,
    pub session: u32,
    pub sessions: Vec<u32>,
    pub closed: bool,
    pub silent: bool,
    pub disconnect_after: Option<usize>,
    pub feature_missing: bool,
    pub build: u32,
    pub uptime_ms: u32,
}

#[derive(Clone, Default)]
pub struct MockHandle(Arc<Mutex<MockState>>);

impl MockHandle {
    pub fn writes(&self) -> Vec<Packet> {
        self.0.lock().unwrap().writes.clone()
    }
    pub fn held(&self) -> InputState {
        self.0.lock().unwrap().state.clone()
    }
    pub fn session(&self) -> u32 {
        self.0.lock().unwrap().session
    }
    pub fn sessions(&self) -> Vec<u32> {
        self.0.lock().unwrap().sessions.clone()
    }
    pub fn is_closed(&self) -> bool {
        self.0.lock().unwrap().closed
    }
    pub fn set_silent(&self, silent: bool) {
        self.0.lock().unwrap().silent = silent;
    }
    pub fn set_disconnect_after(&self, writes: Option<usize>) {
        self.0.lock().unwrap().disconnect_after = writes;
    }
    pub fn set_feature_missing(&self, missing: bool) {
        self.0.lock().unwrap().feature_missing = missing;
    }
    pub fn set_build(&self, build: u32) {
        self.0.lock().unwrap().build = build;
    }
}

/// The transport half. Keep a `MockHandle` clone to inspect what the SDK sent.
pub struct MockTransport {
    state: Arc<Mutex<MockState>>,
    replies: VecDeque<[u8; 64]>,
}

impl Default for MockTransport {
    fn default() -> Self {
        Self::new().0
    }
}

impl MockTransport {
    pub fn new() -> (Self, MockHandle) {
        let state = Arc::new(Mutex::new(MockState {
            build: 0x0DB0_0001,
            uptime_ms: 1_000,
            ..MockState::default()
        }));
        (
            Self {
                state: state.clone(),
                replies: VecDeque::new(),
            },
            MockHandle(state),
        )
    }

    fn feature_report(&self) -> [u8; 64] {
        let mut report = [0u8; 64];
        report[0] = 14;
        report[1..5].copy_from_slice(b"FMT3");
        report[5] = 3;
        report[6] = 2;
        report[7] = 3;
        report[8] = 2;
        report[9] = 0;
        report[10..12].copy_from_slice(&2000u16.to_le_bytes());
        report[14] = 1;
        report[15..19].copy_from_slice(&15u32.to_le_bytes());
        for entry in input_protocol::KEYS {
            if entry.id.usage_page == 7 && entry.id.usage < 224 {
                let usage = entry.id.usage as usize;
                report[19 + usage / 8] |= 1 << (usage % 8);
            }
        }
        report[47] = 0xFF;
        report[48] = 0x7F;
        report[49..57].copy_from_slice(&[0x42, 0x50, 0x33, 0x30, 0x34, 0x42, 0x38, 0x33]);
        let build = self.state.lock().unwrap().build;
        report[57..61].copy_from_slice(&build.to_le_bytes());
        report
    }

    fn push_reply(&mut self, opcode: u8, session: u32, sequence: u32, payload: &[u8]) {
        let mut report = [0u8; 64];
        report[0] = v3::IN_ID;
        report[1] = 3;
        report[2] = opcode | 0x80;
        report[3] = payload.len() as u8;
        report[5..9].copy_from_slice(&session.to_le_bytes());
        report[9..13].copy_from_slice(&sequence.to_le_bytes());
        report[13..13 + payload.len()].copy_from_slice(payload);
        self.replies.push_back(report);
    }
}

impl Transport for MockTransport {
    fn feature(&mut self) -> Result<Option<[u8; 64]>> {
        if self.state.lock().unwrap().feature_missing {
            return Ok(None);
        }
        Ok(Some(self.feature_report()))
    }

    fn write(&mut self, report: &[u8; 64]) -> Result<()> {
        let packet = Packet::decode(report, v3::OUT_ID)
            .map_err(|error| Error::new(ErrorCode::Framing, error.to_string()))?;
        let mut state = self.state.lock().unwrap();
        state.writes.push(packet.clone());
        if let Some(limit) = state.disconnect_after
            && state.writes.len() > limit
        {
            return Err(Error::disconnected("mock transport disconnected"));
        }
        let session = packet.session;
        let sequence = packet.sequence;
        let payload = packet.payload.clone();
        let silent = state.silent;
        match packet.opcode {
            v3::OPEN => {
                if session == 0 || sequence != 1 {
                    if !silent {
                        drop(state);
                        self.push_reply(v3::OPEN, session, sequence, &[1]);
                    }
                    return Ok(());
                }
                if state.session == session {
                    if !silent {
                        drop(state);
                        self.push_reply(v3::OPEN, session, sequence, &[2]);
                    }
                    return Ok(());
                }
                if state.sessions.len() >= 8 {
                    if !silent {
                        drop(state);
                        self.push_reply(v3::OPEN, session, sequence, &[2]);
                    }
                    return Ok(());
                }
                state.session = session;
                state.sessions.push(session);
                state.closed = false;
                if !silent {
                    drop(state);
                    self.push_reply(v3::OPEN, session, sequence, &[0]);
                }
                return Ok(());
            }
            v3::CLOSE => {
                state.sessions.retain(|id| *id != session);
                if state.session == session {
                    state.session = 0;
                    state.closed = true;
                }
                state.state = InputState::default();
                if !silent {
                    drop(state);
                    self.push_reply(v3::CLOSE, session, sequence, &[0]);
                }
                return Ok(());
            }
            v3::HEARTBEAT => return Ok(()),
            _ => {}
        }
        if state.session != session {
            // Unknown session: the firmware drops input silently.
            return Ok(());
        }
        match packet.opcode {
            v3::KEY => {
                let page = u16::from_le_bytes([payload[0], payload[1]]);
                let usage = u16::from_le_bytes([payload[2], payload[3]]);
                let key = input_protocol::KeyId {
                    usage_page: page,
                    usage,
                };
                if state.state.set(key, payload[4] != 0).is_err() {
                    drop(state);
                    if !silent {
                        self.push_reply(v3::EVENT, session, sequence, &[3]);
                    }
                    return Ok(());
                }
            }
            v3::SNAPSHOT => {
                if let Ok(next) = InputState::decode(&payload) {
                    state.state = next;
                }
            }
            v3::MOUSE_MOVE | v3::WHEEL => {}
            v3::MOUSE_BUTTONS => state.state.buttons = payload[0],
            v3::RELEASE_ALL => {
                state.state = InputState::default();
                if !silent {
                    drop(state);
                    self.push_reply(v3::RELEASE_ALL, session, sequence, &[0]);
                }
                return Ok(());
            }
            v3::BOOTLOADER => {
                if !silent {
                    drop(state);
                    self.push_reply(v3::BOOTLOADER, session, sequence, &[0]);
                }
                return Ok(());
            }
            v3::BARRIER => {
                if !silent {
                    drop(state);
                    self.push_reply(v3::BARRIER, session, sequence, &[0]);
                }
                return Ok(());
            }
            v3::STATUS => {
                let mut body = [0u8; 51];
                body[1..5].copy_from_slice(&sequence.to_le_bytes());
                body[5..9].copy_from_slice(&sequence.to_le_bytes());
                body[10..12].copy_from_slice(&2000u16.to_le_bytes());
                body[20..].copy_from_slice(&state.state.encode());
                if !silent {
                    drop(state);
                    self.push_reply(v3::STATUS, session, sequence, &body);
                }
                return Ok(());
            }
            v3::STATUS_EXT => {
                let mut body = [0u8; 23];
                let uptime = state.uptime_ms;
                let build = state.build;
                body[1..5].copy_from_slice(&uptime.to_le_bytes());
                body[10] = state.sessions.len() as u8;
                body[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
                body[16] = 3;
                body[17..19].copy_from_slice(&1u16.to_le_bytes());
                body[19..23].copy_from_slice(&build.to_le_bytes());
                if !silent {
                    drop(state);
                    self.push_reply(v3::STATUS_EXT, session, sequence, &body);
                }
                return Ok(());
            }
            v3::SESSION_LIST => {
                let mut body = [0u8; 48];
                for (index, id) in state.sessions.iter().take(8).enumerate() {
                    let base = index * 6;
                    body[base..base + 2].copy_from_slice(&(*id as u16).to_le_bytes());
                    body[base + 2..base + 4].copy_from_slice(&2000u16.to_le_bytes());
                }
                if !silent {
                    drop(state);
                    self.push_reply(v3::SESSION_LIST, session, sequence, &body);
                }
                return Ok(());
            }
            _ => {
                drop(state);
                if !silent {
                    self.push_reply(v3::EVENT, session, sequence, &[3]);
                }
                return Ok(());
            }
        }
        Ok(())
    }

    fn read(&mut self, _timeout_ms: i32) -> Result<Option<[u8; 64]>> {
        Ok(self.replies.pop_front())
    }
}
