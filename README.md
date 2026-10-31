# FirmwareManageTool — XIAO RP2040 鍵盤／滑鼠管理工具

Windows 桌面程式＋裝置韌體。程式透過 USB 對 Seeed XIAO RP2040 下指令，裝置以真正的
HID 鍵盤／滑鼠身分送出輸入，延遲約 1.5–1.8 ms（1 ms poll，實測值）。沒有 COM 埠、沒有
序列埠回退、沒有背景服務。

- **圖形介面**：Tauri 2 + React 19 + TypeScript，單一 exe，不連網路。
- **韌體**：純 Rust（embassy-rp + embassy-usb），`no_std`、無 heap，33.7 KiB flash／30.1 KiB RAM。
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
| **即時狀態** | 150 ms 取樣的 STATUS 指標（received／completed／pending／剩餘租約／rx／tx／hid／failsafe）＋30 秒走勢、健康判定、`STATUS_EXT` 診斷（uptime、tick 遲到、佇列深度、上次故障、reset 原因、開機次數、鎖定鍵）、連線擁有者表格（`SESSION_LIST`：誰按著幾個鍵、剩餘租約、是否要求 BOOTSEL） |
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
cargo run --release --example probe
cargo run --release --example probe -- --ext            # 讀 STATUS_EXT 診斷
cargo run --release --example probe -- --keys           # 注入 'a'，確認鍵盤報告真的完成
cargo run --release --example probe -- --lease          # 閒置超過租約，裝置應回收 session
cargo run --release --example probe -- --ping 20        # 量裝置往返時間
cargo run --release --example probe -- --settings       # 讀持久設定（亮度、開機次數）
cargo run --release --example probe -- --brightness 8   # 設定 NeoPixel 亮度（0–64）
cargo run --release --example probe -- --bootloader     # 請裝置進 BOOTSEL
cargo run --release --example boot                      # 只用 raw report 進 BOOTSEL（Feature 14 壞掉時的救援）

# 建置韌體＋打包 UF2＋印容量
bash scripts/build-rust-firmware.sh
```

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
src/                主機 library：HID 列舉與 v3 client、滑鼠軌跡、文字／組合鍵、量測、設定、背景 worker
src-tauri/          Tauri 2 外殼：20 個命令、事件、視窗與熱鍵
ui/                 React 19 + TypeScript + Vite（五頁）
shared/input-protocol/  跨語言的命令、能力、回應驗證與 wire 測試向量
firmware/engine/    協議引擎（no_std、固定容量）：session、排程、狀態、LED
firmware/app/       正式韌體：embassy-rp USB device、boot2、NeoPixel、flash 設定
scripts/            建置、打包、preflight、清理、契約檢查、CDP 檢查
```

## 規格

- **線路協定（給要接裝置的人）**：[firmware/protocol-v3.md](firmware/protocol-v3.md)
  — framing、opcode、STATUS／STATUS_EXT／SESSION_LIST 格式、錯誤碼、租約與完成語意。
- **內部架構**：[docs/architecture.md](docs/architecture.md) — 主機四層、韌體六階段、建置產物規則。
- **韌體**：[firmware/README.md](firmware/README.md)、[firmware/app/README.md](firmware/app/README.md)。

## 授權

本專案程式碼採 MIT；隨附韌體與第三方程式依其原有授權，詳見 `THIRD_PARTY_NOTICES.md`。
