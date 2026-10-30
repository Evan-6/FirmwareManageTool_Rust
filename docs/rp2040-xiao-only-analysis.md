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

### A. 韌體引擎移植成 `no_std` crate（最高價值、最低風險）

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
| 2. engine → `no_std` Rust crate，與 C++ 並存 A/B | 992 行 C++、測試替身 | 低（現有 vectors 與 245 行 engine test） | `cargo test` + 逐位元組比對 vectors |
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
