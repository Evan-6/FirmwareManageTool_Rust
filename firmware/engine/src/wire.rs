//! Wire framing: the 63-byte payload that sits behind HID report id 12 (host to
//! device) and 13 (device to host), plus the Feature 14 capability block. The
//! header carries the session id and sequence; the device keeps up to eight of
//! them alive at once.

use crate::state::keyboard_supported;

pub const OPEN: u8 = 1;
pub const HEARTBEAT: u8 = 2;
pub const STATUS: u8 = 3;
pub const BARRIER: u8 = 4;
pub const RELEASE: u8 = 5;
pub const BOOTLOADER: u8 = 6;
/// Explicit teardown: frees the owner slot without counting a failsafe.
pub const CLOSE: u8 = 7;
/// Fixed 24-byte diagnostic block (uptime, queue depths, last fault, reset info).
pub const STATUS_EXT: u8 = 8;
/// Echo up to 8 bytes so the host can measure a true device round trip.
pub const PING: u8 = 9;
/// Persisted device settings (LED brightness, lifetime counters).
pub const SETTINGS_GET: u8 = 10;
pub const SETTINGS_SET: u8 = 11;
/// Per-session snapshot: eight fixed entries of six bytes.
pub const SESSION_LIST: u8 = 15;
pub const SESSION_ENTRY: usize = 6;
pub const SESSION_LIST_BYTES: usize = SESSION_ENTRY * crate::config::SESSIONS;
// 12..=14 stay reserved for a future in-application firmware update; the ROM
// bootloader flow owns updates today, so nothing here claims those opcodes yet.
pub const KEY: u8 = 16;
pub const SNAPSHOT: u8 = 17;
pub const MOVE: u8 = 32;
pub const ABSOLUTE: u8 = 33;
pub const BUTTONS: u8 = 34;
pub const WHEEL: u8 = 35;
pub const EVENT: u8 = 127;

pub const KEYBOARD_REPORT_ID: u8 = 2;
pub const CONSUMER_REPORT_ID: u8 = 5;
pub const MOUSE_REPORT_ID: u8 = 3;

pub const VERSION: u8 = 3;
pub const MAX_PAYLOAD: u8 = 51;
/// Version, opcode, length, flags, session, sequence.
pub const HEADER: usize = 12;
pub const REPORT_BYTES: usize = 63;

pub struct DeviceInfo {
    pub board: u8,
    pub id: [u8; 8],
    /// First four bytes of the git commit the image was built from, little endian.
    /// Zero when the build did not record one, so the host can tell "unknown".
    pub build: u32,
}

/// Fixed size of the STATUS_EXT payload.
pub const EXT_BYTES: usize = 24;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Report {
    pub bytes: [u8; REPORT_BYTES],
}

impl Default for Report {
    fn default() -> Self {
        Self {
            bytes: [0; REPORT_BYTES],
        }
    }
}

impl Report {
    pub fn opcode(&self) -> u8 {
        self.bytes[1]
    }
    pub fn payload(&self) -> u8 {
        self.bytes[2]
    }
    pub fn session(&self) -> u32 {
        read32(&self.bytes[4..])
    }
    pub fn sequence(&self) -> u32 {
        read32(&self.bytes[8..])
    }
    pub fn data(&self) -> &[u8] {
        &self.bytes[HEADER..]
    }
}

pub fn read16(p: &[u8]) -> u16 {
    u16::from_le_bytes([p[0], p[1]])
}

pub fn read32(p: &[u8]) -> u32 {
    u32::from_le_bytes([p[0], p[1], p[2], p[3]])
}

pub fn write16(p: &mut [u8], v: u16) {
    p[..2].copy_from_slice(&v.to_le_bytes());
}

pub fn write32(p: &mut [u8], v: u32) {
    p[..4].copy_from_slice(&v.to_le_bytes());
}

/// Payload length required by each opcode; `None` means the opcode is unknown.
fn shape(op: u8, n: u8) -> Option<bool> {
    let expected = match op {
        OPEN | HEARTBEAT | STATUS | BARRIER | RELEASE | BOOTLOADER | CLOSE | STATUS_EXT => n == 0,
        SETTINGS_GET | SESSION_LIST => n == 0,
        SETTINGS_SET => n == 4,
        PING => n <= 8,
        KEY => n == 5,
        SNAPSHOT => n == 31,
        MOVE | ABSOLUTE | WHEEL => n == 4,
        BUTTONS => n == 1,
        _ => return None,
    };
    Some(expected)
}

/// Rejects anything the dispatcher must not interpret: wrong version, flags,
/// length, opcode shape, or non-zero trailing bytes.
pub fn valid(report: &Report) -> bool {
    let p = &report.bytes;
    let n = p[2];
    if p[0] != VERSION || p[3] != 0 || n > MAX_PAYLOAD || shape(p[1], n) != Some(true) {
        return false;
    }
    p[HEADER + n as usize..].iter().all(|&b| b == 0)
}

pub fn reply(op: u8, session: u32, seq: u32, data: &[u8]) -> Report {
    let mut report = Report::default();
    report.bytes[0] = VERSION;
    report.bytes[1] = op;
    report.bytes[2] = data.len() as u8;
    write32(&mut report.bytes[4..], session);
    write32(&mut report.bytes[8..], seq);
    report.bytes[HEADER..HEADER + data.len()].copy_from_slice(data);
    report
}

/// Feature 14 payload. Firmware version is fixed at 3.1.0, capabilities 23,
/// lease 2000 ms, one poll per millisecond.
pub fn feature(info: &DeviceInfo) -> [u8; REPORT_BYTES] {
    let mut b = [0u8; REPORT_BYTES];
    b[..4].copy_from_slice(b"FMT3");
    b[4] = VERSION;
    b[5] = info.board;
    b[6] = 3;
    b[7] = 1;
    write16(&mut b[9..], 2000);
    b[13] = 1;
    write32(&mut b[14..], 23);
    for usage in 4..crate::state::KEYBOARD_END {
        if keyboard_supported(usage) {
            b[18 + usage as usize / 8] |= 1 << (usage % 8);
        }
    }
    b[46] = 255;
    b[47] = 127;
    b[48..56].copy_from_slice(&info.id);
    // Build identity lives in the spare tail so the 63-byte frame is unchanged.
    write32(&mut b[56..], info.build);
    b
}
