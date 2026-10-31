//! Unified error codes. The numeric values are part of the C ABI and of every
//! binding, so they must never be reordered.

use std::fmt;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorCode {
    Ok = 0,
    Invalid = 1,
    Timeout = 2,
    Disconnected = 3,
    Busy = 4,
    Lease = 5,
    Framing = 6,
    Overflow = 7,
    Unsupported = 8,
    Cancelled = 9,
    Io = 10,
}

impl ErrorCode {
    pub fn as_u8(self) -> u8 {
        self as u8
    }
    pub fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Ok,
            1 => Self::Invalid,
            2 => Self::Timeout,
            3 => Self::Disconnected,
            4 => Self::Busy,
            5 => Self::Lease,
            6 => Self::Framing,
            7 => Self::Overflow,
            8 => Self::Unsupported,
            9 => Self::Cancelled,
            _ => Self::Io,
        }
    }
    pub fn message(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Invalid => "invalid argument",
            Self::Timeout => "device did not answer in time",
            Self::Disconnected => "device disconnected",
            Self::Busy => "device busy: same session id or all eight owners taken",
            Self::Lease => "session lease expired",
            Self::Framing => "device rejected the frame",
            Self::Overflow => "device queue overflow or stale output",
            Self::Unsupported => "not supported on this platform or firmware",
            Self::Cancelled => "cancelled",
            Self::Io => "HID transfer failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
}

impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
    pub fn code(&self) -> ErrorCode {
        self.code
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Invalid, message)
    }
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unsupported, message)
    }
    pub fn cancelled() -> Self {
        Self::new(ErrorCode::Cancelled, ErrorCode::Cancelled.message())
    }
    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Timeout, message)
    }
    pub fn disconnected(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Disconnected, message)
    }
    pub fn io(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Io, message)
    }

    /// Maps the protocol `result` byte onto the SDK code space.
    pub fn from_result(code: u8) -> Self {
        let mapped = match code {
            0 => ErrorCode::Ok,
            1 => ErrorCode::Framing,
            2 => ErrorCode::Busy,
            3 => ErrorCode::Invalid,
            5 => ErrorCode::Lease,
            6 | 7 => ErrorCode::Disconnected,
            8 => ErrorCode::Overflow,
            9 | 10 => ErrorCode::Framing,
            _ => ErrorCode::Io,
        };
        let message = match code {
            1 => "device reported a framing error".to_string(),
            2 => ErrorCode::Busy.message().to_string(),
            3 => "device rejected the input".to_string(),
            5 => ErrorCode::Lease.message().to_string(),
            6 => "device failed to send the HID report".to_string(),
            7 => ErrorCode::Disconnected.message().to_string(),
            8 => ErrorCode::Overflow.message().to_string(),
            9 => "duplicate sequence".to_string(),
            10 => "sequence gap".to_string(),
            other => format!("device error code {other}"),
        };
        Self::new(mapped, message)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.message, self.code.as_u8())
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        let code = match error.kind() {
            std::io::ErrorKind::NotFound => ErrorCode::Disconnected,
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => ErrorCode::Timeout,
            _ => ErrorCode::Io,
        };
        Self::new(code, error.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_codes_map_to_stable_sdk_codes() {
        assert_eq!(Error::from_result(0).code(), ErrorCode::Ok);
        assert_eq!(Error::from_result(2).code(), ErrorCode::Busy);
        assert_eq!(Error::from_result(5).code(), ErrorCode::Lease);
        assert_eq!(Error::from_result(8).code(), ErrorCode::Overflow);
        assert_eq!(Error::from_result(7).code(), ErrorCode::Disconnected);
        assert_eq!(Error::from_result(200).code(), ErrorCode::Io);
    }

    #[test]
    fn numeric_codes_are_the_abi() {
        assert_eq!(ErrorCode::Ok.as_u8(), 0);
        assert_eq!(ErrorCode::Timeout.as_u8(), 2);
        assert_eq!(ErrorCode::Cancelled.as_u8(), 9);
        assert_eq!(ErrorCode::Io.as_u8(), 10);
    }
}
