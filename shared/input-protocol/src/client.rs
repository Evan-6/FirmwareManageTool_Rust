//! Session bookkeeping shared by the manager and remote worker; I/O stays platform owned.
use crate::v3::{self, Capabilities, Command, InputState, Packet, Status};
use anyhow::{ensure, Result};
#[derive(Debug)]
pub struct Client {
    pub capabilities: Capabilities,
    pub state: InputState,
    pub session: u32,
    pub sequence: u32,
    pub last_status: Option<Status>,
    waiting: Option<(u32, u8)>,
    reply: Option<Packet>,
}
impl Client {
    pub fn new(capabilities: Capabilities, session: u32) -> Self {
        Self {
            capabilities,
            state: InputState::default(),
            session: session.max(1),
            sequence: 0,
            last_status: None,
            waiting: None,
            reply: None,
        }
    }
    pub fn prepare(&mut self, command: &Command) -> Result<[u8; 64]> {
        ensure!(self.waiting.is_none(), "尚有未完成的控制要求");
        ensure!(self.sequence < u32::MAX, "命令序號耗盡，請重新 OPEN");
        let (opcode, payload) = command.wire()?;
        match command {
            Command::Key(k, d) => {
                ensure!(self.capabilities.supports(*k), "裝置不支援 {}", k.token());
                self.state.set(*k, *d)?;
            }
            Command::Snapshot(s) => {
                for k in s.pressed() {
                    ensure!(self.capabilities.supports(k), "裝置不支援 {}", k.token());
                }
                self.state = s.clone();
            }
            Command::MouseButtons(b) => self.state.buttons = *b,
            Command::Open | Command::ReleaseAll | Command::Bootloader => {
                self.state = InputState::default()
            }
            _ => {}
        }
        self.sequence += 1;
        if command.expects_reply() {
            self.waiting = Some((self.sequence, opcode));
        }
        Packet {
            opcode,
            flags: 0,
            session: self.session,
            sequence: self.sequence,
            payload,
        }
        .encode(v3::OUT_ID)
    }
    pub fn receive(&mut self, r: &[u8]) -> Result<()> {
        let p = Packet::decode(r, v3::IN_ID)?;
        if p.session != self.session {
            return Ok(());
        }
        ensure!(
            p.opcode != v3::EVENT,
            "裝置故障／租約釋放事件：{:?}",
            p.payload
        );
        let (seq, op) = self
            .waiting
            .ok_or_else(|| anyhow::anyhow!("無對應控制要求的 v3 回應"))?;
        v3::check_response(&p, self.session, seq, op)?;
        if op == v3::STATUS {
            let status = Status::parse(&p.payload)?;
            ensure!(
                status.received == p.sequence
                    && status.completed <= status.received
                    && status.pending <= 32
                    && status.lease_ms <= self.capabilities.lease_ms,
                "狀態序號／佇列／租約不符"
            );
            self.last_status = Some(status);
        }
        self.waiting = None;
        self.reply = Some(p);
        Ok(())
    }
    pub fn take_reply(&mut self) -> Option<Packet> {
        self.reply.take()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn caps() -> Capabilities {
        let mut r = [0; 64];
        r[0] = 14;
        r[1..5].copy_from_slice(b"FMT3");
        r[5] = 3;
        r[10..12].copy_from_slice(&2000u16.to_le_bytes());
        r[14] = 1;
        r[19..47].fill(255);
        r[47] = 255;
        r[48] = 127;
        Capabilities::parse(&r).unwrap()
    }
    #[test]
    fn late_old_session_cannot_finish_open() {
        let mut c = Client::new(caps(), 42);
        c.prepare(&Command::Open).unwrap();
        let p = Packet {
            opcode: 129,
            flags: 0,
            session: 41,
            sequence: 1,
            payload: vec![0],
        };
        c.receive(&p.encode(13).unwrap()).unwrap();
        assert!(c.take_reply().is_none());
        let p = Packet { session: 42, ..p };
        c.receive(&p.encode(13).unwrap()).unwrap();
        assert!(c.take_reply().is_some());
    }
    #[test]
    fn no_input_ack() {
        let mut c = Client::new(caps(), 42);
        c.prepare(&Command::Heartbeat).unwrap();
        c.prepare(&Command::Key(crate::KeyId::from_token("a").unwrap(), true))
            .unwrap();
        c.prepare(&Command::Barrier).unwrap();
        assert!(c.prepare(&Command::Heartbeat).is_err());
    }
    #[test]
    fn malformed_control_payload_cannot_confirm_completion() {
        let mut client = Client::new(caps(), 42);
        client.prepare(&Command::Open).unwrap();
        let p = Packet {
            opcode: 129,
            flags: 0,
            session: 42,
            sequence: 1,
            payload: vec![0, 0],
        };
        assert!(client.receive(&p.encode(13).unwrap()).is_err());
        assert!(client.take_reply().is_none());
    }
}
