# `xiao-firmware` — XIAO RP2040 韌體

裝置唯一的實作。協議引擎、runtime loop、USB device stack 與開機路徑全部是 Rust；
`no_std`、無 heap、固定容量。

```
USB host
   │
   ├─ interface 0：輸入 HID（NKRO id 2、consumer id 5、相對滑鼠 id 3）
   └─ interface 1：vendor HID（OUT 12、IN 13、Feature 14）
        │
   embassy-usb（HID class、64-byte report、1 ms poll）
        │
   Runtime::step（src/runtime.rs，與傳輸層無關，六階段）
        │
   firmware-engine（no_std、無 heap、固定容量）
```

## 建置

```bash
rustup target add thumbv6m-none-eabi
bash scripts/build-rust-firmware.sh
```

這條鏈不需要額外的 C 編譯器或燒錄工具：`cargo build` 產生 ELF，
`uf2pack`（和 GUI 共用同一份 Rust 程式碼）打包 UF2，`scripts/elf-size.py` 印出容量。

| 項目 | 大小 |
| --- | --- |
| Flash | 33,736 B（32.9 KiB／2 MiB） |
| RAM | 30,796 B（30.1 KiB／264 KiB） |
| UF2 | 67,584 B、132 blocks |

RAM 主要是引擎（`InputEngine` 12,564 B）與 32 筆 RX 佇列（2,016 B），其餘是 USB stack 與緩衝。

## 燒錄

把板子放進 BOOTSEL，把 UF2 複製到 `RPI-RP2` 磁碟；或 `cargo run --release --features hardware`
（`.cargo/config.toml` 的 runner 是 `probe-rs run --chip RP2040`）。

## 硬體驗證

`examples/probe.rs` 走 GUI 相同的主機路徑；`examples/boot.rs` 只用 raw vendor report 就能讓
Feature 14 壞掉時仍進得去 BOOTSEL：

```bash
cargo run --release --no-default-features --example probe                # Feature 14 → OPEN → STATUS → HEARTBEAT → BARRIER
cargo run --release --no-default-features --example probe -- --keys      # 注入 'a'，確認鍵盤報告真的完成
cargo run --release --no-default-features --example probe -- --lease     # 閒置超過租約，裝置必須回收 session
cargo run --release --no-default-features --example probe -- --ext       # STATUS_EXT 診斷
cargo run --release --no-default-features --example probe -- --ping 20   # 量往返時間
cargo run --release --no-default-features --example probe -- --settings  # 讀持久設定
cargo run --release --no-default-features --example boot                # 救援用 BOOTSEL
```

`--keys` 的判準是裝置端數字：`keys="a"`、`completed` 前進、按著時 `p=1`，放開後 `keys=""`、
`p=0`。Windows 獨佔鍵盤 collection，主機端 `ReadFile` 會拿到 `ERROR_ACCESS_DENIED`，所以輸入
路徑只能從裝置側證明；要驗 OS 端就得真的打字到有焦點的視窗，目前沒有自動化。

## 測試

- `tests/runtime.rs`：5 個 adapter 測試（mock transport）——每輪最多四個 RX 封包、輸入報告被拒
  產生 HID 故障、bus reset 產生 failsafe 事件並丟棄過期 RX、framing 故障計數、BOOTLOADER 回覆
  經 120 ms gate 才重啟。
- `tests/descriptors.rs`：以 FNV-1a 釘住輸入與 vendor report descriptor，改了會失敗。
- `firmware-engine`：行為測試、golden vectors、37 行回歸 trace、靜態大小斷言。
- host 與 `thumbv6m-none-eabi` 的 clippy 都必須 `-D warnings` 通過，`cargo fmt --check` 亦然。

## 已驗證／未驗證

實機驗證過（XIAO RP2040，序號 `4250304B38353817`）：列舉、Feature 14 回滿 64 bytes、
OPEN／STATUS／HEARTBEAT／BARRIER／CLOSE、按鍵注入與報告完成、租約回收、BOOTSEL 重啟、
重燒後設定仍在（亮度 8 → 重燒 → 仍是 8）、STATUS_EXT／PING、UF2 每塊 256-byte 對齊。

尚未驗證：NeoPixel 的實際顏色與亮度只用設定讀回確認過，沒有肉眼或相機比對，PIO/DMA 波形也沒量；
`set_report` 的鎖定鍵只確認裝置有接收並回報在 STATUS_EXT，沒有觀察 Windows 是否真的送；
滑鼠位移與擬人化的實際軌跡沒有獨立量測（預覽與送出節點由同一個函式產生）。

## 檔案

| 檔案 | 角色 |
| --- | --- |
| `src/runtime.rs` | `Transport` trait + `Runtime::step`：六階段主迴圈，與傳輸層無關 |
| `src/usb_descriptors.rs` | 凍結的 report descriptor（不要手改） |
| `src/main.rs` | embassy-rp USB device、HID interface、`Ticker` 驅動的引擎迴圈、NeoPixel、flash 設定 |
| `../engine/` | 與主機測試共用的協議引擎 |
| `memory.x`、`build.rs` | RP2040 連結配置（boot2、2 MiB flash、264 KiB RAM）與 build hash |
