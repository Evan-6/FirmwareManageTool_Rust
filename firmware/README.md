# FirmwareManageTool 鍵盤／滑鼠韌體

低延遲 USB HID 韌體：主機透過 USB 下指令，裝置以硬體鍵盤與滑鼠身分送出輸入。
指令走 **Vendor HID**（usage page `0xFF60`），**不列舉 CDC 序列埠、對外不露出 COM 埠**；
提供鍵盤、相對／絕對滑鼠、5 鍵、垂直／水平滾輪，以及 failsafe 自動放開。

線路協定規格見 **[PROTOCOL.md](PROTOCOL.md)**（所有板子共用的單一事實來源）。

## 資料夾架構

```
firmware/
  README.md            ← 本檔（總覽 + 板子清單）
  PROTOCOL.md          ← 線路協定規格（板子共用）
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
| [`leonardo_avr`](boards/leonardo_avr/) | ATmega32U4 | Leonardo / Micro / Pro Micro | 自帶 `goosedevil:avr` board package（Arduino AVR core + PluggableUSB） | 已在實機運作 |
| [`rp2040`](boards/rp2040/) | RP2040 | Raspberry Pi Pico 及相容板 | arduino-pico（`rp2040:rp2040`）+ Adafruit TinyUSB | 已在實機運作 |

## 快速燒錄

兩塊板子都用各自資料夾內的 `upload.ps1`（內部呼叫 `arduino-cli`）：

```powershell
# ATmega32U4
firmware\boards\leonardo_avr\upload.ps1

# RP2040
firmware\boards\rp2040\upload.ps1
```

各板的 core 安裝、bootloader 進入方式、注意事項見該資料夾 `README.md`。

## 為什麼兩塊板子的協定邏輯是各自一份

`leonardo_avr` 與 `rp2040` 的 USB 堆疊完全不同（AVR PluggableUSB vs. TinyUSB），
指令解析／鍵盤狀態機雖然邏輯相同，但為了不讓可運作的 AVR 韌體被跨平台抽象拖累，
兩邊各自保留一份實作。**保持兩者一致的依據是 [PROTOCOL.md](PROTOCOL.md)**——
改協定時兩份都要同步，並更新該規格。
