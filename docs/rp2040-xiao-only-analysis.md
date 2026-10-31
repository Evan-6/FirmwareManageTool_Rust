# XIAO-only 分支：冗餘盤點與原生 Rust 遷移評估

日期：2026-10-31。分支 `rp2040-xiao-only`，起點 `8a70436`（master）。
本文件只記錄本分支實際量到的數字與可行選項；沒有實機驗證的項目一律標明。

## 1. 分支已完成的事

| 項目 | 變更 | 證據 |
| --- | --- | --- |
| AVR／Leonardo | 整個 `firmware/boards/leonardo_avr/`（83 檔，含內附 46 檔 Arduino AVR core）、`firmware/tests/avr-stubs/`、`leonardo_test.cpp`、`LeonardoUsbGolden.h` 刪除 | `git show --stat HEAD~1`，140 檔、−14,420 行 |
| 共用引擎副本 | `firmware/common/` 刪除；`boards/rp2040/src/input/` 升為手寫唯一來源（去掉「Generated from」註解） | `scripts/sync-input-protocol.py` 只再生成按鍵表與 wire vectors |
| 板型選擇 | `Board` 四變體 enum → `SKETCH`／`FQBN`／`CORE`／`BOARD_ID` 常數；`src/firmware.rs` 927 → 673 行（非測試 469） | `src/firmware.rs:15-19` |
| AVR 燒錄路徑 | `BootPort`／`parse_ports`／`select_ports`／`wait_ports`（COM 探索）全數刪除，只剩 UF2 磁碟流程 | `src/firmware.rs` `transfer_at` |
| KeyCatalog 重複 | 頂層 `boards/rp2040/KeyCatalog.h` 從未被 include；`InputState.h` 解析到 `src/input/KeyCatalog.h`。已刪重複檔並改為生成被 include 的那一份 | `firmware/boards/rp2040/src/input/InputState.h:4` |
| 腳本／CI | `scripts/test-leonardo.sh`、CI `leonardo` job、Pico 編譯步驟移除 | `.github/workflows/windows.yml` |
| 文件／授權 | README、architecture、PROTOCOL、protocol-v3、板子 README、THIRD_PARTY_NOTICES 移除 AVR／Pico 敘述；`docs/shared-firmware-refactor-spec.md` 刪除 | 歷史紀錄頁首加註 |

驗證（本機實際執行）：

- `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings` 通過。
- `cargo test --workspace --all-targets`：firmware-manage-tool 29 passed、input-protocol 17 passed。
- `python scripts/sync-input-protocol.py --check` 通過。
- `bash scripts/test-rp2040.sh`：3 組模擬測試（runtime、XIAO 巨集、純引擎）通過。
  Windows 本機的 `c++` 是原生 mingw，腳本已改用 `pwd -W` 產生磁碟機路徑（Linux 不受影響）。
- `pwsh scripts/test-v3-bootloader.ps1` 通過（Windows PowerShell 5.1 缺 `[Convert]::ToHexString`，需 PowerShell 7）。
- `arduino-cli compile --fqbn rp2040:rp2040:seeed_xiao_rp2040:usbstack=tinyusb`：成功，Flash 90,792 bytes、靜態 RAM 29,712 bytes。

> 這兩個數字與 `docs/validation.md` 記錄的 88,192／27,584 不同（+2,600／+2,128）。本分支的變更不應增加用量，
> 因此差異來源未確認（可能是當時的 ARM GCC 版本不同）。要當作基準前需重測釐清，不能直接歸因。

## 2. 剩下的冗餘

依「可回收價值 ÷ 風險」排序。

### R1 同一份 wire 規格有兩套實作

- 裝置端 C++：`src/input/` 的 engine 共 768 行 `.cpp` + 224 行 `.h`
  （InputEngine 345、OutputScheduler 215、WireCodec 76、InputState 72、SessionTable 43、BootloaderGate 17）。
- 主機端 Rust：`shared/input-protocol/src/v3.rs` 407 行、`client.rs`、`pointer.rs` 226 行。
- 兩邊都用手寫的 64-byte framing／session 序號／opcode，靠 `shared/input-protocol/vectors.json` 與
  `vectors_generated.h` 的 golden bytes 對齊。協議一改就要同步改 C++ 與 Rust，測試才能抓到漂移。
- 這是專案最大的一處重複，也是「改成原生 Rust」唯一能結構性消除的部分（見 3.A）。

### R2 為了在 PC 上跑 MCU 邏輯而存在的測試替身

- `firmware/tests/stubs/`（6 檔：Arduino.h、Adafruit_TinyUSB.h、Adafruit_NeoPixel.h、device/dcd.h、
  pico/unique_id.h、pico/util/queue.h）是手寫的 TinyUSB／Pico SDK 替身。
- `firmware/tests/rp2040_test.cpp` 752 行 + `rp2040_engine_test.cpp` 245 行，兩者覆蓋重疊
  （engine test 原本還兼測 AVR 的 2／1／8 小容量，AVR 移除後只剩「獨立 engine 實例」的價值）。
- 替身與真實 TinyUSB 的 completion 語意若有差異，測試會通過但實機行為不同；這是無法用測試覆蓋的風險。

### R3 只為了編譯而存在的 arduino-cli 佈建

`src/firmware.rs` 非測試區 469 行中，約 260 行是 arduino-cli 流程：
`ensure_cli`（含 winget 自動安裝）、`write_cli_config`（產生 YAML）、`copy_tree`（複製 sketch 到工作目錄）、
`compile`、`find_uf2`、`copy_uf2`。若韌體改為 `cargo build`，這些幾乎全部消失，只剩「找 BOOTSEL 磁碟 + 複製 UF2」。

### R4 PowerShell 版 v3 client 與 Rust client 重複

- `firmware/tools/vendor_hid_bootloader.ps1` 313 行、`firmware/boards/rp2040/upload.ps1` 211 行。
- GUI 已直接呼叫 arduino-cli 與 Rust `hid`／`v3` 模組，這兩支腳本只剩手動燒錄與 `scripts/test-v3-bootloader.ps1` 在用。
- 它們各自實作 OPEN／BOOTLOADER 封包與回覆檢查，是第 3 套（PowerShell）wire 實作。

### R5 板型身分常數散在四處

| 位置 | 內容 |
| --- | --- |
| `firmware/boards/rp2040/src/platform/BoardConfig.h` | VID `0x03F0`、PID `0x0024`、`BoardId = 2`、USB 字串 |
| `src/firmware.rs` | `FQBN`、`BOARD_ID = 2` |
| `src/protocol.rs:3` | `IDENTITIES = [(0x03f0,0x0024),(0x6666,0x6666)]` |
| `shared/input-protocol` | 協議常數、Feature 佈局 |

`0x6666/0x6666` 是舊裝置身分，目前仍留在列舉白名單中；XIAO-only 之後是否還需要，取決於是否要偵測舊板並顯示「不支援」。

### R6 同一份按鍵表有四種表示

`keys.json` → `keys_generated.rs`（host）、`keys_generated.js`（瀏覽器／MSCV）、`keys_generated.h`、
`src/input/KeyCatalog.h`（韌體）。四份都必須提交並通過 `--check`，其中只有 Rust 與韌體那兩份進入可執行檔。

### R7 文件與現況脫節

`docs/rp2040-refactor-spec.md`（303 行）仍以 `firmware/common/`、Leonardo 過渡與雙板同步為前提；
僅在頁首加註「屬歷史規格」。若要當作長期設計文件，應壓成決策紀錄（ADR），而不是保留已不存在的結構描述。

### R8 hidapi 與 windows-sys 並存

`Cargo.toml:22` 用 `hidapi 2.6.7`（C 函式庫 + build script），`Cargo.toml:23` 又已直接依賴 `windows-sys 0.61`。
HID 存取其實只需 `HidD_GetFeature`／`ReadFile`／`WriteFile`，同一份 windows-sys 就能做（見 3.D）。

## 3. 可以用原生 Rust 撰寫的部分

### A. 韌體引擎移植成 `no_std` crate（最高價值、最低風險）— 移植已完成，C++ 尚未移除

現況：engine 已經**不依賴** Arduino／TinyUSB／Pico SDK，`scripts/test-rp2040.sh` 的 `engine-test`
就是把 `src/input/*.cpp` 直接連結到主機執行檔。這證明它的介面本來就是純邏輯。

做法：
1. 建 `firmware/engine`（`#![no_std]`，無 heap，固定陣列），把 InputState／WireCodec／SessionTable／
   OutputScheduler／InputEngine／BootloaderGate 逐檔翻成 Rust，公開同樣的 `handle/advance/serviceControl/...` 介面。
2. 主機 `cargo test` 跑既有 245 行 engine test 的案例；`thumbv6m-none-eabi` 只做 `cargo build` 確認可編譯。
3. 用 `shared/input-protocol` 的 `v3::Packet` 當作 Rust 端唯一的 framing 實作，engine 直接吃同一個型別，
   C++ `WireCodec` 與 `vectors_generated.h` 的雙實作即可退役（golden vectors 仍保留在主機測試）。
4. 過渡期讓 C++ runtime 呼叫 Rust engine（`staticlib` + `extern "C"` 薄層）或反過來，用同一組 vectors A/B 比對，
   確認位元組與時序一致再切換。

可刪除：C++ engine 992 行、`firmware/common` 遺留概念、`vectors_generated.h`、PowerShell v3 client 的封包實作。
風險：`uint32` 時間、u16 計數 wrap、固定陣列索引的邊界行為必須一一對齊（測試已涵蓋大部分）。

#### A 實作結果（2026-10-31）

`firmware/engine/` 已建立並與 C++ 並存，`firmware/boards/rp2040` 仍照原樣燒錄。

| 項目 | 結果 |
| --- | --- |
| 模組 | `state`／`wire`／`session`／`scheduler`／`engine`／`boot_gate`／`led`／`config`，對應 C++ 的 992 行 |
| 記憶體模型 | `#![no_std]` + `#![forbid(unsafe_code)]`、無 heap、固定陣列；`InputState` 有 31-byte 靜態斷言 |
| 時間 | 所有 `now_ms` 運算改用 `wrapping_sub/add`（C++ 靠 unsigned 溢位，Rust debug 會 panic） |
| 生成物 | `keys_generated.rs`（按鍵 bitmap／Consumer 表）與 `tests/data/vectors_generated.rs` 由同步腳本產生 |
| `no_std` 目標 | `cargo build -p firmware-engine --release --target thumbv6m-none-eabi` 通過（RP2040 = Cortex-M0+） |
| 行為測試 | 11 個案例逐條對應 `firmware/tests/rp2040_engine_test.cpp` |
| 跨語言 wire | `tests/golden.rs` 用 `vectors.json` 的 64-byte 主機封包驅動引擎，檢查 NKRO／Consumer bits 與 OPEN／BARRIER 回覆 |
| 逐位元組 A/B | `firmware/tests/engine_trace.cpp` 與 `tests/trace.rs` 同一腳本輸出 37 行 trace（含 50／51 ms 邊界），內容完全相同 |
| 突變驗證 | 故意把鍵盤 report 長度 29→28、`STALE_MS` 50→49，A/B 分別在第 1／23 行失敗 |

移植時確認的 C++ 細節（容易誤解的兩點，已在 Rust 保留）：

1. `open()` 之後測試 harness 已 tick 12 ms，初始 state job 早已排空，所以 motion 的合併從 `count == 0` 起算；
   九次 1024 位移剛好落在 ±8192 上限，第 10 次才故障。
2. modifer usage `0xE0..=0xE7` 必須先走 `modifiers` 分支，否則 `keys[usage / 8]` 會越界（usage 224–231）。

尚未移植：TinyUSB 傳輸層、core 1 的 NeoPixel 渲染、`FirmwareRuntime` 主迴圈與 `#![no_mangle]` 橋接。
要把 Rust 引擎接上實機，下一步是這些平台層與 runtime（即 3.B 的前半段），而非再改引擎本身。

### B. 整個韌體改 Rust（`embassy-rp` 或 `rp2040-hal` + `usb-device`）

要重寫的四層：

| 層 | 現況 | Rust 對應 |
| --- | --- | --- |
| USB 裝置 | Adafruit TinyUSB（`UsbTransport.cpp` 146 行、`UsbDescriptors.cpp` 185 行、`UsbCallbacks.cpp` 35 行） | `usb-device`（`HidWriter`／`HidReader` + `ControlHandler`），或 `embassy-usb` 的 `Handler` trait；需要兩個 HID 介面（NKRO+Consumer、Vendor） |
| 排程／輸入 | core 0 主迴圈（`FirmwareRuntime.cpp` 71 行） | 同一個 engine crate + `embassy` executor 或 RTIC |
| LED | core 1 + Adafruit NeoPixel（`StatusLedRenderer.cpp` 138 行、`StatusIndicator.cpp` 45 行） | `embassy-rp` 的 PIO + `ws2812-pio`，或 `rp2040-hal` PIO program；`Atomic<LedState>` 語意不變 |
| 重啟 | `rp2040.rebootToBootloader()`（`Bootloader.cpp` 9 行） | `rp2040-hal` 的 watchdog／`rom_data` reset to BOOTSEL（等價行為需實機確認） |

收益：`arduino-cli`、winget、core 6.2.0 版本鎖、NeoPixel 函式庫、sketch 複製全部不需要；
建置變成 `cargo build --release --target thumbv6m-none-eabi` + `elf2uf2-rs`；
韌體與 host 共用 `input-protocol` 型別，R1／R2／R3／R4／R6 一次結清。

成本與風險：USB completion 語意（目前「同時只有一筆 in-flight input report、vendor reply 完成才算」）、
雙核 LED、BOOTSEL 重啟、NeoPixel 時序都需要在實機上重測；
descriptor 與 Feature 佈局必須逐 byte 對上 `firmware/tests/Rp2040UsbGolden.h`（比對已存在，可沿用）。

### C. 燒錄流程

- 現況已經是「複製 UF2 到 BOOTSEL 磁碟」，保留即可；改 Rust 韌體後 `find_uf2`／`copy_uf2` 仍可留用（約 30 行）。
- 另一個選項是走 PICOBOOT（RP2040 ROM USB bootloader）取代磁碟代號，但 Windows 需要 WinUSB 驅動綁定，
  對一般使用者反而更麻煩，**不建議**。
- 真正該刪的是 arduino-cli 佈建（R3）：`ensure_cli`、`write_cli_config`、`copy_tree`、`compile` 四段。

### D. HID 存取改 windows-sys 原生

`src/hid/transport.rs`（64 行）目前靠 hidapi：`HidApi::new`、`device_list`、`open_path`、`get_feature_report`、
`read_timeout`、`write`。改寫成：

- `SetupDiGetClassDevs` + `SetupDiEnumDeviceInterfaces`（HID class GUID）列舉，過濾 `VID/PID` 與
  `HidP_GetCaps` 的 usage page `0xFF60`／usage `0x61`（比 hidapi 的 `usage_page()` 更精確）；
- `CreateFile` + `HidD_GetFeature` 讀 Feature 14；
- `ReadFile`／`WriteFile` 配 `OVERLAPPED` + `WaitForSingleObject` 控制 timeout（目前 hidapi `read_timeout` 必開執行緒）。

收益：移除 C 依賴與 build script；timeout 由自己控制，行為更好預測。
風險：`hidapi` 已經在 Windows 上實際用過，換掉後列舉與 disambiguation（重複序號、多裝置）要重新驗證。

### E. 生成腳本

`scripts/sync-input-protocol.py` 目前 65 行（高度壓縮），同時管按鍵表、wire vectors 與跨 repo 同步。
若採 3.A／3.B，本地部分只剩「keys.json → Rust 常數」，可收進 `build.rs` 或 `xtask`；
跨 repo 同步（`--peer/--mscv-rust/--mscv-cpp`）仍需要一支小工具，用 Rust 寫可省掉 Python 依賴。

### F. 文件

把 `docs/rp2040-refactor-spec.md` 壓成 1 頁決策紀錄（為什麼拆多 TU、為什麼單一 runtime 實例、為什麼固定容量），
其餘歷史留在 git。`docs/validation.md` 已加註 AVR／Pico 段落屬歷史。

## 4. 建議順序

| 步驟 | 可回收 | 風險 | 驗證方式 |
| --- | --- | --- | --- |
| 1. XIAO-only 清理 | 已回收 14,420 行 | 無 | 本文件第 1 節 |
| 2. ~~engine → `no_std` Rust crate，與 C++ 並存 A/B~~ **已完成** | 尚未刪除任何 C++（等切換後才回收） | 低 | 11 行為測試 + vectors + 37 行 trace 逐位元組一致 |
| 2b. runtime／USB／LED 接上 Rust 引擎，C++ 引擎退役 **進行中（engine＋runtime＋USB 已可編譯出 UF2）** | 992 行 C++、測試替身、PowerShell 封包實作 | 中高（completion、雙核、NeoPixel、BOOTSEL） | 現有 C++ runtime 測試改寫為 Rust（5 個 adapter 測試已綠） + 實機 |
| 3. USB／LED／reboot 改 Rust，engine 接到 `usb-device` | TinyUSB 與 Arduino 相關全部、arduino-cli 佈建 | 中高（completion、雙核、NeoPixel、BOOTSEL） | golden bytes + 實機列舉／延遲量測 |
| 4. 拿掉 arduino-cli／winget／sketch 複製 | 約 260 行 + 使用者首次燒錄的網路依賴 | 低 | `cargo build` + UF2 複製流程 |
| 5. hidapi → windows-sys | C 依賴 | 中（列舉、timeout） | 實機列舉／延遲測試對照 |
| 6. 腳本／文件收尾 | PowerShell 524 行、Python 65 行 | 低 | `--check`、bootloader 腳本測試 |

## 5. 尚未驗證（不要當成已完成）

- 沒有任何實機 XIAO 驗證：USB 列舉、Bootloader 重啟、NeoPixel 顏色／時序、8 個 session 的實際負載、
  Windows 端延遲量測，全部沒做。
- 本機 `arduino-cli` 的 rp2040 core 是 6.2.0、NeoPixel 1.15.5、ARM GCC 由 core 提供；版本組合與
  `docs/validation.md` 歷史紀錄相同，但 Flash／RAM 數字有差異（見第 1 節註記），尚未釐清。
- GUI 燒錄流程只用假 `ProcessRunner` 測過（相依／編譯失敗會在上傳前停止），沒有真的走完「編譯→BOOTSEL→複製 UF2→hello 驗證」。

## 6. 3.B 進度：純 Rust firmware 已可編譯、尚未上機

`firmware/app/`（crate `xiao-firmware`）把 engine、runtime、USB device、boot2 與 UF2 打包全部換成 Rust，
產出物不含 Arduino core、TinyUSB 或任何 C++。

```
USB host
  ├─ interface 0: input HID  (NKRO id 2 / consumer 5 / mouse 3)
  └─ interface 1: vendor HID (OUT 12 / IN 13 / Feature 14)
       ↓ embassy-usb（HID class，64-byte，1 ms poll）
  Runtime::step（六階段，與 C++ `FirmwareRuntime::loop` 同序）
       ↓
  firmware-engine（no_std、無 heap、固定容量）
```

### 6.1 量測（release，無 LED、單核）

| 映像 | Flash | RAM |
| --- | --- | --- |
| C++ sketch（arduino-pico 6.2.0 + TinyUSB） | 90,792 B | 29,712 B |
| Rust `xiao-firmware` | 28,008 B | 17,804 B |

RAM 幾乎都是 engine（`InputEngine` 12,564 B）加 32 槽 RX 佇列（2,016 B）。
Flash 差距主要是 Arduino core 與 TinyUSB 不再被連進來。**還沒有 NeoPixel 與 core 1**，這兩項會各吃掉一些。

### 6.2 已驗證（可重跑）

- `cargo test`：5 個 adapter 測試（RX 每輪 4 包上限、input report 被拒 → HID fault、
  bus reset → failsafe + 丟棄殘留 RX、framing fault 帶錯誤數、BOOTLOADER → 120 ms gate → reboot）。
- `cargo clippy --all-targets -D warnings`（host 與 `thumbv6m-none-eabi` 各一次）、`cargo fmt --check`。
- `bash scripts/build-rust-firmware.sh`：descriptor `--check` → build → UF2（111 塊，首塊 `0x10000000`）→ footprint。
- `scripts/elf2uf2.py`：picotool 因 Rust ELF 缺 `.bi_entries` ABI 而拒絕轉檔，故自己讀 program header 產 UF2；
  `.boot2`（embassy-rp 的 W25Q080 版本）確實落在 `0x10000000`。

### 6.3 實機結果（2026-10-31，XIAO RP2040，序號 4250304B38353817）

`examples/probe.rs` 走的是 GUI 相同的主機路徑，四種模式全過：

| 模式 | 結果 |
| --- | --- |
| 預設 | Feature 14（board=2、fw=3.1.0、lease=2000 ms、poll=1 ms）→ OPEN → STATUS → 3 s HEARTBEAT → BARRIER |
| `--keys` | 注入 `a`：`keys="a"`、`completed` 2→4、`p=1`；放開後 `keys=""`、`p=0` |
| `--lease` | 閒置 2000 ms 後裝置回 EVENT `[5]`，session 失效 |
| `--bootloader` | 1 秒內進入 BOOTSEL（`RPI-RP2`），重新燒錄後再列舉為 `fs=0` |

對照：C++ firmware 燒錄前 `fs` 已累積到 31（其他工具留下的過期 session），Rust 版本重開機後
`fs=0`、`hid=0`、`rx=0`、`tx=0`。

實機抓到並修掉的兩個缺陷：

1. **Feature 14 回 63 bytes，Windows 判為短報告**。embassy 的 `get_report` 回多少就送多少，
   但 Windows 期待「report id + 63 bytes = 64」，因此在 `buf[0]` 寫入 id 並回傳 64；
   TinyUSB 由堆疊補上，所以 C++ 版本沒踩到。
2. **UF2 區塊位址未對齊 256 bytes**，RP2040 BOOTSEL 直接忽略整個檔案（檔案留在磁碟上不動）。
   `scripts/elf2uf2.py` 改成先攤平成一張 256-byte 對齊影像再切塊。

### 6.3a 尚未驗證（不要當成完成）

- HID LED `set_report`（lock key）目前接受後丟棄，沒有觀察 Windows 是否真的送。
- Windows 端延遲未與 C++ 版本對照量測。
- NeoPixel、core 1 尚未接；目前全在 core 0，`Ticker::every(1 ms)` 驅動。
- 序號改由 `Flash::blocking_unique_id` 取得，這塊板上與 C++ 相同（4250304B38353817），但只有一個樣本。
- Windows 拒絕對 keyboard collection 做 `ReadFile`（`ERROR_ACCESS_DENIED`），
  輸入路徑是用裝置端計數（`keys`／`completed`）證明，沒有自動驗證「文字真的打進視窗」。

### 6.4 這一輪順手清掉的冗餘

| 項目 | 處理 |
| --- | --- |
| `grep` 才能確認的 descriptor 漂移 | `scripts/gen-usb-descriptors.py --check` 進 build 前置 |
| picotool 對 Rust ELF 的 ABI 檢查 | `scripts/elf2uf2.py` 直接讀 ELF program header |
| `arm-none-eabi-size` 依賴 | `scripts/elf-size.py`（純 Python） |
| RP2040 沒有 atomic RMW | `AtomicU16::fetch_add` 在此 target 不存在，改 critical-section `Cell<Flags>` |
| `Runtime::feature` 掛在泛型 struct 上 | 改呼叫 `wire::feature`，避免型別推導失敗 |

## 7. C++ 全部移除（2026-10-31）

實機驗證通過後，這個分支不再保留 C++：

| 刪除 | 內容 |
| --- | --- |
| `firmware/boards/rp2040/` | 整個 sketch：`rp2040.ino`、`Firmware.cpp`、`src/input/`（引擎）、`src/platform/`（USB／LED／bootloader）、`src/runtime/` |
| `firmware/tests/` | `rp2040_test.cpp`、`rp2040_engine_test.cpp`、`engine_trace.cpp`、`Rp2040UsbGolden.h`、`ProtocolVectors.h`、`stubs/`（A/B 用的 golden trace 移到 `firmware/engine/tests/data/`） |
| `scripts/test-rp2040.sh` | 只跑 C++ 測試與 A/B diff |
| `scripts/gen-usb-descriptors.py` | 唯一的用途是別和 C++ descriptor 漂移；descriptor 改由 `firmware/app/tests/descriptors.rs` 以雜湊釘住 |
| `src/firmware.rs` 的 arduino-cli 路徑 | `ensure_cli`／`write_cli_config`／`copy_tree`／`find_uf2`／FQBN／CORE 全部移除，改為 `cargo build` + `uf2pack` |
| CI `rp2040` job | arduino-cli core 安裝與 sketch 編譯 |

連帶影響：

- GUI 燒錄只要求 Rust 工具鏈（`cargo` + `thumbv6m-none-eabi`），不再需要 winget／arduino-cli／網路下載 core。
- `UploadConfig.cli_path` 改名 `cargo_path`（保留 `cli_path` serde alias，舊設定仍可讀）。
- UF2 打包只有一份實作：`src/uf2.rs`（GUI 與 `scripts/build-rust-firmware.sh` 共用），`scripts/elf2uf2.py` 刪除。

## 8. 加上 CLOSE（2026-10-31）

`fs`（failsafe）一直被誤讀成故障次數，實際上是 session 回收次數：v3 原本沒有關閉動詞，
正常結束只送 BARRIER／RELEASE_ALL，session 留到 2000 ms 租約到期才被回收，所以每跑一次
probe 或 GUI 斷線就 +1（實測 31→32→33）。

一度改成單一 owner（`SESSIONS=1`），但 GUI、MSCV 與 EvanRemote 需要同時連線，因此改回八個
session，只新增 `CLOSE`：

| 變更 | 位置 |
| --- | --- |
| `CLOSE`（opcode 7）：歸還自己的名額、取消待送工作、釋放自己按住的鍵，不發 EVENT、**不計 failsafe** | `firmware/engine/src/wire.rs`、`engine.rs::close` |
| 主機 `Command::Close` + `Session::close()`；GUI 斷線／燒錄／關程式與 `examples/probe.rs` 收尾都送它 | `shared/input-protocol/src/v3.rs`、`src/hid.rs`、`src/backend.rs` |
| 錯誤碼改成可讀訊息（busy 會說「裝置已被其他程式佔用」） | `v3::check_response` |
| GUI 措辭：`fs` 增加＝有程式沒送 CLOSE | `src/app.rs` |

驗證：

- 連跑三次 probe（間隔 3 s）都是 `fs=0`；改動前同樣操作是 31→32→33。
- `CLOSE` 後名額立刻可用，不必等 2 秒；同一時間另一支程式仍可正常連線。
- 行為測試 `close_frees_one_owner_without_a_fault_and_keeps_the_peer`：關掉一個 session
  只釋放它按的鍵，另一個 session 的鍵還在，`fs` 不變。
- `engine_trace.golden` 原封不動通過（CLOSE 不影響既有排程），映像 28,292 B flash / 17,804 B RAM。

## 9. 持久設定（2026-11-01）

LED 亮度原本寫死在程式裡。現在存在最後一個 flash sector（`0x101FF000`），用 `SETTINGS_GET`（0A）／
`SETTINGS_SET`（0B）讀寫，格式是 `FSET` magic + 版本 + brightness + flags + FNV-1a 檢查碼。

- 只在使用者改設定時才寫，不做每次開機寫入（避免磨損）。
- 讀到壞資料或空白 sector 就回預設值，不會卡住開機。
- 這兩個命令由平台層（`UsbTransport::answer_platform`）回答；引擎只驗證 session／sequence 並續租，
  `Runtime` 只有在引擎真的接受了該序號時才把平台的回覆排進 TX，避免重播封包寫 flash。
- 實測：`--brightness 8` → 讀回 8 → 整份重燒 → 讀回仍是 8 → 還原 64。映像 33,488 B flash / 30,796 B RAM。

**沒有做 OTA（在應用內更新）**：RP2040 就地覆寫正在執行的 flash，需要整段 erase／program 迴圈都住在 RAM，
且中斷全關；寫錯就無法用軟體救回，只能按 BOOTSEL。現行 Build → BOOTSEL → 複製 UF2 已經全自動，
`12..=14` 這個 opcode 區段先保留不用。
