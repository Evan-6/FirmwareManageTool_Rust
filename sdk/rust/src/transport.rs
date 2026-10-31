//! Raw 64-byte HID reports. The SDK never assumes anything above this layer.

use crate::error::{Error, Result};

pub trait Transport: Send {
    /// GET_FEATURE(14): the 64-byte report with the report id in byte 0.
    fn feature(&mut self) -> Result<Option<[u8; 64]>>;
    /// One output report (id 12 in byte 0). A short write is an error.
    fn write(&mut self, report: &[u8; 64]) -> Result<()>;
    /// One input report, or `None` when the timeout expired.
    fn read(&mut self, timeout_ms: i32) -> Result<Option<[u8; 64]>>;
}

impl Transport for Box<dyn Transport> {
    fn feature(&mut self) -> Result<Option<[u8; 64]>> {
        (**self).feature()
    }
    fn write(&mut self, report: &[u8; 64]) -> Result<()> {
        (**self).write(report)
    }
    fn read(&mut self, timeout_ms: i32) -> Result<Option<[u8; 64]>> {
        (**self).read(timeout_ms)
    }
}

#[cfg(windows)]
pub struct HidTransport {
    handle: hidapi::HidDevice,
}

#[cfg(windows)]
impl HidTransport {
    pub fn open(path: &str) -> Result<Self> {
        let api = hidapi::HidApi::new().map_err(|error| Error::io(error.to_string()))?;
        let path = std::ffi::CString::new(path)
            .map_err(|_| Error::invalid("HID path contains a NUL byte"))?;
        let handle = api.open_path(path.as_c_str()).map_err(|error| {
            Error::new(crate::error::ErrorCode::Disconnected, error.to_string())
        })?;
        Ok(Self { handle })
    }
}

#[cfg(windows)]
impl Transport for HidTransport {
    fn feature(&mut self) -> Result<Option<[u8; 64]>> {
        let mut report = [0u8; 64];
        report[0] = 14;
        match self.handle.get_feature_report(&mut report) {
            Ok(64) => Ok(Some(report)),
            Ok(other) => Err(Error::new(
                crate::error::ErrorCode::Framing,
                format!("feature report was {other} bytes, expected 64"),
            )),
            Err(_) => Ok(None),
        }
    }

    fn write(&mut self, report: &[u8; 64]) -> Result<()> {
        match self.handle.write(report) {
            Ok(64) => Ok(()),
            Ok(other) => Err(Error::new(
                crate::error::ErrorCode::Io,
                format!("short HID write: {other}/64"),
            )),
            Err(error) => Err(Error::new(
                crate::error::ErrorCode::Disconnected,
                error.to_string(),
            )),
        }
    }

    fn read(&mut self, timeout_ms: i32) -> Result<Option<[u8; 64]>> {
        let mut report = [0u8; 64];
        match self.handle.read_timeout(&mut report, timeout_ms) {
            Ok(0) => Ok(None),
            Ok(64) => Ok(Some(report)),
            Ok(other) => Err(Error::new(
                crate::error::ErrorCode::Framing,
                format!("short HID read: {other}/64"),
            )),
            Err(error) => Err(Error::new(
                crate::error::ErrorCode::Disconnected,
                error.to_string(),
            )),
        }
    }
}
