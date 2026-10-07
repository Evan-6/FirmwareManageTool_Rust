use crate::{
    cancel::Cancellation,
    protocol::{Hello, Status},
};
#[cfg(not(windows))]
use anyhow::bail;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
pub struct Session<T: HidTransport> {
    transport: T,
    events: Vec<ResponseEvent>,
    valid: bool,
    binary: Option<input_protocol::client::Client>,
}
pub type NativeSession = Session<Box<dyn HidTransport>>;
impl<T: HidTransport> Session<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            events: Vec::new(),
            valid: true,
            binary: None,
        }
    }
    /// Query v3 capabilities and OPEN this program's independent session.
    pub fn synchronize(&mut self, cancel: &Cancellation) -> Result<()> {
        cancel.check()?;
        ensure!(self.binary.is_none(), "session 已建立，請使用新連線");
        let result = (|| -> Result<()> {
            let report = self
                .transport
                .feature()?
                .ok_or_else(|| anyhow::anyhow!("裝置不支援 Vendor HID v3，請更新韌體"))?;
            let caps = input_protocol::v3::Capabilities::parse(&report)?;
            self.binary = Some(input_protocol::client::Client::new(
                caps,
                input_protocol::new_session_id(),
            ));
            self.v3_send(&input_protocol::v3::Command::Open, cancel)
        })();
        match result {
            Ok(()) => Ok(()),
            Err(error) => self.fail(error),
        }
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
    pub fn poll(&mut self) -> Result<()> {
        ensure!(self.valid, "HID session 已失效");
        self.v3_pump(0)
    }
    pub fn finish(&mut self, cancel: &Cancellation) -> Result<()> {
        self.v3_send(&input_protocol::v3::Command::Barrier, cancel)
    }
    pub fn capabilities(&self) -> Option<&input_protocol::v3::Capabilities> {
        self.binary.as_ref().map(|b| &b.capabilities)
    }
    pub fn hello(&mut self, cancel: &Cancellation) -> Result<Hello> {
        cancel.check()?;
        ensure!(self.valid, "HID session 已失效");
        let c = &self
            .binary
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("尚未 OPEN v3 session"))?
            .capabilities;
        Ok(Hello {
            protocol: 3,
            nkro: c.flags & 1 != 0,
            media: c.consumer != 0,
            firmware: c.firmware.clone(),
            lease_ms: c.lease_ms.into(),
            mouse: c.flags & 4 != 0,
            raw: format!(
                "protocol=3,board={},fw={},lease_ms={},flags={},consumer={:02X},device={:02X?}",
                c.board, c.firmware, c.lease_ms, c.flags, c.consumer, c.device_id
            ),
        })
    }
    pub fn status(&mut self, cancel: &Cancellation) -> Result<Status> {
        {
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
            Ok(Status {
                raw: format!("v3 {fields:?}"),
                fields,
            })
        }
    }
    pub fn reset(&mut self, cancel: &Cancellation) -> Result<()> {
        self.v3_send(&input_protocol::v3::Command::ReleaseAll, cancel)?;
        ensure!(
            self.status(cancel)?.released(),
            "本 session 釋放後仍有輸入持有"
        );
        Ok(())
    }
}

mod transport;
pub use transport::{enumerate, open};
mod v3;

#[cfg(test)]
pub(crate) mod tests;
