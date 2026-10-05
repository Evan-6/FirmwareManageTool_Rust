# leonardo_avr — ATmega32U4 韌體

目標板：Arduino Leonardo / Micro / ATmega32U4 Pro Micro（原生 USB 的 AVR 板）。
使用 Arduino AVR core，直接註冊並送出原始 HID 鍵盤、相對滑鼠與絕對滑鼠
report（**不要** `#include <Keyboard.h>` / `<Mouse.h>`）。
指令走 **Vendor HID**；板子以 `-DCDC_DISABLED` 編譯（見 `hardware/goosedevil/avr/boards.txt`），
**不列舉 CDC、對外不露出 COM 埠**。

線路協定（指令／回應／鍵名）見上層 [`../../PROTOCOL.md`](../../PROTOCOL.md)。

## 建置與燒錄

```powershell
firmware\boards\leonardo_avr\upload.ps1            # 自動找埠
firmware\boards\leonardo_avr\upload.ps1 -Port COM4 # 指定埠
```

`upload.ps1` 會用 `arduino-cli` 以 FQBN `goosedevil:avr:keyboard` 編譯本 sketch。
board package 直接內嵌在本資料夾：`hardware/goosedevil/avr`（arduino-cli 的 user dir
就指到本資料夾，因此 `hardware/` 必須留在 sketch 內）。

## 下載模式（bootloader）

- 正常情況：`upload.ps1` 會先透過 **Vendor HID** 送 `enter_bootloader`，
  裝置進入 LUFA/Caterina bootloader 後再上傳（不依賴 CDC 的 1200bps touch）。
- 相關輔助腳本：`../../tools/vendor_hid_bootloader.ps1`（AVR 與 RP2040 共用）。
- 若 runtime 韌體壞到 Vendor HID 也無法列舉，改用實體 reset 進 bootloader 後
  以 `-Port` 指定該埠上傳。

## 平台相依重點

- USB：Arduino AVR core 的 `HID` 函式庫送鍵盤（report id 2）、相對滑鼠
  （report id 3）與絕對滑鼠（report id 4）；支援 5 鍵、垂直／水平滾輪。
  Vendor HID 通道自訂 `PluggableUSBModule`（`VendorHidTransport`）。
- Flash 字串走 `PROGMEM` / `PSTR` / `pgm_read_*`。
- bootloader 進入：`avr/wdt.h` 看門狗 + MAGIC_KEY（Caterina 相容）。

> 對照 RP2040 版本：`../rp2040/`。協定相同、USB 實作不同。
