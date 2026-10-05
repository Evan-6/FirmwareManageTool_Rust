# rp2040 — RP2040 韌體

目標板：Raspberry Pi Pico 及相容 RP2040 板。
使用 [arduino-pico](https://github.com/earlephilhower/arduino-pico)（`rp2040:rp2040`）core，
USB 堆疊為 **Adafruit TinyUSB**。對外只列舉 **鍵盤／滑鼠共用 HID + Vendor HID** 兩個介面，
功能使用唯一的 Vendor HID 通道。Sunshine 透過既有 ASCII 行協定傳送鍵盤
命令，以及新增的滑鼠命令；不列舉 CDC/COM 埠。

線路協定（指令／回應／鍵名）與 AVR 版完全相同，見上層 [`../../PROTOCOL.md`](../../PROTOCOL.md)。

## 為什麼沒有 COM 埠

本韌體不呼叫 `Serial.begin()`，因此不會建立 USB 序列埠。指令全走 Vendor HID。

代價：沒有序列 1200bps-touch 自動重置。改由 `upload.ps1` 透過 **Vendor HID 送
`enter_bootloader`** 讓板子自己進 ROM UF2 bootloader（跟 AVR 版自動進 bootloader 的作法一致）。

## 建置與燒錄

```powershell
# 韌體已在跑：直接執行，腳本會自動送 enter_bootloader、等 RPI-RP2 磁碟出現、再燒
firmware\boards\rp2040\upload.ps1

# 全新／空白板（還沒有本韌體可下指令）：按住 BOOTSEL 上電掛載成 RPI-RP2 後執行
firmware\boards\rp2040\upload.ps1

# 也可手動指定 RPI-RP2 磁碟路徑（跳過自動 enter_bootloader）
firmware\boards\rp2040\upload.ps1 -Port E:\
```

`upload.ps1` 會：

1. 註冊 arduino-pico 的 board manager URL 並安裝 `rp2040:rp2040` core
   （該 core 已內含 Adafruit TinyUSB，不需另裝函式庫）。
2. 以 FQBN `rp2040:rp2040:rpipico:usbstack=tinyusb` 編譯（**必須**選 TinyUSB 堆疊，
   否則 `Firmware.cpp` 開頭的 `#error` 會擋下）。
3. 進 bootloader：若已偵測到 RPI-RP2 磁碟就直接用；否則透過共用的
   `../../tools/vendor_hid_bootloader.ps1` 送 `enter_bootloader`，等磁碟掛載。
4. 把編出的 `.uf2` 複製到該磁碟 → RP2040 自動燒錄並重啟。

其它旗標：`-SkipBootloader`（不送指令、假設已在 BOOTSEL）、`-SkipCoreInstall`（跳過 core 安裝）。
若用其它 RP2040 板（非 Pico），把 `upload.ps1` 的 `$Fqbn` 板名部分改成對應型號
（例如 `rp2040:rp2040:adafruitkb2040:usbstack=tinyusb`）。

## 下載模式（bootloader）

- `enter_bootloader` / `bootloader` 指令 → 呼叫 `rp2040.rebootToBootloader()`，
  重啟進入 ROM UF2 bootloader（會掛載成 RPI-RP2 磁碟）。
- 也可實體按住 BOOTSEL 上電進入。

## 平台相依重點（與 AVR 版的差異）

| 面向 | AVR (`leonardo_avr`) | RP2040（本板） |
|------|----------------------|----------------|
| 鍵盤 HID | Arduino AVR `HID` 函式庫 | `Adafruit_USBD_HID`（keyboard 描述子，report id 2） |
| Vendor HID | 自訂 `PluggableUSBModule` | `Adafruit_USBD_HID` + OUT endpoint，OUT 走 `setReportCallback` |
| CDC / COM | 不列舉 | Vendor HID only |
| Mouse HID | Arduino HID report IDs 3/4 | TinyUSB report IDs 3/4 |
| Flash 字串 | `PROGMEM`/`PSTR` | 一般字串（平坦記憶體） |
| bootloader | `avr/wdt` + MAGIC_KEY | `rp2040.rebootToBootloader()` |

## ⚠️ 待實機驗證項目

本板韌體邏輯忠實移植自 AVR 版，但 **尚未在實體 RP2040 上編譯／燒錄過**。
上機時請重點確認：

1. **編譯**：需選 Adafruit TinyUSB 堆疊；核心版本需支援 `TinyUSBDevice.setID()`、
   `setManufacturerDescriptor()`、`setProductDescriptor()`（新版 arduino-pico 才有，
   若編譯報找不到，改用該版本對應 API 或移除這幾行改用 build flag 設定 VID/PID）。
2. **不露出 COM**：列舉後裝置管理員應只看到 HID，**沒有** COM 埠。
3. **列舉**：Keyboard + Vendor HID 兩介面能否同時列舉；主機 `arduino.cpp`
   能否用 VID/PID `03F0:0024`（usage `0xFF60`）＋ `hello` 交握認到裝置。
4. **Vendor HID OUT**：`vendorSetReport()` 對 OUT-endpoint 封包的 report-id 解析
   （目前假設 report_id 參數為 0 時，id 在 buffer[0]）——不同 TinyUSB 版本行為可能不同，
   若 Vendor HID 指令無效，先確認這裡拿到的 buffer 佈局。

驗證 OK 後，把上層 `README.md` 板子清單的狀態從「待實機驗證」改成「已在實機運作」。
