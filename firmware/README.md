# FirmwareManageTool 鍵盤／滑鼠韌體

低延遲 USB HID 韌體：主機透過 USB 下指令，裝置以硬體鍵盤與滑鼠身分送出輸入。
指令走 **Vendor HID**（usage page `0xFF60`），**不列舉 CDC 序列埠、對外不露出 COM 埠**；
提供鍵盤、相對滑鼠、5 鍵、垂直／水平滾輪，以及 failsafe 自動放開。

本分支只支援 **Seeed XIAO RP2040**；Leonardo／AVR、Pico 與其他 arduino-pico 板型已移除。
線路協定索引見 **[PROTOCOL.md](PROTOCOL.md)**：只接受 Vendor HID v3，最多 8 個 session，沒有 v2／serial 回退。

## 資料夾架構

```
firmware/
  README.md            ← 本檔
  PROTOCOL.md          ← 線路協定索引
  protocol-v3.md       ← Vendor HID v3 規範
  tests/               ← 主機端模擬測試（Arduino／TinyUSB／Pico SDK stubs）
  tools/
    vendor_hid_bootloader.ps1  ← 透過 Vendor HID 送指令（含 enter_bootloader）
  boards/
    rp2040/            ← XIAO RP2040 專用 sketch（arduino-pico 6.2.0 + Adafruit TinyUSB）
      rp2040.ino, Firmware.cpp/.h, KeyCatalog.h
      src/input/       ← WireCodec、SessionTable、OutputScheduler、InputEngine、BootloaderGate
      src/platform/    ← USB descriptors／transport／callbacks、bootloader、LED
      src/runtime/     ← FirmwareRuntime 協調
```

`src/input/` 與 `src/platform/Usb*` 是唯一來源，沒有生成副本；
`KeyCatalog.h` 由 `shared/input-protocol/keys.json` 產生，請透過 `scripts/sync-input-protocol.py` 更新。

## 建置與燒錄

固定 FQBN 為 `rp2040:rp2040:seeed_xiao_rp2040:usbstack=tinyusb`，Core 固定 arduino-pico **6.2.0**，
另需 Adafruit NeoPixel。管理工具會把 sketch 複製到工作目錄、安裝依賴、編譯並複製 UF2；
也可在 `boards/rp2040/` 內使用 `upload.ps1`。

```sh
arduino-cli core update-index --additional-urls https://github.com/earlephilhower/arduino-pico/releases/download/global/package_rp2040_index.json
arduino-cli core install rp2040:rp2040@6.2.0 --additional-urls https://github.com/earlephilhower/arduino-pico/releases/download/global/package_rp2040_index.json
arduino-cli lib install "Adafruit NeoPixel"
arduino-cli compile --fqbn rp2040:rp2040:seeed_xiao_rp2040:usbstack=tinyusb firmware/boards/rp2040
```

第一次燒錄或無法由 HID 進入下載模式時，按住 BOOTSEL 上電後複製 UF2。
正常韌體沒有 COM，也沒有 1200bps-touch reset。

## 容量與平台

output／RX／TX 佇列深度均為 32，8 個 session；滿載或最舊待送工作超過 50ms 即回報故障並優先釋放。
USB 完成依 TinyUSB 主機 ACK 通知；個別釋放保留其他來源，USB／共用佇列故障才清理全部 session。
core 0 擁有輸入、排程與 LED 決策，core 1 只讀發布的狀態並驅動 NeoPixel。
Feature 14 的 board id 固定為 2，USB 序號取自 RP2040 unique ID。
