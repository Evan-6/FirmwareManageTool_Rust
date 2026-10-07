# RP2040 韌體 3.0.3

目標為 Raspberry Pi Pico、Seeed XIAO RP2040 與相容板。建置固定使用 arduino-pico **6.2.0**、內附 Adafruit TinyUSB；XIAO 另需 Adafruit NeoPixel。對外提供共用鍵盤／Consumer／單一相對滑鼠 HID，以及 Vendor HID，合計兩個介面，沒有 CDC／COM。

## 協定與按鍵

新版工具優先選擇 [v3 二進位協定](../../protocol-v3.md)。按鍵使用 `{usage_page, usage}`，區分主鍵區、數字鍵盤、左右修飾鍵與 ISO／JIS；鍵盤輸出為 NKRO。Consumer 支援音量增減、靜音、播放／暫停、上一首、下一首、停止。字元輸入與目標鍵盤配置由作業系統處理。

[v2 文字協定](../../protocol-v2.md) 保留原本指令、別名與 ACK，最多六個一般鍵加八個修飾鍵，租約 30 秒。v3 租約 2 秒、主機每 500ms 續租，一般輸入沒有 ACK；釋放與屏障會等待 USB 傳輸完成。v3 最多八個程式同時控制；v2/v3 不得同時修改輸入狀態，競爭時回 busy。

3.0.2 移除絕對滑鼠 descriptor、座標狀態與輸出，硬體只提供相對位移、五鍵與雙軸滾輪。
畫面上的「絕對定位」與擬人化曲線都由主機程式讀取實體游標、換算相對位移並回授修正。
Feature 14 回報韌體版本 3.0.3、flags=23（absolute bit=0、multi-session bit=1），USB bcdDevice 為 0x0303。
舊 v3 opcode 33 會拒絕並釋放；v2 `mouse_abs` 回 `err:unsupported_mouse_abs`，不能誤當相對位移執行。
需要重新燒錄 RP2040；主機軟體更新不會改變舊韌體的 USB descriptor。新版管理工具不再送出絕對命令。

## 多程式同時控制

3.0.3 支援最多八個 v3 session，現有 v3 管理工具與 EvanRemote 可使用原本的 OPEN／輸入命令。
各 session 有自己的序號、按鍵／修飾鍵／Consumer／五鍵狀態、控制回覆與 2 秒租約。
USB 按住狀態取各 session 的聯集：兩個程式都按住 A 時，必須都放開 A 才會輸出放開。
相對移動與滾輪依收到的順序送出；不同程式的位移都會作用在同一支游標。

OPEN 不清除其他程式的狀態。RELEASE_ALL、非法輸入／序號故障、租約到期只清理所屬 session；
其餘程式的待送短按與屏障保留。STATUS 回報查詢者自己的輸入狀態、序號及租約，佇列與錯誤計數仍為裝置共用。
USB 斷線／傳輸故障、共用佇列溢位／過期使全部 session 失效並釋放。
全部放開後 session 仍可續租／再輸入；程式關閉後最多 2 秒回收名額。
第九個 OPEN 或重複 session ID 回 busy，不能覆蓋現有連線。

v2 文字協定沒有 session ID，不阻擋多個 v2 程式送指令，但全部共用同一份輸入狀態、租約與 ACK。
任何程式的 up／sync／reset 都可能改變其他程式的持有狀態，ACK 也無法識別來源；v2 與 v3 寫入仍互斥。
所有 v3 session 都完成 RELEASE_ALL 且輸出閒置後，v2 可接手，原有 v3 session 失效。
BOOTLOADER 在其他 v3 session 存在時回 busy；燒錄前關閉其他控制程式並等 2 秒。
多程式同時做游標回授定位時會互相影響目標位置，韌體只負責合併／排程相對輸入。

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

2026-10-07 已在 Linux 編譯 Pico 與 XIAO，並用模擬 USB 執行實際韌體邏輯測試（包含八個 session 與隔離釋放）。**新版尚未完成實體 RP2040／Windows USB 驗收。** 詳見 [驗證紀錄](../../../docs/validation.md)。需要確認列舉、NKRO／Consumer 支援、短按、鎖定鍵、兩版相容、拔線與 bootloader；編譯成功不等同實機運作。
