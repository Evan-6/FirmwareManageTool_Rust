# RP2040 韌體 3.1.0

目標為 Raspberry Pi Pico、Seeed XIAO RP2040 與相容板。建置固定使用 arduino-pico **6.2.0**、內附 Adafruit TinyUSB；XIAO 另需 Adafruit NeoPixel。對外提供共用鍵盤／Consumer／單一相對滑鼠 HID，以及 Vendor HID，合計兩個介面，沒有 CDC／COM。

## 協定與按鍵

僅接受 [v3 二進位協定](../../protocol-v3.md)。按鍵使用 `{usage_page, usage}`，區分主鍵區、數字鍵盤、左右修飾鍵與 ISO／JIS；鍵盤輸出為 NKRO。Consumer 支援音量增減、靜音、播放／暫停、上一首、下一首、停止。字元輸入與目標鍵盤配置由作業系統處理。

v3 租約 2 秒、主機每 500ms 續租，一般輸入沒有 ACK；釋放與屏障等待 USB 傳輸完成。
最多 8 個程式同時控制，不保留 v2／serial 回退。

韌體沒有絕對滑鼠 descriptor、座標狀態與輸出，硬體只提供相對位移、五鍵與雙軸滾輪。
畫面上的「絕對定位」與擬人化曲線都由主機程式讀取實體游標、換算相對位移並回授修正。
Feature 14 回報韌體版本 3.1.0、flags=23（absolute bit=0、multi-session bit=1），USB bcdDevice 為 0x0310。
舊 v3 opcode 33 會拒絕並釋放，不能誤當相對位移執行。
需要重新燒錄 RP2040；主機軟體更新不會改變舊韌體的 USB descriptor。新版管理工具不再送出絕對命令。

## 多程式同時控制

3.1.0 支援最多八個 v3 session，現有 v3 管理工具與 EvanRemote 可使用原本的 OPEN／輸入命令。
各 session 有自己的序號、按鍵／修飾鍵／Consumer／五鍵狀態、控制回覆與 2 秒租約。
USB 按住狀態取各 session 的聯集：兩個程式都按住 A 時，必須都放開 A 才會輸出放開。
相對移動與滾輪依收到的順序送出；不同程式的位移都會作用在同一支游標。

OPEN 不清除其他程式的狀態。RELEASE_ALL、非法輸入／序號故障、租約到期只清理所屬 session；
其餘程式的待送短按與屏障保留。STATUS 回報查詢者自己的輸入狀態、序號及租約，待送數也屬於查詢者，錯誤計數為裝置共用。
USB 斷線／傳輸故障、共用佇列溢位／過期使全部 session 失效並釋放。
全部放開後 session 仍可續租／再輸入；程式關閉後最多 2 秒回收名額。
第九個 OPEN 或重複 session ID 回 busy，不能覆蓋現有連線。

BOOTLOADER 在其他 v3 session 存在時回 busy；燒錄前關閉其他控制程式並等 2 秒。
多程式同時做游標回授定位時會互相影響目標位置，韌體只負責合併／排程相對輸入。

## 模組

`Firmware.cpp` 僅組裝一個 `FirmwareRuntime` 並委派 Arduino 入口。`src/input/` 的 codec、session、scheduler 與 engine 使用正常 `.h/.cpp`，分別編譯及連結，可在不帶 Arduino／TinyUSB／Pico SDK 的主機測試。`src/platform/` 持有 USB、bootloader、LED 硬體，`src/runtime/` 依固定順序協調；標頭不建立可變全域狀態，也不依賴 include 順序。

core 0 持有全部輸入、排程與 LED 狀態決策，core 1 只讀原子發布的 LED 狀態並渲染。USB callbacks 只交付 RX、完成／失敗及 reset 通知，Feature 使用初始化後的唯讀資料。RX 保留 Pico SDK queue，TX 改由 engine 的固定陣列持有，容量仍為 32。

RP2040 不再使用生成的 `modules/V3*.h`；`firmware/common/` 暫留供 Leonardo 使用。兩板仍共用 wire 規格、按鍵表與 golden vectors。詳見 [重構規格](../../../docs/rp2040-refactor-spec.md)。

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

Windows 可使用管理工具或 `upload.ps1`，bootloader 入口也只接受 v3。
首次從舊韌體升級請按住 BOOTSEL 上電後複製 UF2。正常韌體沒有 COM 與 1200bps-touch reset。
燒錄前關閉其他控制端並等 2 秒；BOOTLOADER 有其他 session 時回 busy。

## 驗證狀態

2026-10-07 已在 Linux 編譯 Pico 與 XIAO，並用模擬 USB 執行實際韌體邏輯測試（包含八個 session 與隔離釋放）。**新版尚未完成實體 RP2040／Windows USB 驗收。** 詳見 [驗證紀錄](../../../docs/validation.md)。需要確認列舉、NKRO／Consumer 支援、短按、鎖定鍵、舊設備拒絕、拔線與 bootloader；編譯成功不等同實機運作。
