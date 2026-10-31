use super::*;
use input_protocol::v3::{Packet, StatusExt};
use input_protocol::{KeyId, v3::Command};
impl<T: HidTransport> Session<T> {
    pub(super) fn v3_pump(&mut self, timeout: i32) -> Result<()> {
        let mut r = [0; 64];
        let n = match self.transport.read_timeout(&mut r, timeout) {
            Ok(n) => n,
            Err(e) => return self.fail(e),
        };
        if n == 0 {
            return Ok(());
        }
        if n > 64 {
            return self.fail(anyhow::anyhow!("HID 回應長度錯誤"));
        }
        let Some(client) = self.binary.as_mut() else {
            return self.fail(anyhow::anyhow!("尚未 OPEN v3 session"));
        };
        if let Ok(packet) = input_protocol::v3::Packet::decode(&r[..n], input_protocol::v3::IN_ID)
            && packet.session == client.session
        {
            if self.events.len() >= 512 {
                self.events.remove(0);
            }
            self.events.push(ResponseEvent {
                unix_ms: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis(),
                line: format!(
                    "v3 opcode={},session={},sequence={},payload={:?}",
                    packet.opcode, packet.session, packet.sequence, packet.payload
                ),
            });
        }
        if let Err(e) = client.receive(&r[..n]) {
            return self.fail(e);
        }
        Ok(())
    }
    pub(super) fn v3_send(&mut self, command: &Command, cancel: &Cancellation) -> Result<()> {
        self.v3_exchange(command, cancel).map(|_| ())
    }
    /// Same handshake, but hands back the reply packet so typed queries
    /// (STATUS_EXT, PING) can parse the payload themselves.
    pub fn query(&mut self, command: Command, cancel: &Cancellation) -> Result<Packet> {
        cancel.check()?;
        self.v3_exchange(&command, cancel)?
            .ok_or_else(|| anyhow::anyhow!("命令沒有回應"))
    }
    fn v3_exchange(&mut self, command: &Command, cancel: &Cancellation) -> Result<Option<Packet>> {
        cancel.check()?;
        ensure!(self.valid, "HID session 已失效");
        self.v3_pump(0)?;
        let r = self
            .binary
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("尚未 OPEN v3 session"))?
            .prepare(command)?;
        match self.transport.write(&r) {
            Ok(64) => {}
            Ok(n) => return self.fail(anyhow::anyhow!("HID 短寫入 {n}/64")),
            Err(e) => return self.fail(e),
        }
        if command.expects_reply() {
            let started = Instant::now();
            loop {
                if let Err(e) = cancel.check() {
                    return self.fail(e);
                }
                if started.elapsed() >= Duration::from_secs(1) {
                    return self.fail(anyhow::anyhow!("v3 控制回應逾時"));
                }
                self.v3_pump(10)?;
                if let Some(packet) = self.binary.as_mut().unwrap().take_reply() {
                    return Ok(Some(packet));
                }
            }
        }
        Ok(None)
    }

    pub fn status_ext(&mut self, cancel: &Cancellation) -> Result<StatusExt> {
        let reply = self.query(Command::StatusExt, cancel)?;
        StatusExt::parse(&reply.payload)
    }

    /// Round-trip time in microseconds for a 4-byte echo.
    pub fn ping(&mut self, token: [u8; 4], cancel: &Cancellation) -> Result<u32> {
        let started = Instant::now();
        let reply = self.query(Command::Ping(token.to_vec()), cancel)?;
        ensure!(reply.payload == token, "PING 回音內容不符");
        Ok(started.elapsed().as_micros().min(u128::from(u32::MAX)) as u32)
    }
    pub fn input(&mut self, command: Command, cancel: &Cancellation) -> Result<()> {
        self.v3_send(&command, cancel)
    }
    pub fn wait_until(&mut self, deadline: Instant, cancel: &Cancellation) -> Result<()> {
        let interval = self.heartbeat_interval();
        while deadline.saturating_duration_since(Instant::now()) > interval {
            cancel.sleep(interval)?;
            self.input(Command::Heartbeat, cancel)?;
        }
        crate::platform::wait_until(deadline, cancel)
    }
    pub fn needs_heartbeat(&self, _held: bool) -> bool {
        self.valid && self.binary.is_some()
    }
    pub fn heartbeat_interval(&self) -> Duration {
        Duration::from_millis(500)
    }
    pub fn key(&mut self, key: KeyId, down: bool, cancel: &Cancellation) -> Result<()> {
        self.input(Command::Key(key, down), cancel)
    }
    pub fn set_button(&mut self, button: u8, down: bool, cancel: &Cancellation) -> Result<()> {
        ensure!((1..=5).contains(&button), "無效滑鼠按鈕");
        let bit = 1 << (button - 1);
        let current = self
            .binary
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("尚未 OPEN v3 session"))?
            .state
            .buttons;
        self.input(
            Command::MouseButtons(if down { current | bit } else { current & !bit }),
            cancel,
        )?;
        self.finish(cancel)
    }
}
