use super::*;

/// Turns an SDK error into a message a user can act on: the code carries the
/// meaning, the English text keeps the technical detail for bug reports.
pub(crate) fn sdk_error(error: gdb::Error) -> anyhow::Error {
    use gdb::ErrorCode;
    let hint = match error.code {
        ErrorCode::Disconnected => "裝置已斷線或無法開啟",
        ErrorCode::Timeout => "裝置沒有回應",
        ErrorCode::Framing => "協議回應格式錯誤",
        ErrorCode::Lease => "session 租約失效，請重新連線",
        ErrorCode::Busy => "裝置忙碌（同名 session 或名額已滿）",
        ErrorCode::Unsupported => "此平台或韌體不支援這個操作",
        ErrorCode::InvalidScreen => "移動目標的螢幕已不存在，請確認螢幕設定",
        ErrorCode::StaleWindow => "視窗已失效（可能已關閉或最小化）",
        ErrorCode::Overflow => "輸出溢位",
        ErrorCode::Cancelled => "操作已取消",
        ErrorCode::Invalid => "參數無效",
        _ => "HID 傳輸錯誤",
    };
    anyhow::anyhow!("{hint}：{}", error.message)
}

/// Enumeration and the physical report channel come from the GDBOX SDK, so the
/// hidapi handle, the Feature 14 length check and the short-read/write rules have
/// exactly one owner.
pub fn enumerate() -> Result<Vec<Device>> {
    let entries = gdb::Manager::list().map_err(sdk_error)?;
    Ok(entries
        .into_iter()
        .map(|entry| Device {
            path: entry.path,
            serial: entry.serial,
            product: entry.product,
            vid: entry.vid,
            pid: entry.pid,
        })
        .collect())
}

/// Opens one device. The SDK performs the Feature 14 handshake and OPENs the
/// session inside `Manager::open`, and keeps the lease alive on its own thread.
pub fn open(device: &Device) -> Result<Session> {
    Ok(Session::new(open_sdk(device)?))
}

/// The bare SDK handle, for the one caller that must close it itself: the flash
/// sequence drops the session before it waits for the BOOTSEL drive.
pub fn open_sdk(device: &Device) -> Result<gdb::Device> {
    let options = gdb::DeviceOptions {
        auto_reconnect: true,
        timeout_ms: 1000,
        heartbeat: true,
    };
    gdb::Manager::open(Some(&device.path), options).map_err(sdk_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_errors_lead_with_an_actionable_hint() {
        let text = sdk_error(gdb::Error::new(
            gdb::ErrorCode::Disconnected,
            "hidapi: no such device",
        ))
        .to_string();
        assert!(text.starts_with("裝置已斷線"), "{text}");
        assert!(text.contains("no such device"), "{text}");
    }
}
