use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const REPORT_SIZE: usize = 64;
pub const OUT_ID: u8 = 10;
pub const IN_ID: u8 = 11;
pub const IDENTITIES: [(u16, u16); 2] = [(0x03f0, 0x0024), (0x6666, 0x6666)];
pub const USAGE_PAGE: u16 = 0xff60;
pub const USAGE: u16 = 0x61;

pub fn encode(command: &str) -> Result<Vec<[u8; REPORT_SIZE]>> {
    ensure!(
        !command.is_empty() && command.len() <= 127,
        "指令須為 1–127 個 ASCII 字元"
    );
    ensure!(
        command.bytes().all(|b| (0x20..=0x7e).contains(&b)),
        "指令含非可見 ASCII 字元"
    );
    let line = format!("{command}\n");
    Ok(line
        .as_bytes()
        .chunks(63)
        .map(|chunk| {
            let mut report = [0; REPORT_SIZE];
            report[0] = OUT_ID;
            report[1..1 + chunk.len()].copy_from_slice(chunk);
            report
        })
        .collect())
}

#[derive(Default)]
pub struct Decoder {
    partial: Vec<u8>,
}
impl Decoder {
    pub fn feed(&mut self, report: &[u8]) -> Result<Vec<String>> {
        ensure!(
            !report.is_empty() && report[0] == IN_ID,
            "不正確的 HID IN report id"
        );
        let mut lines = Vec::new();
        for &byte in &report[1..] {
            match byte {
                0 => {}
                b'\r' | b'\n' => {
                    if !self.partial.is_empty() {
                        lines.push(String::from_utf8(std::mem::take(&mut self.partial))?);
                    }
                }
                0x20..=0x7e => {
                    ensure!(self.partial.len() < 1024, "回應超過 1024 bytes");
                    self.partial.push(byte);
                }
                _ => bail!("回應含非法 ASCII byte: {byte:#x}"),
            }
        }
        Ok(lines)
    }
}

pub fn fields(line: &str, prefix: &str) -> Result<BTreeMap<String, String>> {
    ensure!(line.starts_with(prefix), "非預期回應：{line}");
    let mut map = BTreeMap::new();
    for field in line[prefix.len()..]
        .trim_start_matches(',')
        .split(',')
        .filter(|s| !s.is_empty())
    {
        let (key, value) = field
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("無效欄位：{field}"))?;
        map.insert(key.to_owned(), value.to_owned());
    }
    Ok(map)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: u8,
    pub nkro: bool,
    pub media: bool,
    pub firmware: String,
    pub lease_ms: u64,
    pub mouse: bool,
    pub raw: String,
}
impl Hello {
    pub fn parse(line: &str) -> Result<Self> {
        let map = fields(line, "ok:hello,")?;
        ensure!(
            map.get("protocol").map(String::as_str) == Some("2"),
            "裝置不支援 protocol=2"
        );
        let firmware = map
            .get("fw")
            .ok_or_else(|| anyhow::anyhow!("hello 缺少 fw"))?
            .clone();
        let lease_ms = map
            .get("lease_ms")
            .ok_or_else(|| anyhow::anyhow!("hello 缺少 lease_ms"))?
            .parse()?;
        ensure!(lease_ms > 5000, "裝置租約短於心跳週期");
        Ok(Self {
            protocol: 2,
            nkro: false,
            media: false,
            firmware,
            lease_ms,
            mouse: map.get("mouse").map(String::as_str) == Some("1"),
            raw: line.into(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub fields: BTreeMap<String, String>,
    pub raw: String,
}
impl Status {
    pub fn parse(line: &str) -> Result<Self> {
        let fields = fields(line, "ok:status,")?;
        for name in ["p", "tx", "hid", "fs"] {
            fields
                .get(name)
                .ok_or_else(|| anyhow::anyhow!("status 缺少 {name}"))?
                .parse::<u64>()?;
        }
        u8::from_str_radix(
            fields
                .get("mb")
                .ok_or_else(|| anyhow::anyhow!("status 缺少 mb"))?,
            16,
        )?;
        Ok(Self {
            fields,
            raw: line.into(),
        })
    }
    pub fn count(&self, name: &str) -> u64 {
        self.fields
            .get(name)
            .and_then(|n| n.parse().ok())
            .unwrap_or(0)
    }
    pub fn buttons(&self) -> u8 {
        self.fields
            .get("mb")
            .and_then(|n| u8::from_str_radix(n, 16).ok())
            .unwrap_or(0)
    }
    pub fn released(&self) -> bool {
        self.count("p") == 0 && self.buttons() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encoding_boundaries() {
        assert_eq!(encode(&"a".repeat(127)).unwrap().len(), 3);
        assert!(encode(&"a".repeat(128)).is_err());
        assert!(encode("reset\nmouse_button:1,down").is_err());
        assert!(encode("中文").is_err());
        assert_eq!(encode("hello").unwrap()[0][0], OUT_ID);
    }
    #[test]
    fn split_combined_and_crlf() {
        let mut decoder = Decoder::default();
        assert!(decoder.feed(b"\x0bok:hel").unwrap().is_empty());
        assert_eq!(
            decoder.feed(b"\x0blo\r\npong\nok:up\r\0").unwrap(),
            ["ok:hello", "pong", "ok:up"]
        );
        assert!(decoder.feed(b"\x0abad").is_err());
    }
    #[test]
    fn handshake_and_status() {
        assert!(Hello::parse("ok:hello,protocol=3,fw=2,lease_ms=30000").is_err());
        assert!(
            Hello::parse("ok:hello,protocol=2,fw=2,lease_ms=30000,mouse=1")
                .unwrap()
                .mouse
        );
        assert!(
            Status::parse("ok:status,p=0,tx=0,hid=0,fs=0,mb=00")
                .unwrap()
                .released()
        );
    }
}
