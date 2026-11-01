# FirmwareManageTool — XIAO RP2040 鍵盤／滑鼠管理工具

Windows 桌面程式＋裝置韌體。程式透過 USB 對 Seeed XIAO RP2040 下指令，裝置以真正的
HID 鍵盤／滑鼠身分送出輸入（1 ms poll）。沒有 COM 埠、沒有序列埠回退、沒有背景服務。

- **圖形介面**：Tauri 2 + React 19 + TypeScript，單一 exe，不連網路。
- **韌體**：純 Rust（embassy-rp + embassy-usb），`no_std`、無 heap，32.0 KiB flash／30.0 KiB RAM。
- **同時多程式**：最多八個控制程式（本工具、MSCV、EvanRemote 等）共用同一支裝置，各自獨立 session。
- **輸入**：NKRO 鍵盤、Consumer 多媒體鍵、五鍵相對滑鼠（含雙軸滾輪）、擬人化移動、文字輸入、組合鍵、巨集。

## 需求

- Windows 10／11 x64（GUI、燒錄、打包都只在 Windows 驗證）。
- Rust stable（以 1.98.1 驗證）＋ Visual Studio Build Tools 的「Desktop development with C++」與 Windows SDK。
- Node.js 20+（只有從原始碼建置 GUI 時需要）。
- 硬體：Seeed XIAO RP2040，USB 身分 `03F0:0024`、Vendor HID usage `FF60:0061`。

## 快速開始

```powershell
# 1. 建置並打包（fmt／clippy／測試 → release exe → ZIP）
./scripts/build.ps1

# 2. 直接開發執行（不需要先打包）
cargo run --release -p firmware-manage-tool-app
```

產物：

| 檔案 | 用途 |
| --- | --- |
| `target/release/firmware-manage-tool-app.exe` | 主程式（Tauri 外殼，內嵌 UI） |
| `dist/FirmwareManageTool_Rust-0.1.0-windows-x64.zip` | 可攜包：exe＋韌體原始碼＋文件＋授權 |

**建置慣例**：host 建置不加 `--target`，只用 `target/debug` 與 `target/release`。
明寫 host triple 會讓 cargo 把整套依賴編兩份（曾累積到 17 GB）。要回收空間用
`./scripts/prune.ps1 -Apply`（只刪可重建的樹，不動 `target/release` 與韌體 UF2）。

## 第一次使用

1. 執行 `FirmwareManageTool.exe`（可攜包解壓後直接跑，不需要安裝）。
2. 「裝置」頁按 **重新掃描** → 選一台 → **連線**。連線後右上角顯示韌體版本與 build hash，
   若板上的 build 與程式不同會出現黃色提示。
3. 「韌體」頁按 **建置並燒錄**：程式會 `cargo build` 韌體、打包 UF2、請裝置進 BOOTSEL、
   複製檔案、重新列舉並驗證板型，全程免手動。
4. 燒錄後裝置重新列舉，重新按 **重新掃描** → **連線** 即可使用。

## 介面

| 頁面 | 內容 |
| --- | --- |
| **裝置** | 掃描／連線／中斷、能力與身分（協定、韌體版本、build、租約、NKRO／多媒體／滑鼠）、目前按住的鍵、session 數、開機次數、reset 原因、全域熱鍵狀態 |
| **即時狀態** | 150 ms 取樣的 STATUS 指標（received／completed／pending／剩餘租約／rx／tx／hid／failsafe）＋30 秒走勢、健康判定、`STATUS_EXT` 診斷（uptime、tick 遲到、佇列深度、上次故障、reset 原因、開機次數）、連線擁有者表格（`SESSION_LIST`：誰按著幾個鍵、剩餘租約、是否要求 BOOTSEL） |
| **韌體** | 韌體目錄與 `cargo` 路徑、建置進度、BOOTSEL 與複製進度、燒錄後驗證 |
| **延遲** | 1–10000 輪、模式（一般／高精度／極限 REALTIME）、逐輪結果與統計 |
| **滑鼠** | 擬人化參數（曲率、抖動、過衝、收尾停頓、最小步距、時間曲線）＋**實際軌跡預覽**、相對移動、點擊／拖曳／滾輪、文字輸入、組合鍵、巨集錄製與重播 |

**除錯模式／遊戲模式**（右上角切換，設定會保存）：

| | 除錯模式 | 遊戲模式 |
| --- | --- | --- |
| STATUS 輪詢 | 每 150 ms | 不送 |
| 診斷／擁有者清單 | 每 1 s | 不送 |
| HID 每行回覆轉到介面 | 是 | 否 |
| 心跳（維持租約） | 每 500 ms | 每 500 ms |
| 鍵盤／滑鼠／輸入 | 照常 | 照常 |

要不要輪詢只在「看數字」時有意義；遊玩或實機操作時切遊戲模式，裝置就不必回覆多餘查詢。
**同一個 IN 端點同時只能有一個輪詢者**：GUI 在除錯模式下和 `probe --watch` 同時跑，
兩邊會互偷回覆並讓自己的 session 失效（`failsafe` 增加）。

**全域熱鍵 `Ctrl+Alt+Shift+Esc`**：立即放開所有按鍵與滑鼠鍵（`Reset`）。必須真人按，
程式注入的按鍵不會觸發已註冊的熱鍵。

## 命令列工具

不需要 GUI 也能驗證裝置：

```powershell
# 走 GUI 相同的主機路徑：enumerate → Feature 14 → OPEN → STATUS → HEARTBEAT → BARRIER
cargo run --release --no-default-features --example probe
cargo run --release --no-default-features --example probe -- --ext            # 讀 STATUS_EXT 診斷
cargo run --release --no-default-features --example probe -- --keys           # 注入 'a'，確認鍵盤報告真的完成
cargo run --release --no-default-features --example probe -- --lease          # 閒置超過租約，裝置應回收 session
cargo run --release --no-default-features --example probe -- --bootloader     # 請裝置進 BOOTSEL

# 建置韌體＋打包 UF2＋印容量
bash scripts/build-rust-firmware.sh
```

## GDBOX SDK

`sdk/` 是 Host SDK，只有兩層：權威實作是 Rust 的 crate `gdb`；其他語言一律走 C ABI（`gdb.dll`＋
`include/gdb.h`），C++／Python／Go 三個薄綁定只把名字翻譯過去，本身沒有邏輯。C ABI 有兩種句柄：直接句柄
涵蓋 session、輸入、查詢、錯誤與日誌，外加 `gdb_command`／`gdb_move_raw` 兩個逃生口；附加句柄
（`gdb_attach`／`gdb_remote_*`）接上持有裝置的 broker 行程。燒錄、broker 的伺服端、watch、量測、
錄製重播與螢幕目標都只在 Rust。本工具自己也走 SDK：裝置列舉與實體 HID 通道呼叫
`gdb::Manager::list()`／`gdb::transport::HidTransport`，hidapi 由 SDK 擁有。

```powershell
cargo build -p gdb-ffi --release       # target/release/gdb.dll
cargo test  -p gdb -p gdb-motion       # 92 個測試，免硬體
bash scripts/check-sdk.sh              # Rust＋Python＋C++＋（有 Go 才跑）Go
```

- 裝置管理：列舉、以序號或路徑連線、自動重連、多裝置、逾時與 11 種錯誤碼。
- 滑鼠：相對／絕對移動、到點時間、線性／緩入／緩出／貝茲／自訂曲線、多段路徑、
  中途取消與改目標、五鍵、滾輪、拖曳、全部放開。
- 多螢幕：絕對移動用 `MotionOptions::screen` 指定主螢幕、第 i 台螢幕或某個視窗
  （`Primary`／`Monitor(i)`／`Window(hwnd)`，預設主螢幕），座標是虛擬桌面的實體像素，
  點會被夾進目標矩形，只有「沒有任何螢幕涵蓋這個點」會回 `Unsupported`。
- 鍵盤：按下／放開／單擊／長按、組合鍵、按鍵序列、文字輸入、全部放開。
- 自動化：任務佇列、延遲、週期排程、優先級、暫停／恢復／取消、錯誤收集。
- 多程式共用一支裝置：`gdb::broker` 讓一個行程持有裝置，其他行程透過具名管道
  `\\.\pipe\gdbox-<名稱>` 下指令；單一指令原子、連續指令不原子，順序由呼叫端用
  `barrier` 劃分。伺服端是 Rust，其他語言用 `gdb_attach`／`gdb_remote_*` 接上
  （C++ `gdb::Remote`、Python `gdb.Remote`、Go `Attach`）。它不提供原始 HID 回報、
  能力掃描、背景移動 worker 與燒錄。
- 熱插拔：`gdb::watch` 以 500 ms 輪詢匯流排，回報插入／移除與 BOOTSEL 磁碟出現，
  只列舉裝置、不讀寫 vendor collection，所以不會搶走擁有者的回覆。
- 量測：`device.metrics()` 提供送收幀數、重試、重連、租約故障、逾時與往返延遲
  （min／p50／p95／max）直方圖；記錄成本是一次 relaxed 原子加。
- 錄製重播：`gdb::replay` 把「意圖」存成 JSON（`version` 1）再重播，可變速、
  取消時送 RELEASE_ALL；種子與螢幕尺寸一併記錄，重播具決定性。
- 開發者：統一錯誤碼、日誌等級與 sink（`Trace` 會記下每個送出／收到的封包與被跳過的他 session 封包）、
  事件回呼、免硬體的模擬裝置、Rust 與三個綁定的範例與測試。

細節與 API 對照表：[sdk/README.md](sdk/README.md)、[sdk/API.md](sdk/API.md)。

## 疑難排解

| 現象 | 原因與處理 |
| --- | --- |
| 掃不到裝置 | 只認 `03F0:0024`＋usage `FF60:0061`。檢查線材是否只有充電、換 USB 埠、確認韌體已燒錄 |
| 「裝置忙碌」 | 同名 session 已存在，或八個名額已滿。關掉沒在用控制程式（正常結束會送 `CLOSE` 立刻歸還） |
| 連線後數字不動 | 你切在遊戲模式。切回除錯模式即恢復輪詢 |
| `failsafe` 一直增加 | 有程式沒有送 `CLOSE` 就結束（崩潰、強制結束、拔線），owner 要等 2 秒租約；或兩個程式同時輪詢同一端點 |
| 無法讀取鍵盤 HID collection | Windows 獨佔鍵盤 collection，主機端 `ReadFile` 會被拒絕。輸入驗證要看裝置端 STATUS 的 `keys`／`completed` |
| HID 進不了 BOOTSEL | 按住 BOOTSEL 上電，把 `firmware/app/target/thumbv6m-none-eabi/release/xiao-firmware.uf2` 複製到 `RPI-RP2` 磁碟 |
| 白畫面 | exe 必須以 `custom-protocol` feature 建置（`scripts/build.ps1` 已處理），否則會去連不存在的 dev server |

## 專案結構

```
src/                主機 library：v3 client（列舉與 HID 通道來自 SDK）、滑鼠軌跡、文字／組合鍵、量測、設定、背景 worker
src-tauri/          Tauri 2 外殼：20 個命令、事件、視窗與熱鍵
ui/                 React 19 + TypeScript + Vite（五頁）
shared/input-protocol/  跨語言的命令、能力、回應驗證與 wire 測試向量
firmware/engine/    協議引擎（no_std、固定容量）：session、排程、狀態、LED
firmware/app/       正式韌體：embassy-rp USB device、boot2、NeoPixel
clients/            Rust／C++／Python 最小操作端（由協議常數產生）
sdk/                GDBOX Host SDK：motion 數學、Rust 實作、C ABI、C++／Python／Go 薄綁定
scripts/            建置、打包、preflight、清理、契約與客戶端檢查、CDP 檢查
```

## 規格

- **線路協定（給要接裝置的人）**：[firmware/protocol-v3.md](firmware/protocol-v3.md)
  — framing、opcode、STATUS／STATUS_EXT／SESSION_LIST 格式、錯誤碼、租約與完成語意，
  以及 Rust／C++／Python 三個最小操作端的用法。
- **內部架構**：[docs/architecture.md](docs/architecture.md) — 主機四層、韌體六階段、建置產物規則。
- **韌體**：[firmware/README.md](firmware/README.md)、[firmware/app/README.md](firmware/app/README.md)。

## 授權

本專案程式碼採 MIT；隨附韌體與第三方程式依其原有授權，詳見 `THIRD_PARTY_NOTICES.md`。
