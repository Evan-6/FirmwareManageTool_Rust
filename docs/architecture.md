# 架構

主機與韌體都是純 Rust。這份文件說明「東西放在哪、為什麼這樣切、改了會影響什麼」；
線路協定本身見 [firmware/protocol-v3.md](../firmware/protocol-v3.md)。

## 主機：四層

```
ui/                  React 19 + TS + Vite（只透過 lib/ipc.ts 呼叫命令）
  │  Tauri invoke / app://event
src-tauri/           Tauri 2 外殼：視窗、全域熱鍵、20 個命令、UiEvent
  │  AppCommand / AppEvent
src/backend.rs       背景 worker：獨占 HID handle，一次只做一件事
  │
src/                 library：hid（列舉／v3 client）、mouse、typing、latency、firmware、settings
```

- **worker 獨占裝置**：GUI 只保存顯示狀態，所有裝置操作排進同一條命令通道，任何時刻只有一項
  工作在跑；取消用原子旗標，BOOTSEL 選擇走獨立 channel，長工作不阻塞介面。
- **連線期間自動輪詢**：150 ms STATUS、1 s STATUS_EXT／SESSION_LIST、500 ms 心跳。命令執行期間
  輪詢暫停，結束後恢復。除錯模式關閉時全部停止，只留心跳（見下）。
- **契約由腳本守住**：`scripts/check-ipc.mjs` 以 `generate_handler![...]` 與 `enum UiEvent` 為真相，
  雙向比對 `ui/src/lib/ipc.ts`／`types.ts`；命令或事件改了卻沒同步，檢查直接紅。
- **UI 是編譯期嵌入**：`src-tauri` 以 `custom-protocol` 把 `ui/dist` 打進 exe，所以 release 只有一支
  執行檔；改完前端一定要重建外殼，否則跑的是舊 bundle。

## GDBOX SDK：主機端的第二個入口

`sdk/` 把同一套主機能力包成給外部程式用的 SDK，與 GUI 平行、互不依賴：

```
sdk/motion/     gdb-motion：純移動數學（整數友善、固定 PRNG、無 I/O）
sdk/rust/       gdb：session、心跳、絕對座標閉環、鍵鼠、排程
sdk/ffi/        gdb-ffi：C ABI（include/gdb.h）+ cdylib
sdk/cpp|python|go   薄綁定，只做名稱轉換，全部呼叫同一份 C ABI
```

- **一份實作、四種語言**：行為只在 Rust 寫一次，綁定不得自行補邏輯，否則四份會漂移。
- **韌體沒有為了 SDK 改動**：SDK 只送既有 opcode；絕對座標、移動演算法、任務與排程都在主機。
- **模擬裝置有虛擬游標**：免硬體的測試可以完整跑 `move_to`／`move_path`／`retarget`。
- **驗證**：`bash scripts/check-sdk.sh`（已納入 `preflight.sh`）。

## 除錯模式與遊戲模式

`Settings::debug` 決定 worker 對裝置的說話量：

| | 除錯模式（預設） | 遊戲模式 |
| --- | --- | --- |
| STATUS | 每 150 ms | 不送 |
| STATUS_EXT／SESSION_LIST | 每 1 s | 不送 |
| HID 每行回覆轉到介面 | 是 | 否 |
| HEARTBEAT | 每 500 ms | 每 500 ms |
| 命令 | 照常 | 照常 |

心跳不可省：租約 2000 ms，少了心跳裝置兩秒後就回收 session，`failsafe` 也會增加。實測遊戲模式
放著 45 秒，`received` 凍結、`failsafe` 仍為 0，切回除錯模式立刻恢復。

輪詢還有一個必須知道的副作用：同一個 vendor IN 端點只能有一個讀者，兩個程式同時讀會互偷回覆，
被偷的一方序號錯亂就讓自己的 session 失效。要看裝置數字時只留一個輪詢者。

## 韌體：六階段主迴圈

```
USB OUT (id 12) → Runtime::step → engine → 排程 → USB IN (id 13) / 輸入 HID (id 2,3,5)
```

`Runtime::step` 在 `firmware/app/src/runtime.rs`，與傳輸層無關（測試用 mock transport）：

1. 收 OUT 報告、驗 framing
2. 交給 `firmware-engine` 解碼（opcode、session、sequence）
3. 佇列輸出（鍵盤／consumer／滑鼠，移動可合併）
4. 每 1 ms tick 推進排程，送出到期的 HID 報告
5. 送出待回覆的 vendor reply（Feature 14、STATUS、STATUS_EXT、SESSION_LIST 等）
6. 處理 bootloader gate、watchdog、LED 狀態

引擎是 `#![forbid(unsafe_code)]`、`no_std`、無 heap、固定容量；所有時間由呼叫端傳入 `now_ms`，
只用 `wrapping_*`，因此同一份程式也能在主機上跑測試。

## Session 與租約

- 八個 slot，各自獨立 session 與 sequence；OPEN 用非零 id、sequence=1，之後每個命令遞增。
- 租約 2000 ms，OPEN／輸入／HEARTBEAT／診斷查詢續租，**STATUS 與 BARRIER 不續租**。
- 正常結束送 `CLOSE`：立刻歸還名額、取消自己待送工作、只放開自己按住的鍵、不發 EVENT、不計
  `failsafe`。其他 session 完全不受影響。
- 重複／缺號只讓該 session 失效（避免重播相對位移）；未知 session 的輸入丟棄。
- 鍵盤／consumer／滑鼠按鈕是所有 session 的位元聯集；snapshot 只替換自己的狀態。
- 全域故障（USB reset／拔線／suspend、佇列溢位）讓所有 session 失效，各發一個 EVENT。

## 時序與容量

| 項目 | 值 |
| --- | --- |
| 主迴圈 | core 0，`Ticker::every(1 ms)`，每 tick 量 lateness 餵給 STATUS_EXT |
| 佇列 | output／RX／TX 各 32 項 |
| 最舊待送 | 超過 50 ms 即失效、清除並釋放 |
| 移動累計 | 單軸上限 ±8192，超過即失效 |
| Bootloader gate | 120 ms ＋ output 閒置 ＋ vendor ready ＋ TX 清空 |
| Watchdog | 2 s；reset reason 與開機次數存在 watchdog scratch |

## 建置產物放在哪裡

cargo 依「profile」和「有沒有指定 `--target`」分開放產物。在 Windows 上 host triple 本來就是
`x86_64-pc-windows-msvc`，一旦明寫 `--target`，build script／proc macro 留在 `target/<profile>`，
其餘留在 `target/<triple>/<profile>`——同一套依賴編兩份。所有腳本都不加 `--target`；
`scripts/build.ps1` 改成檢查預設 toolchain 的 host 是否為 MSVC，不是就報錯。

| 位置 | 內容 | 處置 |
| --- | --- | --- |
| `target/release` | 應用程式（唯一正式產物） | 保留 |
| `target/thumbv6m-none-eabi` | `firmware-engine` 的嵌入式建置 | 保留（<1 MB） |
| `firmware/app/target/thumbv6m-none-eabi` | 韌體 ELF 與 UF2 | 保留（要燒錄） |
| `target/debug` | 測試與 clippy 的 dev 產物 | 可刪，下次重建 |
| `firmware/app/target/debug` | 韌體 host 測試的 dev 產物 | 可刪 |

`scripts/prune.ps1` 列出可重建的樹，`-Apply` 才真的刪除。清完後整份 repo 約 2.1 GB。

## 品質門檻

`scripts/preflight.sh` 依序跑：前端建置 → `cargo fmt --check` → clippy（host 與 thumbv6m）→
workspace 測試 → 韌體 adapter／descriptor 測試 → 協議與客戶端同步 `--check` →
客戶端檢查（Python 行為、Rust 編譯、C++ 語法）→ IPC 契約檢查。
任何一項失敗就不算完成；CI 的 windows job 跑同一組檢查，Linux job 只驗純邏輯與韌體編譯。
