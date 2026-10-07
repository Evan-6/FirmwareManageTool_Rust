# 驗證紀錄

日期：2026-10-07（Asia/Taipei）。開發主機為 Linux，沒有連接 Leonardo／RP2040 或 Windows USB 實機。

## 韌體 3.1.0（重構前產物）

Leonardo／Pico／XIAO 僅提供 Vendor HID v3，8 個獨立 session、NKRO／Consumer／相對滑鼠。
v2 解析器、report 10／11、serial 回退與舊 bootloader 線路已移除。
Leonardo 固定序號 HP-KB-0024、Feature board=3；RP2040 保留唯一板號。
兩種板子 Feature flags=23、bcdDevice=0x0310；Feature 原先的舊租約位置保留為零，不改動後續欄位。

| 板子 | 固定 Core | Flash bytes | 靜態 RAM bytes | 可用 RAM bytes |
| --- | --- | ---: | ---: | ---: |
| Leonardo | bundled AVR 1.8.8 | 13474 | 1811 | 749 |
| Pico | arduino-pico 6.2.0 | 85376 | 27492 | 234652 |
| XIAO | arduino-pico 6.2.0、NeoPixel 1.15.5 | 88192 | 27584 | 234560 |

產物位於 `dist/firmware-v3/`：`leonardo-3.1.0.hex`、`pico-3.1.0.uf2`、`xiao-3.1.0.uf2`。
`BUILD-3.1.0.json` 保存來源指紋、產物 SHA256、固定 Core 版本與建置用量；dist 不納入版本控制。
AVR 額外以同一編譯器、停用 LTO 的 `-fstack-usage` 檢查個別函式：最大 frame 為主迴圈 198 bytes，
releaseSession 133、sessionFault 83、USB setup 80、reply 76、sendReport 73、protocolFault 13；這不是完整堆疊或實機保證。

## 已通過的檢查

- `scripts/test-rp2040.sh`：重構後分別編譯及連結 RP2040 的正式 `.cpp`，使用假 USB completion。
  涵蓋同鍵／五鍵持有、個別釋放／租約／錯誤、歷史短按與位移保留、序號、8 個 session／第九個拒絕、
  USB／佇列故障、bootloader 完成，以及舊 report／opcode 拒絕。
- `scripts/test-leonardo.sh`：編譯實際 AVR Firmware.cpp 與內附 core 的 HidPackets.h。
  涵蓋 endpoint bank ACK、不發送尾端 ZLP、Feature／介面、快速 down/up/barrier、8 個 session、
  個別釋放／STATUS pending、滿載廣播、8 個閒置租約同時到期、USB reset／suspend 與 bootloader 回覆完成。
- 兩種韌體測試另以 ASan／UBSan 執行，`UBSAN_OPTIONS=halt_on_error=1` 通過。
- `scripts/test-v3-bootloader.ps1`：PowerShell parser 與內嵌 C# 編譯；以假 Stream 執行實際控制函式，
  驗證 OPEN／BOOTLOADER 固定封包、其他 session、busy／fault、填充、短回覆、序號及 timeout。
  未在 Linux 呼叫 Windows HID P/Invoke 或實際重啟設備。
- 管理工具：workspace no-default-features 31＋共享協定 17 項測試、全目標嚴格 Clippy、fmt 通過。
  全 workspace／all-targets 的 Windows MSVC 原始碼檢查通過。
- MSCV_RUST：輸入 facade／假 USB 14 項測試，輸入層 Windows MSVC 嚴格 Clippy，app／bins 原始碼檢查通過。
  未格式化無關的既有 app 程式。
- MSCV：可攜式 v3 測試與 ASan／UBSan、HardwareKeyboard Windows GNU 目標物件編譯（嚴格警告）通過。
- EvanRemote：core 19、hardware-input 23、共享協定 17 項測試與 Node 18 項通過。
  hardware-input／server 的 Windows MSVC 原始碼檢查通過；原有軟體輸出測試持續通過。
  輸入相關嚴格 Clippy 在允許既有 core 五項 lint 後通過：derivable_impls、io_other_error、result_unit_err、type_complexity、too_many_arguments。
- `sync-input-protocol.py --check --peer ../EvanRemote --mscv-rust ../MSCV_RUST --mscv-cpp ../MSCV` 通過。
  共享引擎與按鍵表生成物已納入版本控制，使用端可獨立建置。

CI 已加入 Leonardo 固定 Core 建置、實際韌體測試及 PowerShell bootloader 測試；尚未在遠端執行。
libudev 開發標頭與 pkg-config 在 `/tmp/rp2040-redesign/udev` 準備，以 PKG_CONFIG_PATH／CFLAGS 驗證，未修改系統套件。
Windows 原始碼檢查使用既有 SDK／交叉工具，只有 check 或物件編譯，沒有產生 Windows EXE／ZIP。
先前使用者回報 Linux 產出的 Windows EXE 無法啟動；後續發行檔僅由 Windows 原生 MSVC 建置。

## 待 Windows／USB 實機驗收

- [ ] Leonardo／Pico／XIAO 列舉、Feature、NKRO／Consumer、鎖定鍵 LED、五鍵／雙軸滾輪及快速短按。
- [ ] 多個 Windows 程式同時開啟 Vendor HID，收到自己的回覆／EVENT，同鍵持有與個別停止／退出。
- [ ] 8 個 session、個別租約、AVR 小佇列的實際負載／故障回報、USB 擁塞／reset／拔線及重連。
- [ ] 原有游標回授與 Windows 軟體輸出、模式切換與失焦／取消清理。
- [ ] 舊設備拒絕、首次 reset／BOOTSEL 升級、v3 bootloader 回覆完成與燒錄後板型驗證。
- [ ] 重複 Leonardo 序號／多設備的燒錄歧義處理。
- [ ] Windows 原生完整 app 建置／測試／打包，GUI 與延遲量測實際操作。

編譯及模擬測試不取代 Windows driver 或 USB 實機驗證。屏障代表 USB 完成，延遲量測仍以 Windows GetAsyncKeyState 觀察到輸入為終點。

## RealTime 量測修正

使用者回報要求 50 輪卻只有約 15 輪成功。已確認極限測試切換至子程序後，
GUI 原本已釋放的 session 仍需等待 2 秒租約到期；共用 failsafe 計數因此增加，
舊量測判定會將其他 session 的到期誤認為本輪故障並停止。實際出錯輪數仍需使用者的錯誤文字確認。

- 實際共用韌體測試重現 GUI session 到期、只有 GUI 收到 EVENT、worker 仍能按下／放開／屏障／STATUS，且 failsafe 增加。
- 主機假 USB 回歸測試確認：其他 session 的 EVENT 與 failsafe 增加不會使測試失敗；本 session EVENT、TX／HID 計數增加仍失敗。
- REALTIME／TIME_CRITICAL 限於送出指令；Windows 觀察使用高精度優先權，避免在 REALTIME 忙等阻礙輸入。
- 顯示成功／已執行／要求／未執行輪數，成功率以已執行輪數計算；15 成功、1 timeout、34 未執行有回歸測試。
- Windows MSVC 全目標嚴格 Clippy 只做程式碼檢查；尚未進行 Windows／USB 實機重測，沒有產生 Windows EXE。


## RP2040 結構重構

Pico／XIAO 已依 [rp2040-refactor-spec.md](rp2040-refactor-spec.md) 遷移至正常 `.h/.cpp`；
純引擎與平台分開，FirmwareRuntime 持有正式實例，core 0 決定輸入／LED 狀態，core 1 只渲染 LED。
版本維持 protocol 3、firmware 3.1.0、bcdDevice 0x0310。Leonardo 與 `firmware/common/` 的內容未變。

### 行為與建置

- 重建 `ec34092` 的重構前 Pico／XIAO 基準，Flash／RAM 與上表一致。
- 同一批既有測試命令／時間／USB completion 下，前後 552 筆 input／vendor report 的 bytes 與順序完全一致。
  比對時每個測試案例都從零計數開始，舊 harness 的直接故障注入改由相同的 USB failed callback 觸發。
- Pico／XIAO 的 159-byte input descriptor、35-byte vendor descriptor、63-byte Feature 各自逐位元組相同。
  重構前的 bytes 固定於 `firmware/tests/Rp2040UsbGolden.h`，不由新 encoder 產生預期值。
- `scripts/test-rp2040.sh` 同時驗證 Pico／XIAO runtime、無板級 SDK 的正式引擎、各 header 獨立編譯、兩個 TU 引用並正常連結。
  新增獨立 engine、1999／2000ms 租約、50／51ms 過期、時鐘／計數 wrap、TX／motion overflow、RX 四筆預算與錯誤次數、
  send 拒絕重試／清理、錯誤 report ID completion、USB reset 舊通知、BOOT ACK／119／120ms 與 LED 核心分工。
- 完整 RP2040 測試另以 ASan／UBSan、`UBSAN_OPTIONS=halt_on_error=1` 通過。
- arduino-cli 1.5.1、arduino-pico 6.2.0、TinyUSB、NeoPixel 1.15.5 下兩板全新編譯通過，所有新 `.cpp` 都出現在編譯清單。
  擷取並編譯管理工具原有 `copy_tree` 函式，遞迴複製完整 sketch 至工作副本；來源逐檔相同，副本 Pico 建置也通過，UF2 與直接建置逐位元組相同。
- Leonardo 模擬測試與固定 bundled core 建置通過，仍為 Flash 13474／靜態 RAM 1811 bytes。
  共享生成物及四個 repo 的同步檢查通過；管理工具／共享協議 31＋17 項 Rust 測試通過。

TinyUSB 的 `tud_event_hook_cb` 在 BUS_RESET／UNPLUGGED 發布同步 reset 通知，因 BUS_RESET 不保證出現主迴圈可觀察的 mounted=false。
Transport 以 USB epoch、input report ID 與 BOOT 回覆 session／sequence 追蹤傳送；
引擎在 reset／故障使 generation 失效，避免舊 completion 推進新工作或重啟。
Vendor 回覆只在 send 接受後出列；BOOT ACK generation 在 callback 收到時保存，不被後續 STATUS 覆寫。

同步使用固定 Core 的 `pico_atomic/atomic.c`：原子操作以 atomic spinlock 加上 IRQ save／restore 實作，
不是假設 Cortex-M0+ 的原子操作皆 lock-free。callback 只執行有界複製、佇列操作與通知，
沒有等待 USB、sleep、引擎命令執行或 NeoPixel 渲染。LED 狀態以 release／acquire 發布。

### 資源差異

| 板子 | 重構前 Flash | 重構後 Flash | 重構前靜態 RAM | 重構後靜態 RAM | 重構後可用 RAM |
| --- | ---: | ---: | ---: | ---: | ---: |
| Pico | 85376 | 87920 | 27492 | 29636 | 232508 |
| XIAO | 88192 | 90792 | 27584 | 29712 | 232432 |

Flash 分別增加 2544／2600 bytes，來自分檔後明確的模組呼叫與 USB 傳送追蹤／同步。
TX 的 32×63=2016 bytes 改由 InputEngine 固定持有，原本的 Pico SDK TX queue heap 配置移除。
固定 SDK 的 queue 實際配置為 `(32+1)×63=2079` bytes；RX 仍保留一組相同配置。
因此靜態 RAM 的 +2144／+2128 bytes 並非全數增加執行期記憶體：扣除移除的 TX queue 資料配置，
靜態加佇列資料合計差異為 +65／+49 bytes，未計 allocator metadata 或其他 SDK 執行期配置。
沒有新增熱路徑 heap 配置或多組輸出／RX／TX 佇列。

使用相同 ARM 編譯器／`-Os`，移除 LTO 旗標並加 `-fstack-usage`，分別編譯正式來源：

| 函式 frame | 重構前 | 重構後 |
| --- | ---: | ---: |
| 主迴圈 | Firmware::loop 200 | FirmwareRuntime::loop 152；入口 wrapper 8 |
| sessionFault | 120 | 32 |
| releaseSession | 144 | 176 |
| reply | 96 | 104 |
| queueState／queueMotion／queueBarrier／cancelOwner | 融入原呼叫者 | 320／304／272／288 |

新 wrapper 的主要成本是局部 8×31=248-byte 來源快照，取代 scheduler 對全域 session 的讀取。
較深的應用程式路徑保守相加（未扣 tail call／未包含 SDK、libc、Arduino main 或中斷 frame）：

- 原版：loop → sessionFault → releaseSession → state → protocolFault → reply 約 640 bytes。
- 新版：入口 → runtime.loop → handle → queueState → checkScheduled → fault → event → reply → wire.reply 約 860 bytes。
- 新版個別取消：入口 → runtime.loop → handle → sessionFault → cancelOwner → releaseSession → state 約 792 bytes。

兩板 ELF linker map 的 core 0 stack 範圍為 `0x20041800..0x20042000`（2048 bytes）。
上述數字是模組 frame 與主要呼叫路徑分析，不能當成完整 peak stack 或實機保證；
USB 中斷、SDK 與 XIAO core 1 NeoPixel 的實際尖峰仍待硬體驗收。

重構產物另存於 `dist/firmware-rp2040-refactor/`：`pico-3.1.0.uf2`、`xiao-3.1.0.uf2` 與 `BUILD-3.1.0.json`。
manifest 記錄 sketch 來源指紋、產物 SHA256、固定工具／核心與用量；不覆蓋前述重構前產物。
目前未燒錄設備，亦未產生 Windows EXE／ZIP；一般／高精度／RealTime 各 50 輪與多 session 的 Windows／USB 實機驗收仍待完成。
