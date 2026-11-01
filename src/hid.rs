//! The tool's view of one vendor-HID session.
//!
//! The protocol itself (Feature 14, OPEN, sequences, reply matching, heartbeats,
//! reconnects) lives in the GDBOX SDK, so nothing here speaks the wire format.
//! This module is a thin adapter: it keeps the method names the worker, the
//! latency tester and the mouse feature already call, maps the SDK's STATUS into
//! the UI's `Status` shape, and turns SDK errors into actionable Chinese.

use crate::{
    cancel::Cancellation,
    protocol::{Hello, Status},
};
use anyhow::{Result, ensure};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use transport::sdk_error;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
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

/// One line of the packet log. The SDK logs every frame it sends or accepts at
/// `Level::Trace`; this is that stream, not a second reply collector.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ResponseEvent {
    pub unix_ms: u128,
    pub line: String,
}

/// Accepted frames waiting to be shown. The worker owns the only live session in
/// a process, and the SDK's log sink is process-wide, so one queue is enough.
static EVENTS: Mutex<VecDeque<ResponseEvent>> = Mutex::new(VecDeque::new());
/// Trace is reference counted: enabling it for one session must not disable the
/// sink another session installed, and the last release turns it off.
static TRACES: Mutex<usize> = Mutex::new(0);
const EVENT_LINES: usize = 512;

/// `false` (the default) drops heartbeat and successful-poll frames and never
/// prints a raw payload array: the log is for the person driving the tool, not a
/// packet dump.
static VERBOSE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Shows every frame, payload included. The SDK's own log sink has the same
/// information; this only decides what reaches the debug pane.
pub fn set_verbose(enabled: bool) {
    VERBOSE.store(enabled, std::sync::atomic::Ordering::Relaxed);
}

fn record(level: gdb::Level, line: &str) {
    if level != gdb::Level::Trace {
        return;
    }
    let verbose = VERBOSE.load(std::sync::atomic::Ordering::Relaxed);
    let text = match Frame::parse(line) {
        Some(frame) if verbose || !frame.is_noise() => frame.describe(),
        Some(_) => return,
        None => line.to_string(),
    };
    let Ok(mut events) = EVENTS.lock() else {
        return;
    };
    if events.len() >= EVENT_LINES {
        events.pop_front();
    }
    events.push_back(ResponseEvent {
        unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
        line: text,
    });
}

/// One SDK trace line (`out opcode=6 session=1 sequence=9 payload=[0, …]`),
/// turned into something a person can read: opcode name, direction, and only the
/// fields that matter for that opcode.
struct Frame {
    outgoing: bool,
    foreign: bool,
    opcode: u8,
    sequence: u32,
    payload: Vec<u8>,
}

impl Frame {
    fn parse(line: &str) -> Option<Self> {
        let mut words = line.split_whitespace();
        let direction = words.next()?;
        let outgoing = match direction {
            "out" => true,
            "in" => false,
            "skip" => true,
            _ => return None,
        };
        let mut opcode = None;
        let mut sequence = 0;
        let mut payload = Vec::new();
        for word in words {
            if let Some(value) = word.strip_prefix("opcode=") {
                opcode = value.parse().ok();
            } else if let Some(value) = word.strip_prefix("sequence=") {
                sequence = value.parse().unwrap_or(0);
            } else if let Some(value) = word.strip_prefix("payload=") {
                payload = parse_payload(value);
            }
        }
        Some(Self {
            outgoing,
            foreign: direction == "skip",
            opcode: opcode?,
            sequence,
            payload,
        })
    }

    /// Heartbeats and successful polling carry no information for the user: the
    /// charts already show those counters once a second.
    fn is_noise(&self) -> bool {
        let opcode = self.opcode & 0x7f;
        if self.foreign {
            return false;
        }
        match opcode {
            // Polling frames: the charts already show these counters once a
            // second, so they are not log lines. SESSION_LIST carries no result
            // byte, so it can never be judged by payload[0].
            input_protocol::v3::HEARTBEAT
            | input_protocol::v3::STATUS_EXT
            | input_protocol::v3::SESSION_LIST => true,
            input_protocol::v3::STATUS => self.payload.first().copied().unwrap_or(0) == 0,
            _ => false,
        }
    }

    fn describe(&self) -> String {
        let reply = self.opcode & 0x80 != 0;
        let opcode = self.opcode & 0x7f;
        if self.foreign {
            return format!("忽略別的 session 的封包（opcode {opcode:#04x}）");
        }
        let arrow = if self.outgoing { "→" } else { "←" };
        let name = opcode_name(opcode);
        let body = if self.outgoing {
            describe_out(opcode, &self.payload)
        } else {
            describe_in(opcode, &self.payload)
        };
        let result = if reply {
            format!("{arrow} {name} 回覆")
        } else {
            format!("{arrow} {name}")
        };
        if body.is_empty() {
            format!("{result}（seq {}）", self.sequence)
        } else {
            format!("{result}：{body}")
        }
    }
}

fn opcode_name(opcode: u8) -> &'static str {
    use input_protocol::v3;
    match opcode {
        v3::OPEN => "OPEN",
        v3::CLOSE => "CLOSE",
        v3::HEARTBEAT => "心跳",
        v3::BARRIER => "BARRIER",
        v3::STATUS => "STATUS",
        v3::STATUS_EXT => "STATUS_EXT",
        v3::SESSION_LIST => "SESSION_LIST",
        v3::BOOTLOADER => "進 BOOTSEL",
        v3::KEY => "按鍵",
        v3::SNAPSHOT => "快照",
        v3::MOUSE_MOVE => "滑鼠位移",
        v3::MOUSE_BUTTONS => "滑鼠鍵",
        v3::WHEEL => "滾輪",
        v3::EVENT => "裝置事件",
        _ => "未知命令",
    }
}

fn describe_out(opcode: u8, payload: &[u8]) -> String {
    use input_protocol::v3;
    match opcode {
        v3::KEY if payload.len() >= 5 => format!(
            "page {} usage {} {}",
            u16::from_le_bytes([payload[0], payload[1]]),
            u16::from_le_bytes([payload[2], payload[3]]),
            if payload[4] == 0 { "放開" } else { "按下" }
        ),
        v3::MOUSE_MOVE if payload.len() >= 4 => format!(
            "dx {} dy {}",
            i16::from_le_bytes([payload[0], payload[1]]),
            i16::from_le_bytes([payload[2], payload[3]])
        ),
        v3::WHEEL if payload.len() >= 4 => format!(
            "垂直 {} 水平 {}",
            i16::from_le_bytes([payload[0], payload[1]]),
            i16::from_le_bytes([payload[2], payload[3]])
        ),
        v3::MOUSE_BUTTONS if !payload.is_empty() => format!("按鍵位元 {:02X}", payload[0]),
        v3::SNAPSHOT if !payload.is_empty() => {
            let pressed = payload.iter().filter(|byte| **byte != 0).count();
            format!("{pressed} 個位元組有按鍵")
        }
        _ => String::new(),
    }
}

fn describe_in(opcode: u8, payload: &[u8]) -> String {
    use input_protocol::v3;
    if payload.is_empty() {
        return String::new();
    }
    let result = payload[0];
    if result != 0 && opcode != v3::SESSION_LIST {
        return format!("錯誤碼 {result}{}", fault_hint(result));
    }
    match opcode {
        v3::STATUS if payload.len() >= 20 => format!(
            "完成 {}/{} 待送 {} 租約 {} ms 失敗 rx {} tx {} hid {} fs {}",
            u32::from_le_bytes(payload[5..9].try_into().unwrap_or_default()),
            u32::from_le_bytes(payload[1..5].try_into().unwrap_or_default()),
            payload[9],
            u16::from_le_bytes([payload[10], payload[11]]),
            u16::from_le_bytes([payload[12], payload[13]]),
            u16::from_le_bytes([payload[14], payload[15]]),
            u16::from_le_bytes([payload[16], payload[17]]),
            u16::from_le_bytes([payload[18], payload[19]]),
        ),
        v3::STATUS_EXT if payload.len() >= 23 => format!(
            "開機 {} s，session {}，故障碼 {}，開機次數 {}，build {:08x}",
            u32::from_le_bytes(payload[1..5].try_into().unwrap_or_default()) / 1000,
            payload[10],
            payload[11],
            u16::from_le_bytes([payload[17], payload[18]]),
            u32::from_le_bytes(payload[19..23].try_into().unwrap_or_default()),
        ),
        v3::SESSION_LIST => {
            let owners = payload
                .chunks(6)
                .filter(|slot| slot.len() == 6 && slot[..2] != [0, 0])
                .count();
            format!("{owners} 個擁有者")
        }
        v3::EVENT => format!("故障碼 {}", payload[0]),
        _ => String::new(),
    }
}

fn fault_hint(code: u8) -> &'static str {
    match code {
        1 => "（framing）",
        2 => "（忙碌：同名或名額已滿）",
        3 => "（輸入無效）",
        5 => "（租約到期）",
        6 => "（HID 送出失敗）",
        7 => "（USB 斷線）",
        8 => "（溢位／輸出過期）",
        9 => "（序號重複）",
        10 => "（序號跳號）",
        _ => "",
    }
}

fn parse_payload(text: &str) -> Vec<u8> {
    text.trim_matches(|c| c == '[' || c == ']')
        .split(',')
        .filter_map(|item| item.trim().parse().ok())
        .collect()
}

pub struct Session {
    device: gdb::Device,
    valid: bool,
    traced: bool,
}

impl Session {
    /// Wraps a device the SDK already handshook and OPENed.
    pub fn new(device: gdb::Device) -> Self {
        Self {
            device,
            valid: true,
            traced: false,
        }
    }

    /// `gdb::Manager::open` reads Feature 14 and OPENs the session, so there is
    /// nothing left to negotiate here; this only honours a pending cancellation.
    pub fn synchronize(&mut self, cancel: &Cancellation) -> Result<()> {
        cancel.check()?;
        ensure!(self.valid, "HID session 已失效");
        Ok(())
    }

    pub fn is_valid(&self) -> bool {
        self.valid
    }

    /// Liveness check for the worker's idle loop. The SDK reads on demand and
    /// owns reconnects, so there is nothing to pump here; a transient disconnect
    /// during the SDK's own reopen must not tear the tool's session down, and the
    /// next real command reports the failure anyway.
    pub fn poll(&mut self) -> Result<()> {
        ensure!(self.valid, "HID session 已失效");
        Ok(())
    }

    fn ready(&self) -> Result<()> {
        ensure!(self.valid, "HID session 已失效");
        Ok(())
    }

    /// Turns the SDK's packet trace into `take_events()` lines. Off by default so
    /// game mode and the latency test do not pay for formatting every frame.
    pub fn set_trace(&mut self, enabled: bool) {
        if enabled == self.traced {
            return;
        }
        let Ok(mut traces) = TRACES.lock() else {
            return;
        };
        if enabled {
            if *traces == 0 {
                gdb::log::set_sink(Some(Box::new(record)));
                gdb::log::set_level(gdb::Level::Trace);
            }
            *traces += 1;
        } else {
            *traces = traces.saturating_sub(1);
            if *traces == 0 {
                gdb::log::set_sink(None);
                gdb::log::set_level(gdb::Level::Off);
            }
        }
        self.traced = enabled;
    }

    pub fn take_events(&mut self) -> Vec<ResponseEvent> {
        EVENTS
            .lock()
            .map(|mut events| events.drain(..).collect())
            .unwrap_or_default()
    }

    /// Identity and capability bits straight from the SDK's Feature 14 read.
    pub fn info(&self) -> Option<gdb::DeviceInfo> {
        self.valid.then(|| self.device.info())
    }

    pub fn hello(&mut self, cancel: &Cancellation) -> Result<Hello> {
        cancel.check()?;
        self.ready()?;
        let info = self.device.info();
        // Protocol 3.2.0 capability bits: bit0 NKRO, bit1 Consumer, bit2
        // relative mouse. `media` reports the capability; the count of consumer
        // usages is only in the raw Feature 14 report, which the SDK keeps.
        Ok(Hello {
            protocol: 3,
            nkro: info.capabilities & 1 != 0,
            media: info.capabilities & 2 != 0,
            firmware: info.firmware,
            lease_ms: info.lease_ms.into(),
            mouse: info.capabilities & 4 != 0,
        })
    }

    pub fn status(&mut self, cancel: &Cancellation) -> Result<Status> {
        cancel.check()?;
        self.ready()?;
        let status = self.device.status().map_err(sdk_error)?;
        let mut fields = BTreeMap::new();
        for (key, value) in [
            ("p", status.state.pressed().len() as u64),
            ("rx", status.rx.into()),
            ("tx", status.tx.into()),
            ("hid", status.hid.into()),
            ("fs", status.failsafe.into()),
            ("received", status.received.into()),
            ("completed", status.completed.into()),
            ("pending", status.pending.into()),
            ("lease", status.lease_ms.into()),
        ] {
            fields.insert(key.into(), value.to_string());
        }
        fields.insert("mb".into(), format!("{:02X}", status.state.buttons));
        fields.insert(
            "keys".into(),
            status
                .state
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

    pub fn status_ext(&mut self, cancel: &Cancellation) -> Result<input_protocol::v3::StatusExt> {
        cancel.check()?;
        self.ready()?;
        self.device.status_ext().map_err(sdk_error)
    }

    /// Who else is driving the keyboard right now.
    pub fn sessions(
        &mut self,
        cancel: &Cancellation,
    ) -> Result<Vec<input_protocol::v3::SessionInfo>> {
        cancel.check()?;
        self.ready()?;
        self.device.sessions().map_err(sdk_error)
    }

    /// Sends one command and hands back the reply, for the typed queries that
    /// parse a payload themselves.
    pub fn query(
        &mut self,
        command: input_protocol::v3::Command,
        cancel: &Cancellation,
    ) -> Result<input_protocol::v3::Packet> {
        cancel.check()?;
        self.ready()?;
        self.device
            .command(&command)
            .map_err(sdk_error)?
            .ok_or_else(|| anyhow::anyhow!("命令沒有回應"))
    }

    /// Any command, including input ones (the wire protocol's own escape hatch).
    pub fn input(
        &mut self,
        command: input_protocol::v3::Command,
        cancel: &Cancellation,
    ) -> Result<()> {
        cancel.check()?;
        self.ready()?;
        self.device.command(&command).map(|_| ()).map_err(sdk_error)
    }

    /// USB barrier: returns once the device has queued everything sent so far.
    pub fn finish(&mut self, cancel: &Cancellation) -> Result<()> {
        cancel.check()?;
        self.ready()?;
        self.device.barrier().map_err(sdk_error)
    }

    /// Gives our owner slot back. Frees it immediately instead of waiting for the
    /// lease and is not a fault, so the next program does not have to wait.
    pub fn close(&mut self, cancel: &Cancellation) -> Result<()> {
        let result = if self.valid {
            cancel
                .check()
                .and_then(|()| self.device.close().map_err(sdk_error))
        } else {
            Ok(())
        };
        self.valid = false;
        result
    }

    pub fn reset(&mut self, cancel: &Cancellation) -> Result<()> {
        cancel.check()?;
        self.ready()?;
        self.device.release_all().map_err(sdk_error)?;
        ensure!(
            self.status(cancel)?.released(),
            "本 session 釋放後仍有輸入持有"
        );
        Ok(())
    }

    pub fn key(
        &mut self,
        key: input_protocol::KeyId,
        down: bool,
        cancel: &Cancellation,
    ) -> Result<()> {
        cancel.check()?;
        self.ready()?;
        let keyboard = self.device.keyboard();
        let result = if down {
            keyboard.key_down(key.usage_page, key.usage)
        } else {
            keyboard.key_up(key.usage_page, key.usage)
        };
        result.map_err(sdk_error)
    }

    pub fn set_button(&mut self, button: u8, down: bool, cancel: &Cancellation) -> Result<()> {
        cancel.check()?;
        self.ready()?;
        ensure!((1..=5).contains(&button), "無效滑鼠按鈕");
        let button = gdb::Button::from_u8(button).map_err(sdk_error)?;
        self.device
            .mouse()
            .button(button, down)
            .map_err(sdk_error)?;
        self.finish(cancel)
    }

    /// One relative mouse frame; the caller barriers when it needs an ack.
    pub fn mouse_move(&mut self, dx: i32, dy: i32, cancel: &Cancellation) -> Result<()> {
        cancel.check()?;
        self.ready()?;
        self.device.mouse().move_raw(dx, dy).map_err(sdk_error)
    }

    /// One wheel notch. The caller splits anything larger than ±1024.
    pub fn wheel(&mut self, vertical: i32, horizontal: i32, cancel: &Cancellation) -> Result<()> {
        cancel.check()?;
        self.ready()?;
        self.device
            .mouse()
            .wheel(vertical, horizontal)
            .map_err(sdk_error)
    }

    /// Waits in slices so cancellation is honoured between device calls. A single
    /// SDK call below is not interruptible: it blocks up to its 1000 ms timeout.
    pub fn wait_until(&mut self, deadline: Instant, cancel: &Cancellation) -> Result<()> {
        crate::platform::wait_until(deadline, cancel)
    }

    /// The SDK keeps the 500 ms lease heartbeat on its own thread, so the worker
    /// must not send a second one on the same session.
    pub fn needs_heartbeat(&self, _held: bool) -> bool {
        false
    }

    pub fn heartbeat_interval(&self) -> Duration {
        Duration::from_millis(500)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.set_trace(false);
        // CLOSE is a normal teardown verb, not a fault: it hands the owner slot
        // back now instead of after the 2 s lease. Best effort by definition.
        let _ = self.device.close();
    }
}

mod transport;
pub use transport::{enumerate, open, open_sdk};

#[cfg(test)]
pub(crate) mod tests;
