//! Enumeration and connection entry points. A selector is a USB serial number
//! or a HID path; `None` picks the first matching device.

use crate::device::{Device, DeviceOptions, Target};
use crate::error::{Error, ErrorCode, Result};
use crate::transport::Transport;
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceEntry {
    pub path: String,
    pub serial: String,
    pub product: String,
    pub manufacturer: String,
    pub vid: u16,
    pub pid: u16,
}

pub struct Manager;

impl Manager {
    pub fn list() -> Result<Vec<DeviceEntry>> {
        entries()
    }

    pub fn open(selector: Option<&str>, options: DeviceOptions) -> Result<Device> {
        let devices = entries()?;
        let entry = match selector {
            Some(wanted) => devices
                .iter()
                .find(|entry| entry.serial == wanted || entry.path == wanted)
                .or_else(|| devices.iter().find(|entry| entry.serial.contains(wanted)))
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::Disconnected,
                        format!("no GDBOX device matches {wanted:?}"),
                    )
                })?,
            None => devices
                .first()
                .ok_or_else(|| Error::new(ErrorCode::Disconnected, "no GDBOX device found"))?,
        };
        let transport = open_transport(&Target::Path(entry.path.clone()))?;
        Device::from_transport(
            transport,
            Target::Serial(entry.serial.clone()),
            entry.serial.clone(),
            entry.product.clone(),
            options,
        )
    }

    /// A device backed by the in-memory mock; used by every binding's test kit.
    pub fn open_mock(options: DeviceOptions) -> Result<Device> {
        let (transport, _handle) = crate::mock::MockTransport::new();
        Device::from_transport(
            Box::new(transport),
            Target::Mock,
            "MOCK".to_string(),
            "GDBOX mock".to_string(),
            options,
        )
    }

    pub fn open_mock_with_handle(
        options: DeviceOptions,
    ) -> Result<(Device, crate::mock::MockHandle)> {
        let (transport, handle) = crate::mock::MockTransport::new();
        let device = Device::from_transport(
            Box::new(transport),
            Target::Mock,
            "MOCK".to_string(),
            "GDBOX mock".to_string(),
            options,
        )?;
        Ok((device, handle))
    }
}

pub(crate) fn open_transport(target: &Target) -> Result<Box<dyn Transport>> {
    match target {
        Target::Mock => {
            let (transport, _handle) = crate::mock::MockTransport::new();
            Ok(Box::new(transport))
        }
        Target::Path(path) => open_path(path),
        Target::Serial(serial) => transport_for_serial(serial),
    }
}

#[cfg(windows)]
fn open_path(path: &str) -> Result<Box<dyn Transport>> {
    Ok(Box::new(crate::transport::HidTransport::open(path)?))
}

#[cfg(not(windows))]
fn open_path(_path: &str) -> Result<Box<dyn Transport>> {
    Err(Error::unsupported(
        "physical HID is only implemented on Windows",
    ))
}

#[cfg(windows)]
fn entries() -> Result<Vec<DeviceEntry>> {
    use crate::{PRODUCT_ID, USAGE, USAGE_PAGE, VENDOR_ID};
    let api = hidapi::HidApi::new().map_err(|error| Error::io(error.to_string()))?;
    Ok(api
        .device_list()
        .filter(|device| {
            (device.vendor_id(), device.product_id()) == (VENDOR_ID, PRODUCT_ID)
                && device.usage_page() == USAGE_PAGE
                && device.usage() == USAGE
        })
        .map(|device| DeviceEntry {
            path: device.path().to_string_lossy().into_owned(),
            serial: device.serial_number().unwrap_or_default().to_string(),
            product: device.product_string().unwrap_or("Vendor HID").to_string(),
            manufacturer: device.manufacturer_string().unwrap_or_default().to_string(),
            vid: device.vendor_id(),
            pid: device.product_id(),
        })
        .collect())
}

#[cfg(not(windows))]
fn entries() -> Result<Vec<DeviceEntry>> {
    Err(Error::unsupported(
        "physical HID is only implemented on Windows",
    ))
}

/// Reconnect needs a transport for a serial target; the path is resolved again.
pub(crate) fn transport_for_serial(serial: &str) -> Result<Box<dyn Transport>> {
    let entry = entries()?
        .into_iter()
        .find(|entry| entry.serial == serial)
        .ok_or_else(|| {
            Error::new(
                ErrorCode::Disconnected,
                format!("device {serial} is not present"),
            )
        })?;
    open_path(&entry.path)
}
