# firmware-engine（XIAO RP2040 引擎的 Rust 移植）

`firmware/boards/rp2040/src/input/` 的 C++ 引擎的逐行為移植，供最終取代 C++ 實作。
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
bash scripts/test-rp2040.sh                                   # C++ 引擎測試 + A/B trace diff
```

三層證據：

1. **行為**：`tests/behaviour.rs` 的 11 個案例逐條對應
   `firmware/tests/rp2040_engine_test.cpp`（獨立實例、租約／過期邊界、計數 wrap、TX／位移溢位、
   boot generation、peer release、LED、boot gate、notices）。
2. **跨語言 wire**：`tests/golden.rs` 把 `shared/input-protocol/vectors.json` 的主機封包
   （含 report id 12 的完整 64 bytes）餵給 Rust 引擎，檢查接受的封包、NKRO／Consumer 輸出位元、
   OPEN／BARRIER 回覆。
3. **逐位元組 A/B**：`firmware/tests/engine_trace.cpp` 與 `tests/trace.rs` 執行同一份腳本
   （2 個 session、按鍵／位移／按鈕、屏障、STATUS、RELEASE、50/51 ms 過期邊界、全域故障、
   boot 請求、租約到期），輸出 `I`／`R`／`S` 行並比對 `firmware/tests/engine_trace.golden`。
   腳本故意破壞測試驗證過：把鍵盤 report 長度 29 → 28、或 `STALE_MS` 50 → 49，都會在第 1／23 行失敗。

### 更新 golden

只有**刻意的行為變更**才可重生 golden，且必須同時更新 C++ 與 Rust 的腳本：

```sh
c++ -std=c++17 -Ifirmware/tests/stubs firmware/boards/rp2040/src/input/*.cpp \
    firmware/tests/engine_trace.cpp -o /tmp/engine-trace && /tmp/engine-trace > firmware/tests/engine_trace.golden
```

`scripts/test-rp2040.sh` 會重建 C++ 端並以 `diff --strip-trailing-cr` 比對，確保 golden 沒有被
單邊修改。

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

## 尚未移植

- USB 傳輸層（TinyUSB、Pico SDK queue、callback 橋接）：仍在 `src/platform/`。
- NeoPixel 渲染（core 1）：仍在 `src/platform/StatusLedRenderer.cpp`。
- 韌體入口與 `FirmwareRuntime` 主迴圈：目前仍由 C++ 組裝；若要直接跑 Rust 引擎，
  需要一個 `#[no_mangle] extern "C"` 薄層或把 runtime 一併移植。
