# firmware-engine（XIAO RP2040 引擎的 Rust 移植）

原本是 `firmware/boards/rp2040/src/input/` 那個 C++ 引擎的逐行為移植；那個 C++ 實作連同整個
Arduino sketch 已經在這個分支刪除，這裡就是唯一引擎。
`no_std`、無 heap、無浮點、`#![forbid(unsafe_code)]`，時間一律由呼叫端傳入 `now_ms`。

## 模組對照

| C++ | Rust | 內容 |
| --- | --- | --- |
| `InputState.h/.cpp` | `state.rs` | 31-byte 快照、實體鍵規則、`merge_sources` |
| `WireCodec.h/.cpp` | `wire.rs` | framing、opcode 驗證、回覆、Feature 14 |
| `SessionTable.h/.cpp` | `session.rs` | 8 個 slot、序號、租約 |
| `OutputScheduler.h/.cpp` | `scheduler.rs` | 32 筆佇列、合併、屏障、完成扣除 |
| `InputEngine.h/.cpp` | `engine.rs` | 命令分派、TX、故障、boot 確認 |
| `BootloaderGate.h/.cpp` | `boot_gate.rs` | 120 ms + 靜默匯流排 |
| `platform/StatusIndicator.h/.cpp` | `led.rs` | LED 優先序與計時（發布交由平台層） |
| `InputConfig.h` | `config.rs` | 8／32／32／32、120 ms、2000 ms、50 ms、±8192 |
| `KeyCatalog.h`（生成） | `keys_generated.rs`（生成） | 按鍵 bitmap、Consumer 表 |

`keys_generated.rs` 與 `tests/data/vectors_generated.rs` 由
`scripts/sync-input-protocol.py` 從 `shared/input-protocol/` 產生，勿手改。

## 驗證

```sh
cargo test -p firmware-engine                                  # 主機行為測試
cargo build -p firmware-engine --release --target thumbv6m-none-eabi   # no_std 目標（RP2040 = Cortex-M0+）
cargo test -p firmware-engine --test trace                     # golden trace
```

三層證據：

1. **行為**：`tests/behaviour.rs` 的 11 個案例（獨立實例、租約／過期邊界、計數 wrap、TX／位移溢位、
   boot generation、peer release、LED、boot gate、notices）；原始案例是逐條照被刪除的 C++ 引擎測試寫的。
2. **跨語言 wire**：`tests/golden.rs` 把 `shared/input-protocol/vectors.json` 的主機封包
   （含 report id 12 的完整 64 bytes）餵給 Rust 引擎，檢查接受的封包、NKRO／Consumer 輸出位元、
   OPEN／BARRIER 回覆。
3. **golden trace**：`tests/trace.rs` 執行一段固定腳本（2 個 session、按鍵／位移／按鈕、屏障、STATUS、
   RELEASE、50/51 ms 過期邊界、全域故障、boot 請求、租約到期），輸出 `I`／`R`／`S` 行並比對
   `tests/data/engine_trace.golden`。這份 golden 就是 C++ 引擎被刪除前逐位元組相同的那一份，
   因此它同時是回歸測試與歷史紀錄。腳本故意破壞測試驗證過：鍵盤 report 長度 29 → 28、
   或 `STALE_MS` 50 → 49，都會在第 1／23 行失敗。

### 更新 golden

只有**刻意的行為變更**才可重生 golden，而且要在 commit message 說明為什麼舊行為是錯的：

```sh
cargo test -p firmware-engine --test trace -- --nocapture > /tmp/trace.txt
diff -u firmware/engine/tests/data/engine_trace.golden /tmp/trace.txt
```

C++ 端已經不存在，沒有「兩邊一起改」這回事；golden 異動就代表裝置行為改變。

## 記憶體

靜態用量與 C++ 引擎幾乎相同（`arm-none-eabi-g++ -mcpu=cortex-m0plus -Os` 量 `sizeof`，
Rust 用 `tests/sizes.rs`）：

| 型別 | C++ | Rust |
| --- | ---: | ---: |
| InputState | 31 | 31 |
| Session | 60 | 56 |
| OutputScheduler | 10,072 | — |
| InputEngine | 12,600 | 12,564 |

`tests/sizes.rs` 會把這些值固定成斷言，結構長大就會讓 `cargo test` 失敗。
Sketch 目前總靜態 RAM 為 29,712 bytes（含 Arduino／TinyUSB），所以引擎層的記憶體風險已排除；
換成 Rust USB 時要重新量的是 USB stack 與 async futures，不是引擎。

## 已經接上的部分

- USB 傳輸層與韌體入口：`firmware/app`（embassy-rp／embassy-usb，`src/main.rs`）。
- runtime 主迴圈：`firmware/app/src/runtime.rs` 的 `Transport` trait + `Runtime::step`，
  與原本 C++ `FirmwareRuntime::loop` 同一個階段順序。

## 尚未接上的部分

- NeoPixel 渲染（原本在 C++ 的 core 1）：`LedState` 已由引擎產生，但沒有人驅動 WS2812。
