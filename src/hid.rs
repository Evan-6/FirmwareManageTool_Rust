use crate::{
    cancel::Cancellation,
    protocol::{self, Decoder, Hello, Status},
};
#[cfg(not(windows))]
use anyhow::bail;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub trait HidTransport: Send {
    fn feature(&mut self) -> Result<Option<[u8; 64]>> {
        Ok(None)
    }
    fn write(&mut self, report: &[u8]) -> Result<usize>;
    fn read_timeout(&mut self, buffer: &mut [u8], timeout_ms: i32) -> Result<usize>;
}
impl<T: HidTransport + ?Sized> HidTransport for Box<T> {
    fn feature(&mut self) -> Result<Option<[u8; 64]>> {
        (**self).feature()
    }
    fn write(&mut self, report: &[u8]) -> Result<usize> {
        (**self).write(report)
    }
    fn read_timeout(&mut self, buffer: &mut [u8], timeout_ms: i32) -> Result<usize> {
        (**self).read_timeout(buffer, timeout_ms)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Device {
    pub path: String,
    pub serial: String,
    pub product: String,
    pub vid: u16,
    pub pid: u16,
}
impl Device {
    pub fn label(&self) -> String {
        format!(
            "{} {:04X}:{:04X} {}",
            self.product, self.vid, self.pid, self.serial
        )
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseEvent {
    pub unix_ms: u128,
    pub line: String,
}
struct Pending {
    expected: String,
    sent: Instant,
}
pub struct Session<T: HidTransport> {
    transport: T,
    decoder: Decoder,
    pending: VecDeque<Pending>,
    replies: VecDeque<String>,
    events: Vec<ResponseEvent>,
    valid: bool,
    resynchronizing: bool,
    binary: Option<input_protocol::client::Client>,
    buttons: u8,
}
pub type NativeSession = Session<Box<dyn HidTransport>>;
impl<T: HidTransport> Session<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            decoder: Decoder::default(),
            pending: VecDeque::new(),
            replies: VecDeque::new(),
            events: Vec::new(),
            valid: true,
            resynchronizing: false,
            binary: None,
            buttons: 0,
        }
    }
    /// A new connection releases input before hello. Old ACKs cannot satisfy reset/status.
    pub fn synchronize(&mut self, cancel: &Cancellation) -> Result<()> {
        if let Some(report) = self.transport.feature()? {
            let caps = input_protocol::v3::Capabilities::parse(&report)?;
            self.binary = Some(input_protocol::client::Client::new(
                caps,
                input_protocol::new_session_id(),
            ));
            self.v3_send(&input_protocol::v3::Command::Open, cancel)?;
            return Ok(());
        }
        let mut buffer = [0; protocol::REPORT_SIZE];
        // Drain already queued responses without attributing them to a new command.
        for _ in 0..64 {
            cancel.check()?;
            let count = self.transport.read_timeout(&mut buffer, 0)?;
            if count == 0 {
                break;
            }
            ensure!(count <= buffer.len(), "HID 回應長度錯誤");
            for line in self.decoder.feed(&buffer[..count])? {
                self.events.push(ResponseEvent {
                    unix_ms: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis(),
                    line,
                });
            }
        }
        self.decoder = Decoder::default();
        self.resynchronizing = true;
        let result = self.reset(cancel);
        self.resynchronizing = false;
        result
    }
    pub fn is_valid(&self) -> bool {
        self.valid
    }
    pub fn take_events(&mut self) -> Vec<ResponseEvent> {
        std::mem::take(&mut self.events)
    }
    fn fail<R>(&mut self, error: anyhow::Error) -> Result<R> {
        self.valid = false;
        Err(error)
    }
    fn pump(&mut self, timeout: i32) -> Result<()> {
        if self.binary.is_some() {
            return self.v3_pump(timeout);
        }
        let mut buffer = [0; protocol::REPORT_SIZE];
        let count = match self.transport.read_timeout(&mut buffer, timeout) {
            Ok(n) => n,
            Err(e) => return self.fail(e),
        };
        if count == 0 {
            return Ok(());
        }
        let lines = match self.decoder.feed(&buffer[..count]) {
            Ok(lines) => lines,
            Err(e) => return self.fail(e),
        };
        for line in lines {
            if self.events.len() >= 512 {
                self.events.remove(0);
            }
            self.events.push(ResponseEvent {
                unix_ms: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis(),
                line: line.clone(),
            });
            if line.starts_with("err:") || line.starts_with("warn:failsafe") {
                return self.fail(anyhow::anyhow!("裝置回報：{line}"));
            }
            if line.starts_with("mscv-keyboard:ready") {
                continue;
            }
            if let Some(pending) = self.pending.front() {
                if !pending.expected.split('|').any(|prefix| {
                    line == prefix || (prefix.ends_with(',') && line.starts_with(prefix))
                }) {
                    if self.resynchronizing {
                        continue;
                    }
                    return self.fail(anyhow::anyhow!(
                        "回應順序錯誤，預期 {}，收到 {line}",
                        pending.expected
                    ));
                }
                self.pending.pop_front();
                self.replies.push_back(line);
            } else if (line.starts_with("ok:") || line == "pong") && !self.resynchronizing {
                return self.fail(anyhow::anyhow!("收到無對應命令的回應：{line}"));
            }
        }
        Ok(())
    }
    pub fn poll(&mut self) -> Result<()> {
        ensure!(self.valid, "HID session 已失效");
        self.pump(0)
    }
    pub fn queue(&mut self, command: &str, expected: &str, cancel: &Cancellation) -> Result<()> {
        ensure!(self.binary.is_none(), "v3 session 必須使用型別化輸入介面");
        cancel.check()?;
        ensure!(self.valid, "HID session 已失效，請重新連線");
        // Four is the total outstanding limit, including the command about to be sent.
        while self.pending.len() >= 4 {
            self.wait_one(cancel)?;
        }
        self.pump(0)?;
        let reports = protocol::encode(command)?;
        // A response can arrive immediately after the last write. Register before writing.
        self.pending.push_back(Pending {
            expected: expected.into(),
            sent: Instant::now(),
        });
        for report in reports {
            match self.transport.write(&report) {
                Ok(protocol::REPORT_SIZE) => {}
                Ok(n) => return self.fail(anyhow::anyhow!("HID 寫入不完整：{n}/64 bytes")),
                Err(e) => return self.fail(e),
            }
        }
        Ok(())
    }
    fn wait_one(&mut self, cancel: &Cancellation) -> Result<()> {
        let before = self.pending.len();
        while self.pending.len() >= before && before > 0 {
            if let Err(e) = cancel.check() {
                return self.fail(e);
            }
            if self
                .pending
                .front()
                .is_some_and(|p| p.sent.elapsed() >= Duration::from_secs(1))
            {
                return self.fail(anyhow::anyhow!("HID ACK 逾時（1 秒）"));
            }
            self.pump(10)?;
        }
        Ok(())
    }
    pub fn finish(&mut self, cancel: &Cancellation) -> Result<()> {
        if self.binary.is_some() {
            self.v3_send(&input_protocol::v3::Command::Barrier, cancel)?;
            return Ok(());
        }
        while !self.pending.is_empty() {
            self.wait_one(cancel)?;
        }
        self.pump(0)?;
        self.replies.clear();
        Ok(())
    }
    pub fn command(
        &mut self,
        command: &str,
        expected: &str,
        cancel: &Cancellation,
    ) -> Result<String> {
        self.finish(cancel)?;
        self.queue(command, expected, cancel)?;
        while !self.pending.is_empty() {
            self.wait_one(cancel)?;
        }
        self.replies
            .pop_back()
            .ok_or_else(|| anyhow::anyhow!("缺少回應"))
    }
    pub fn capabilities(&self) -> Option<&input_protocol::v3::Capabilities> {
        self.binary.as_ref().map(|b| &b.capabilities)
    }
    pub fn hello(&mut self, cancel: &Cancellation) -> Result<Hello> {
        if let Some(binary) = &self.binary {
            let c = &binary.capabilities;
            return Ok(Hello {
                protocol: 3,
                nkro: true,
                media: c.consumer != 0,
                firmware: c.firmware.clone(),
                lease_ms: c.lease_ms.into(),
                mouse: true,
                raw: format!(
                    "protocol=3,board={},fw={},lease_ms={},NKRO,consumer={:02X},device={:02X?}",
                    c.board, c.firmware, c.lease_ms, c.consumer, c.device_id
                ),
            });
        }
        Hello::parse(&self.command("hello", "ok:hello,", cancel)?)
    }
    pub fn status(&mut self, cancel: &Cancellation) -> Result<Status> {
        if self.binary.is_some() {
            self.v3_send(&input_protocol::v3::Command::Status, cancel)?;
            let s = self.binary.as_ref().unwrap().last_status.as_ref().unwrap();
            let mut fields = std::collections::BTreeMap::new();
            for (key, value) in [
                ("p", s.state.pressed().len() as u64),
                ("rx", s.rx.into()),
                ("tx", s.tx.into()),
                ("hid", s.hid.into()),
                ("fs", s.failsafe.into()),
                ("received", s.received.into()),
                ("completed", s.completed.into()),
                ("pending", s.pending.into()),
                ("lease", s.lease_ms.into()),
            ] {
                fields.insert(key.into(), value.to_string());
            }
            fields.insert("mb".into(), format!("{:02X}", s.state.buttons));
            fields.insert(
                "keys".into(),
                s.state
                    .pressed()
                    .iter()
                    .map(|k| k.token())
                    .collect::<Vec<_>>()
                    .join(","),
            );
            return Ok(Status {
                raw: format!("v3 {fields:?}"),
                fields,
            });
        }
        Status::parse(&self.command("status", "ok:status,", cancel)?)
    }
    pub fn reset(&mut self, cancel: &Cancellation) -> Result<()> {
        if self.binary.is_some() {
            self.v3_send(&input_protocol::v3::Command::ReleaseAll, cancel)?;
        } else {
            self.command("reset", "ok:release_all", cancel)?;
        }
        self.buttons = 0;
        ensure!(self.status(cancel)?.released(), "reset 後仍有按鍵按住");
        Ok(())
    }
}

mod transport;
pub use transport::{enumerate, open};
mod v3;

#[cfg(test)]
pub(crate) mod tests;
