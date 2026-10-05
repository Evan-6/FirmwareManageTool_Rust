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
    fn write(&mut self, report: &[u8]) -> Result<usize>;
    fn read_timeout(&mut self, buffer: &mut [u8], timeout_ms: i32) -> Result<usize>;
}
impl<T: HidTransport + ?Sized> HidTransport for Box<T> {
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
        }
    }
    /// A new connection releases input before hello. Old ACKs cannot satisfy reset/status.
    pub fn synchronize(&mut self, cancel: &Cancellation) -> Result<()> {
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
    pub fn hello(&mut self, cancel: &Cancellation) -> Result<Hello> {
        Hello::parse(&self.command("hello", "ok:hello,", cancel)?)
    }
    pub fn status(&mut self, cancel: &Cancellation) -> Result<Status> {
        Status::parse(&self.command("status", "ok:status,", cancel)?)
    }
    pub fn reset(&mut self, cancel: &Cancellation) -> Result<()> {
        self.command("reset", "ok:release_all", cancel)?;
        ensure!(self.status(cancel)?.released(), "reset 後仍有按鍵按住");
        Ok(())
    }
}

#[cfg(windows)]
struct NativeTransport(hidapi::HidDevice);
#[cfg(windows)]
impl HidTransport for NativeTransport {
    fn write(&mut self, report: &[u8]) -> Result<usize> {
        Ok(self.0.write(report)?)
    }
    fn read_timeout(&mut self, buffer: &mut [u8], timeout_ms: i32) -> Result<usize> {
        Ok(self.0.read_timeout(buffer, timeout_ms)?)
    }
}
pub fn enumerate() -> Result<Vec<Device>> {
    #[cfg(windows)]
    {
        let api = hidapi::HidApi::new()?;
        Ok(api
            .device_list()
            .filter(|d| {
                protocol::IDENTITIES.contains(&(d.vendor_id(), d.product_id()))
                    && d.usage_page() == protocol::USAGE_PAGE
                    && d.usage() == protocol::USAGE
            })
            .map(|d| Device {
                path: d.path().to_string_lossy().into_owned(),
                serial: d.serial_number().unwrap_or("").into(),
                product: d.product_string().unwrap_or("Vendor HID").into(),
                vid: d.vendor_id(),
                pid: d.product_id(),
            })
            .collect())
    }
    #[cfg(not(windows))]
    {
        bail!("實體 HID 功能僅支援 Windows 11 x64")
    }
}
pub fn open(device: &Device) -> Result<NativeSession> {
    #[cfg(windows)]
    {
        let path = std::ffi::CString::new(device.path.as_str())?;
        let api = hidapi::HidApi::new()?;
        let handle = api.open_path(&path)?;
        Ok(Session::new(Box::new(NativeTransport(handle))))
    }
    #[cfg(not(windows))]
    {
        let _ = device;
        bail!("實體 HID 功能僅支援 Windows 11 x64")
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub struct Fake {
        pub reads: VecDeque<Vec<u8>>,
        pub writes: Vec<Vec<u8>>,
        pub short: bool,
        pub reads_done: usize,
    }
    impl Fake {
        pub fn lines(lines: &[&str]) -> Self {
            Self {
                reads: lines
                    .iter()
                    .map(|line| {
                        let mut r = vec![protocol::IN_ID];
                        r.extend_from_slice(format!("{line}\n").as_bytes());
                        r
                    })
                    .collect(),
                writes: Vec::new(),
                short: false,
                reads_done: 0,
            }
        }
    }
    impl HidTransport for Fake {
        fn write(&mut self, report: &[u8]) -> Result<usize> {
            self.writes.push(report.to_vec());
            Ok(if self.short { 1 } else { report.len() })
        }
        fn read_timeout(&mut self, buffer: &mut [u8], timeout: i32) -> Result<usize> {
            if self.reads_done >= self.writes.len() {
                return Ok(0);
            }
            if let Some(report) = self.reads.pop_front() {
                self.reads_done += 1;
                buffer[..report.len()].copy_from_slice(&report);
                Ok(report.len())
            } else {
                std::thread::sleep(Duration::from_millis(timeout.max(0) as u64));
                Ok(0)
            }
        }
    }
    #[test]
    fn short_write_invalidates() {
        let mut fake = Fake::lines(&[]);
        fake.short = true;
        let mut s = Session::new(fake);
        assert!(s.queue("ping", "pong", &Cancellation::default()).is_err());
        assert!(!s.is_valid());
    }
    #[test]
    fn final_error_and_failsafe_are_preserved() {
        for line in ["err:hid_send_failed", "warn:failsafe_release_all"] {
            let mut s = Session::new(Fake::lines(&[line]));
            assert!(s.command("ping", "pong", &Cancellation::default()).is_err());
            assert_eq!(s.take_events()[0].line, line);
            assert!(!s.is_valid());
        }
    }
    #[test]
    fn ordered_ack_and_last_drain() {
        let mut s = Session::new(Fake::lines(&["ok:mouse_abs", "ok:mouse_abs"]));
        let c = Cancellation::default();
        s.queue("mouse_abs:1,1", "ok:mouse_abs", &c).unwrap();
        s.queue("mouse_abs:2,2", "ok:mouse_abs", &c).unwrap();
        s.finish(&c).unwrap();
        assert_eq!(s.take_events().len(), 2);
    }
    #[test]
    fn timeout_and_cancel_are_bounded() {
        let mut s = Session::new(Fake::lines(&[]));
        let c = Cancellation::default();
        s.queue("ping", "pong", &c).unwrap();
        c.cancel();
        assert!(s.finish(&c).is_err());
        assert!(!s.is_valid());
        let mut s = Session::new(Fake::lines(&[]));
        let start = Instant::now();
        assert!(s.command("ping", "pong", &Cancellation::default()).is_err());
        assert!(start.elapsed() < Duration::from_secs(2));
    }
    struct Fragmented {
        writes: usize,
        reads: usize,
    }
    impl HidTransport for Fragmented {
        fn write(&mut self, report: &[u8]) -> Result<usize> {
            self.writes += 1;
            Ok(report.len())
        }
        fn read_timeout(&mut self, buffer: &mut [u8], _: i32) -> Result<usize> {
            let data: &[u8] = if self.writes == 1 && self.reads == 0 {
                self.reads += 1;
                b"\x0bok:mo"
            } else if self.writes == 2 && self.reads == 1 {
                self.reads += 1;
                b"\x0buse_abs\r\nok:mouse_abs\n"
            } else {
                return Ok(0);
            };
            buffer[..data.len()].copy_from_slice(data);
            Ok(data.len())
        }
    }
    #[test]
    fn session_handles_split_and_combined_delayed_ack() {
        let mut s = Session::new(Fragmented {
            writes: 0,
            reads: 0,
        });
        let token = Cancellation::default();
        s.queue("mouse_abs:1,1", "ok:mouse_abs", &token).unwrap();
        s.queue("mouse_abs:2,2", "ok:mouse_abs", &token).unwrap();
        s.finish(&token).unwrap();
        assert_eq!(
            s.take_events()
                .iter()
                .map(|e| e.line.as_str())
                .collect::<Vec<_>>(),
            ["ok:mouse_abs", "ok:mouse_abs"]
        );
    }
    struct Disconnected;
    impl HidTransport for Disconnected {
        fn write(&mut self, _: &[u8]) -> Result<usize> {
            Ok(64)
        }
        fn read_timeout(&mut self, _: &mut [u8], _: i32) -> Result<usize> {
            anyhow::bail!("device disconnected")
        }
    }
    #[test]
    fn disconnect_invalidates_and_a_fresh_connection_resets() {
        let token = Cancellation::default();
        let mut old = Session::new(Disconnected);
        assert!(old.command("ping", "pong", &token).is_err());
        assert!(!old.is_valid());
        let mut fresh = Session::new(Fake::lines(&[
            "ok:release_all",
            "ok:status,p=0,tx=0,hid=0,fs=0,mb=00",
        ]));
        fresh.reset(&token).unwrap();
        assert!(fresh.is_valid());
    }
    struct Startup {
        reads: VecDeque<Vec<u8>>,
    }
    impl HidTransport for Startup {
        fn write(&mut self, report: &[u8]) -> Result<usize> {
            let line = if report[1..].starts_with(b"reset") {
                b"\x0bok:mouse_abs\nok:release_all\n".as_slice()
            } else {
                b"\x0bok:status,p=0,tx=0,hid=0,fs=0,mb=00\n".as_slice()
            };
            self.reads.push_back(line.to_vec());
            Ok(report.len())
        }
        fn read_timeout(&mut self, buffer: &mut [u8], _: i32) -> Result<usize> {
            if let Some(report) = self.reads.pop_front() {
                buffer[..report.len()].copy_from_slice(&report);
                Ok(report.len())
            } else {
                Ok(0)
            }
        }
    }
    #[test]
    fn reconnect_discards_old_fragments_and_does_not_mistake_move_ack_for_reset() {
        let mut session = Session::new(Startup {
            reads: VecDeque::from([b"\x0bold partial".to_vec()]),
        });
        session.synchronize(&Cancellation::default()).unwrap();
        assert!(session.is_valid());
        let events = session.take_events();
        assert!(events.iter().any(|e| e.line == "ok:mouse_abs"));
        assert!(events.iter().any(|e| e.line == "ok:release_all"));
    }
}
