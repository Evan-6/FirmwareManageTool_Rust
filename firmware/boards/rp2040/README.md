# RP2040 韌體 3.0

目標為 Raspberry Pi Pico、Seeed XIAO RP2040 與相容板。建置固定使用 arduino-pico **6.2.0**、內附 Adafruit TinyUSB；XIAO 另需 Adafruit NeoPixel。對外提供共用鍵盤／Consumer／相對與絕對滑鼠 HID，以及 Vendor HID，合計兩個介面，沒有 CDC／COM。

## 協定與按鍵

新版工具優先選擇 [v3 二進位協定](../../protocol-v3.md)。按鍵使用 `{usage_page, usage}`，區分主鍵區、數字鍵盤、左右修飾鍵與 ISO／JIS；鍵盤輸出為 NKRO。Consumer 支援音量增減、靜音、播放／暫停、上一首、下一首、停止。字元輸入與目標鍵盤配置由作業系統處理。

[v2 文字協定](../../protocol-v2.md) 保留原本指令、別名與 ACK，最多六個一般鍵加八個修飾鍵，租約 30 秒。v3 租約 2 秒、主機每 500ms 續租，一般輸入沒有 ACK；釋放與屏障會等待 USB 傳輸完成。v2/v3 不得同時修改輸入狀態，競爭時回 busy。

## 模組

`Firmware.cpp` 組合各內部模組，core 0 負責 USB／解析／狀態／排程；core 1 只讀取原子 LED 狀態並渲染。`modules/` 分離設定、輸入模型、USB descriptor、輸出排程、v2 adapter／解析／RX／TX、v3、LED 與生命週期。實作 header 只在此 translation unit 組合一次，避免 TinyUSB callbacks 與狀態出現多份實例。

- USB callback 只接收封包或通知完成；不執行輸入命令，也不等待 endpoint。
- 鍵盤與按鈕轉換保持順序；32 項輸出佇列，50ms 過期／溢位停止 session 並優先釋放。
- 相鄰移動可合併，不能跨按鈕事件／屏障；斷線不重播舊事件。
- `KeyCatalog.h` 由版本化按鍵表產生，請透過同步腳本更新。

## 建置與燒錄

```sh
arduino-cli core update-index --additional-urls https://github.com/earlephilhower/arduino-pico/releases/download/global/package_rp2040_index.json
arduino-cli core install rp2040:rp2040@6.2.0 --additional-urls https://github.com/earlephilhower/arduino-pico/releases/download/global/package_rp2040_index.json
arduino-cli lib install "Adafruit NeoPixel"
arduino-cli compile --fqbn rp2040:rp2040:rpipico:usbstack=tinyusb firmware/boards/rp2040
arduino-cli compile --fqbn rp2040:rp2040:seeed_xiao_rp2040:usbstack=tinyusb firmware/boards/rp2040
```

Windows 可使用管理工具，或 `upload.ps1`。舊 PowerShell 入口透過 v2 `enter_bootloader`，仍可操作新版韌體；若 v3 正在控制裝置，先關閉控制端並等租約釋放。空白板按住 BOOTSEL 上電後複製 UF2；正常韌體沒有 COM 與 1200bps-touch reset。

## 驗證狀態

2026-10-06 已在 Linux 編譯 Pico 與 XIAO，並用模擬 USB 執行實際韌體邏輯測試。**新版尚未完成實體 RP2040／Windows USB 驗收。** 詳見 [驗證紀錄](../../../docs/validation.md)。需要確認列舉、NKRO／Consumer 支援、短按、鎖定鍵、兩版相容、拔線與 bootloader；編譯成功不等同實機運作。
