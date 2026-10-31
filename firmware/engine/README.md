# firmware-engine

Vendor HID v3 的協議引擎：`no_std`、無 heap、無浮點、`#![forbid(unsafe_code)]`，時間一律由呼叫端傳入
`now_ms`，因此同一份程式也能在主機上跑測試。線路協定見 [`../protocol-v3.md`](../protocol-v3.md)。

## 模組

| 檔案 | 內容 |
| --- | --- |
| `state.rs` | 31-byte 快照、實體鍵規則、`merge_sources` |
| `wire.rs` | framing、opcode 驗證、回覆、Feature 14 |
| `session.rs` | 8 個 slot、序號、租約 |
| `scheduler.rs` | 32 筆輸出佇列、合併、屏障、完成扣除 |
| `engine.rs` | 命令分派、TX、故障、boot 確認、STATUS／STATUS_EXT／SESSION_LIST |
| `boot_gate.rs` | 進入 BOOTSEL 前的 120 ms ＋ 靜默匯流排條件 |
| `led.rs` | `LedState` 優先序與計時（實際輸出由平台層負責） |
| `config.rs` | 8／32／32／32、120 ms、2000 ms、50 ms、±8192 |

`keys_generated.rs` 與 `tests/data/vectors_generated.rs` 由 `scripts/sync-input-protocol.py` 從
`shared/input-protocol/` 產生，勿手改。

## 驗證

```sh
cargo test -p firmware-engine                                          # 主機行為與 wire 測試
cargo build -p firmware-engine --release --target thumbv6m-none-eabi   # no_std 目標（Cortex-M0+）
```

- `tests/behaviour.rs`：session 生命週期、租約／過期邊界、計數 wrap、TX／位移溢位、boot generation、
  peer release、LED、boot gate、notices、STATUS_EXT、SESSION_LIST。
- `tests/golden.rs`：把 `shared/input-protocol/vectors.json` 的主機封包（含 report id 12 的完整 64 bytes）
  餵給引擎，檢查接受的封包、NKRO／Consumer 輸出位元、OPEN／BARRIER 回覆。
- `tests/sizes.rs`：`InputState` 31 B、`Report` 63 B、`InputEngine` 的靜態上限等 layout 斷言。

## 記憶體

`tests/sizes.rs` 把所有 wire 可見的 layout 與引擎預算固定成斷言，結構長大就讓 `cargo test` 失敗：

| 型別 | 現在 |
| --- | ---: |
| `InputState` | 31 |
| `Session` | ≤ 64 |
| `InputReport` | 31 |
| `InputEngine` | ≤ 64 KiB |

韌體整體的 Flash／RAM 數字由 `scripts/build-rust-firmware.sh` 或 `scripts/elf-size.py` 量測後寫在
`firmware/app/README.md`。

## 接線

`firmware/app` 的 `Transport` trait 提供 USB 與 flash 事實，`Runtime::step` 每毫秒呼叫引擎一次；
NeoPixel 由同一核心以 PIO + DMA 驅動。
