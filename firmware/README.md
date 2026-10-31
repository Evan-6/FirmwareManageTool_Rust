# FirmwareManageTool 鍵盤／滑鼠韌體

低延遲 USB HID 韌體：主機透過 USB 下指令，裝置以硬體鍵盤與滑鼠身分送出輸入。
指令走 **Vendor HID**（usage page `0xFF60`），**不列舉 CDC 序列埠、對外不露出 COM 埠**；
提供鍵盤、相對滑鼠、5 鍵、垂直／水平滾輪，以及 failsafe 自動放開。
**可同時連線八個控制程式**（GUI／MSCV／EvanRemote）：正常結束送 `CLOSE` 立即歸還名額，
不會留給租約回收，也不計 failsafe。

本分支只支援 **Seeed XIAO RP2040**，而且**只有 Rust**：Arduino sketch、TinyUSB 與 C++ 引擎
已全部移除，正式韌體是 `firmware/app`（crate `xiao-firmware`）加 `firmware/engine`。
線路協定索引見 **[PROTOCOL.md](PROTOCOL.md)**：只接受 Vendor HID v3，最多 8 個 session（OPEN／CLOSE），沒有 v2／serial 回退。

## 資料夾架構

```
firmware/
  README.md            ← 本檔
  PROTOCOL.md          ← 線路協定索引
  protocol-v3.md       ← Vendor HID v3 規範
  engine/              ← 協議引擎（no_std、無 heap、固定容量）+ 主機測試
  app/                 ← 正式韌體：embassy-rp USB device、boot2、UF2 打包
  tools/
    vendor_hid_bootloader.ps1  ← 透過 Vendor HID 送指令（含 enter_bootloader）
```

`firmware/engine/src/` 是唯一引擎實作；`keys_generated.rs` 由
`scripts/sync-input-protocol.py` 從 `shared/input-protocol/keys.json` 產生，勿手改。
USB report descriptor 凍結在 `firmware/app/src/usb_descriptors.rs`，由
`firmware/app/tests/descriptors.rs` 以雜湊釘住，改了會直接測試失敗。

## 建置與燒錄

```sh
rustup target add thumbv6m-none-eabi
bash scripts/build-rust-firmware.sh                  # 建置 + 打包 UF2 + 印出容量
cp firmware/app/target/thumbv6m-none-eabi/release/xiao-firmware.uf2 /e/   # E: = RPI-RP2
```

沒有 Arduino／Pico SDK 依賴，也不需要 picotool：boot2 由 embassy-rp 提供，
UF2 由 `uf2pack`（同一個 GUI 用的 Rust 程式碼）產生。

管理工具會自動 `cargo build`、打包 UF2、請裝置進 BOOTSEL、複製檔案，最後用 hello 驗證板型。
第一次燒錄或無法由 HID 進入下載模式時，按住 BOOTSEL 上電後手動複製 UF2。
正常韌體沒有 COM，也沒有 1200bps-touch reset。

## 容量與平台

output／RX／TX 佇列深度均為 32，8 個 session；滿載或最舊待送工作超過 50ms 即回報故障並優先釋放。
USB 完成由 embassy 的 IN transfer 完成事件通知；個別釋放保留其他來源，USB／共用佇列故障才清理全部 session。
目前全部在 core 0（`Ticker::every(1 ms)`），LED 狀態由引擎決定、尚未驅動 WS2812。
Feature 14 的 board id 固定為 2，USB 序號取自 flash unique id。

## 量測

| 映像 | Flash | RAM |
| --- | --- | --- |
| 舊 C++ sketch（arduino-pico 6.2.0 + TinyUSB，已刪除） | 90,792 B | 29,712 B |
| Rust `firmware/app` | 28,292 B | 17,804 B |
