# 韌體（Seeed XIAO RP2040）

主機透過 USB 下指令，裝置以真正的 HID 鍵盤／滑鼠身分送出輸入。沒有 CDC 序列埠、沒有 COM 埠，
唯一的控制通道是 Vendor HID（usage page `0xFF60`），協議 3.2.0。

純 Rust：`firmware/app`（crate `xiao-firmware`，embassy-rp + embassy-usb）加 `firmware/engine`
（協議引擎）。建置鏈只需要 Rust：boot2 由 embassy-rp 提供，UF2 由 `uf2pack` 打包，不需要額外的
燒錄工具或 C 編譯器。

- 線路協定：[protocol-v3.md](protocol-v3.md)
- USB：`03F0:0024`、產品字串 `HP Keyboard`、bcdDevice `0x0310`、序號取自 flash unique id
- 容量：Flash 32,724 B（32.0 KiB／2 MiB）、RAM 30,684 B（30.0 KiB／264 KiB）
- UF2：65,536 B、128 blocks、每塊 256-byte 對齊、首塊 `0x10000000`

## 資料夾

```
firmware/
  README.md       ← 本檔
  protocol-v3.md  ← Vendor HID v3 規範（framing、opcode、診斷、租約）
  engine/         協議引擎：no_std、無 heap、固定容量，可在主機上測試
  app/            正式韌體：USB device、runtime loop、NeoPixel
```

`firmware/engine/src/` 是唯一引擎實作。`keys_generated.rs` 由
`scripts/sync-input-protocol.py` 從 `shared/input-protocol/keys.json` 產生，不要手改。
USB report descriptor 凍結在 `firmware/app/src/usb_descriptors.rs`，由
`firmware/app/tests/descriptors.rs` 以雜湊釘住——改了會直接測試失敗。

## 建置與燒錄

```sh
rustup target add thumbv6m-none-eabi
bash scripts/build-rust-firmware.sh          # cargo build + uf2pack + 印容量
cp firmware/app/target/thumbv6m-none-eabi/release/xiao-firmware.uf2 /e/   # E: = RPI-RP2
```

管理工具會自動完成這整段：`cargo build` → 打包 UF2 → 請裝置進 BOOTSEL → 複製檔案 → 重新列舉
並驗證板型。第一次燒錄、或無法由 HID 進入下載模式時，按住 BOOTSEL 上電後手動複製 UF2。
正常韌體沒有 COM，也沒有 1200 bps touch reset。

## 行為

- **八個 session**：GUI／MSCV／EvanRemote 可同時連。正常結束送 `CLOSE` 立刻歸還名額，
  不留給租約回收、也不計 `failsafe`。
- **租約 2000 ms**：OPEN、輸入、HEARTBEAT 與診斷查詢續租；STATUS／BARRIER 不續租。
  租約是「程式崩潰或直接拔線」的保險，正常結束請送 CLOSE。
- **佇列**：output／RX／TX 各 32 項；最舊待送超過 50 ms 或溢位即失效並釋放。
  相對位移與滾輪單軸累計上限 ±8192。
- **安全**：watchdog 2 s；USB suspend／reset 視為 bus loss（故障碼 7），所有 session 失效並放開
  按住的鍵——闔蓋時按著 W，醒來不該繼續走。
- **狀態燈**：NeoPixel 以 PIO + DMA 驅動（GPIO12 資料、GPIO11 電源閘），固定亮度，顏色代表
  Idle／Held／Error／Failsafe／Bootloader。
- **Feature 14**：board id 固定 2、capabilities 15，帶 build hash（commit 前四 bytes），
  主機用來比對版本。
- **最小操作端**：`clients/{rust,cpp,python}` 由協議常數自動產生，只送命令、不管管理功能；
  見 [protocol-v3.md](protocol-v3.md#最小操作端rustc-python)。
