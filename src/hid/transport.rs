use super::*;
#[cfg(windows)]
struct NativeTransport(hidapi::HidDevice);
#[cfg(windows)]
impl HidTransport for NativeTransport {
    fn feature(&mut self) -> Result<Option<[u8; 64]>> {
        let mut r = [0; 64];
        r[0] = 14;
        match self.0.get_feature_report(&mut r) {
            Ok(n) => {
                ensure!(n == 64, "能力 report 長度錯誤");
                Ok(Some(r))
            }
            Err(_) => Ok(None),
        }
    }
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
