# 架構

這份文件只回答三件事：東西放哪、改動落在哪、哪些關卡守住契約。線路協定本身見
[firmware/protocol-v3.md](../firmware/protocol-v3.md)，SDK 的用法見
[sdk/README.md](../sdk/README.md)。

## 依賴方向

```
ui → src-tauri → src → gdb → input-protocol
```

單向，不回頭。SDK 不依賴工具，工具反過來用 SDK。

| 層 | 位置 | 內容 |
| --- | --- | --- |
| UI | `ui/` | React 19 + TypeScript + Vite，五頁；只透過 `lib/ipc.ts` 呼叫命令 |
| 外殼 | `src-tauri/` | Tauri 2：視窗、全域熱鍵、`generate_handler![…]` 的命令、`enum UiEvent`；`ui/dist` 編譯期嵌入 |
| 工具 library | `src/` | 背景 worker（獨占 HID handle，一次只做一件事）、latency、firmware、settings、uf2 |
| SDK 實作 | `sdk/rust` | 唯一的行為實作：session、心跳、絕對座標閉環、鍵鼠、排程、broker、watch、metrics、診斷、replay、flash |
| 移動數學 | `sdk/motion` | 純計算，無 I/O、固定 PRNG |
| 協議 | `shared/input-protocol` | opcode、編碼、canonical client、測試 fake |

- 一行為一實作：功能先進 `sdk/rust`，要跨語言才加 `sdk/ffi/include/gdb.h`，C++／Python／Go
  綁定只翻譯名字。工具本身也走 SDK（`gdb::Manager::list`、`gdb::transport`），hidapi 由 SDK 擁有。
- 韌體不為 SDK 改：SDK 只送既有 opcode，絕對座標、移動演算法、任務與排程都在主機。
- `Device::command` 是唯一的原樣轉送入口，它跳過 SDK 的簿記（按住的鍵、mock 游標）。

## 改動落在哪

| 你改了 | 連帶要動 |
| --- | --- |
| opcode、payload、租約語意 | `shared/input-protocol` → `firmware/engine` → `sdk/rust` 的 session，並跑協議同步檢查 |
| 一個裝置能力 | `sdk/rust`（實作）→ 需要跨語言時才加 `sdk/ffi` → 對應綁定 |
| 介面命令或事件 | `src-tauri` 的 `generate_handler!`／`UiEvent` 與 `ui/src/lib/ipc.ts`、`types.ts` 同時改 |
| 一個頁面 | `ui/`，然後重建外殼，否則跑的是舊 bundle |
| 燒錄流程 | `sdk/rust/src/flash.rs`；`src/` 只 adapt，不重寫 |

## 韌體在哪

`firmware/engine` 是協議引擎：`no_std`、無 heap、固定容量，時間一律由呼叫端傳入
`now_ms`，所以同一份程式也在主機上跑測試。`firmware/app` 是正式韌體（embassy-rp USB
device、boot2、NeoPixel），本身是獨立 workspace。

`Runtime::step`（`firmware/app/src/runtime.rs`）一輪六階段：收 OUT 報告並驗 framing →
引擎解碼 → 佇列輸出 → 1 ms 排程推進 → 送出待回覆的 vendor reply → bootloader gate、
watchdog、LED。佇列深度、租約、位移上限這些容量與時序數字見
[firmware/README.md](../firmware/README.md)。

## 四個共用關卡

| 關卡 | 守住什麼 |
| --- | --- |
| `node scripts/check-ipc.mjs` | IPC 契約：以 `generate_handler![…]` 與 `enum UiEvent` 為真相，雙向比對 `ui/src/lib/ipc.ts`／`types.ts` |
| `python scripts/sync-input-protocol.py --check`、`python scripts/check-clients.py` | 協議常數與生成的客戶端沒有走鐘 |
| `python scripts/check-bindings.py` | C ABI 表面：以 `gdb.h` 與實際匯出的符號為清單，雙向比對三個綁定的名字、ABI minor 與 by-pointer struct 的欄位（`gdb_metrics`、`gdb_clamps`、`gdb_move_report`） |
| `bash scripts/check-sdk.sh` | SDK 端到端：Rust、C++、Python、（找得到就）Go，全部對著同一份 `gdb.dll` |

四者都掛在 `scripts/preflight.sh` 裡，連同前端建置、fmt、clippy、workspace 測試與韌體
測試一起跑。任何一項紅燈都算沒完成。

## 建置產物

host 建置不加 `--target`：`target/debug` 與 `target/release` 就是全部，明寫 triple 會讓
同一套依賴編兩份。韌體產物在 `firmware/app/target/thumbv6m-none-eabi`；`ui/dist` 在
`src-tauri` 建置時嵌入。`scripts/prune.ps1` 列出可重建的樹，加 `-Apply` 才真的刪除。
