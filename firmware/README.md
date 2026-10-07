# FirmwareManageTool 鍵盤／滑鼠韌體

低延遲 USB HID 韌體：主機透過 USB 下指令，裝置以硬體鍵盤與滑鼠身分送出輸入。
指令走 **Vendor HID**（usage page `0xFF60`），**不列舉 CDC 序列埠、對外不露出 COM 埠**；
提供鍵盤、相對滑鼠、5 鍵、垂直／水平滾輪，以及 failsafe 自動放開。

線路協定索引見 **[PROTOCOL.md](PROTOCOL.md)**。Leonardo 與 RP2040 都只接受 Vendor HID v3，最多 8 個 session，沒有舊版回退。

## 資料夾架構

```
firmware/
  README.md            ← 本檔（總覽 + 板子清單）
  PROTOCOL.md          ← 線路協定規格（板子共用）
  common/input/        ← 共用協議、session、排程、TX、boot gate
  common/usb/          ← 共用 report descriptors
  tools/
    vendor_hid_bootloader.ps1  ← 共用：透過 Vendor HID 送指令（含 enter_bootloader）
  boards/
    leonardo_avr/      ← ATmega32U4（Leonardo / Micro / Pro Micro），Arduino AVR core
    rp2040/            ← RP2040（Pico 等），arduino-pico core + Adafruit TinyUSB
```

每個板子資料夾自成一個 Arduino sketch（`<board>.ino` + `Firmware.cpp/.h`），
並附各自的 `upload.ps1` 與 `README.md`。

## 支援板子

| 板子 | MCU | 目標 | Core / USB 堆疊 | 狀態 |
|------|-----|------|-----------------|------|
| [`leonardo_avr`](boards/leonardo_avr/) | ATmega32U4 | Leonardo / Micro / Pro Micro | 自帶 `goosedevil:avr` board package（Arduino AVR core + PluggableUSB） | 新版已編譯／模擬測試，待實機驗證 |
| [`rp2040`](boards/rp2040/) | RP2040 | Raspberry Pi Pico 及相容板 | arduino-pico（`rp2040:rp2040`）+ Adafruit TinyUSB | 新版已編譯／模擬測試，待實機驗證 |

## 快速燒錄

兩塊板子都用各自資料夾內的 `upload.ps1`（內部呼叫 `arduino-cli`）：

```powershell
# ATmega32U4
firmware\boards\leonardo_avr\upload.ps1

# RP2040
firmware\boards\rp2040\upload.ps1
```

各板的 core 安裝、bootloader 進入方式、注意事項見該資料夾 `README.md`。

## 共用引擎與平台差異

`common/input/` 是協議、session、輸出排程、TX 與 bootloader 等待條件的唯一實作來源；
`common/usb/` 是 USB report descriptor 的唯一來源。
`scripts/sync-input-protocol.py` 將相同的正常 `.h/.cpp` 同步到兩板的 `src/`，供 Arduino 獨立編譯；
按鍵表仍由 `shared/input-protocol/keys.json` 產生。生成物需提交，`--check` 會檢查兩板漂移與多餘的引擎來源。
板型差異限於 `src/platform/`、`src/runtime/` 及容量設定；不再有 namespace 內 include 的實作片段。
詳見 [共用韌體重構規格](../docs/shared-firmware-refactor-spec.md)。
兩種板子都有 NKRO、七種 Consumer、五鍵相對滑鼠與雙軸滾輪。
RP2040 output／RX／TX 深度為 32／32／32，AVR 為 2／1／8；滿載回報故障。
USB 完成依平台的主機 ACK 通知；個別釋放保留其他來源，USB／共用佇列故障清理全部 session。
第一次從舊韌體升級使用 reset（AVR）或 BOOTSEL（RP2040）。新版 PowerShell bootloader 入口也只用 v3。
