# Input protocol 3.0

`keys.json` 是版本化實體按鍵表，`vectors.json` 是獨立的固定線路測試向量。
Keyboard page 0x07 與 Consumer page 0x0C 依 [USB HID Usage Tables](https://www.usb.org/hid)，
瀏覽器 code 依 [W3C UI Events code](https://www.w3.org/TR/uievents-code/)。
名稱／別名供顯示與主機 API 解析；傳輸與 ownership 使用 `KeyId`。

本 crate 包含按鍵識別、v3 編解碼、能力／狀態解析與 session；不依賴 hidapi 或 OS。
`pointer` 是主機共用的相對游標回授控制器；畫面目標換算為相對位移，沒有絕對 HID Command。
v3 opcode 33 保留編號但不再提供發送 API，韌體 3.1.0 能力 flags=23、最多 8 個 session。
`test-support` 提供有界錯誤注入的 USB 測試替身，只供主機測試使用。
生成的 C++／Rust／JavaScript 表與相關 repo 的本地 crate 均納入版本控制，各自可獨立建置。

從 FirmwareManageTool_Rust 執行：

```sh
python3 scripts/sync-input-protocol.py --peer ../EvanRemote --mscv-rust ../MSCV_RUST --mscv-cpp ../MSCV
python3 scripts/sync-input-protocol.py --check --peer ../EvanRemote --mscv-rust ../MSCV_RUST --mscv-cpp ../MSCV
cargo test --locked -p input-protocol
```

修改 keys.json 後重新生成；修改 crate 或測試向量後同步 peer。`--check` 不修改檔案，CI 檢查本地產物，
共同工作目錄另檢查 peer；勿手動修改生成表。相關 repo 同步改版時，先跑 peer drift 檢查再交付。
