# 驗證紀錄

日期：2026-10-06（Asia/Taipei）。開發主機為 Linux，沒有連接 AVR／RP2040 或 Windows 實機。
以下編譯與模擬結果不等同 USB／Windows 實機驗收。

## 本次已完成

- `cargo fmt --all -- --check`。
- 管理工具 Linux／Windows MSVC 目標的 workspace Clippy：`--all-targets -- -D warnings`。
- 管理工具 Linux workspace all-targets：主程式 36 項、共享協定 10 項測試通過。
  no-default-features：主程式 35 項、共享協定 10 項通過。
- EvanRemote Linux：core 13、hardware-input 33、共享協定 10 項測試通過。
- EvanRemote 修改的 Rust 檔案以 rustfmt（skip_children）檢查；未格式化無關的既有檔案。
- EvanRemote 輸入相關 crate 的 Clippy 在允許五項既有 core lint 後通過：
  derivable_impls、io_other_error、result_unit_err、type_complexity、too_many_arguments。
  整個 EvanRemote 的無例外 fmt／Clippy 仍不是通過狀態。
- Node 18 項；Playwright 1.57 Chromium 操作測試通過，包含實體鍵封包、模式切換、失焦／拖曳與網頁既有回歸。
- 按鍵表／固定封包向量生成與跨 repo 漂移檢查：`sync-input-protocol.py --check --peer ../EvanRemote`。
- C++ 實際 RP2040 韌體配合 USB 替身：`scripts/test-rp2040.sh`（C++17、Wall／Wextra／Werror）。
  同一套測試另以 AddressSanitizer／UndefinedBehaviorSanitizer 執行通過。
- arduino-cli 1.5.1、arduino-pico **6.2.0** 與其內附 TinyUSB、Adafruit NeoPixel 1.15.5：Pico 與 XIAO RP2040 編譯通過。
- 本地 UF2 產物：`dist/rp2040-v3/pico-3.0.0.uf2`、`xiao-3.0.0.uf2`；BUILD.txt 記錄 SHA256 與來源指紋。
  Pico 91872 bytes flash／19424 bytes RAM，XIAO 94696 bytes flash／19516 bytes RAM。
- 管理工具與 EvanRemote server Windows x86_64 MSVC **Release 交叉建置通過**。
  使用 cargo-xwin 0.23.1、clang-cl／lld-link 14；尚未在 Windows 執行。
- Windows／Linux 輸入 CI、Pico／XIAO 固定 core 建置 CI、可攜式套件納入共享按鍵表與授權。
  workflow 與 PowerShell 打包修改未在 GitHub Actions／Windows 原生環境執行。

硬體 crate 所需 libudev 開發標頭與 pkg-config 在 `/tmp/rp2040-redesign/udev` 準備，
執行時使用 PKG_CONFIG_PATH 與 CFLAGS，不修改系統套件。一般開發主機可直接安裝 libudev-dev。
MSVC 靜態 CRT 缺少 Microsoft 私有 PDB 的 LNK4099 警告不影響 Release 連結成功。

## 自動測試涵蓋

- 主鍵區／數字鍵盤數字、Enter／KP Enter、左右修飾鍵、ISO／JIS、Consumer 與超過六鍵的識別。
- v3 固定向量、framing／版本／長度、保留填充、混用 k 與 p/u（含 null）、短寫入、舊 session 回覆、控制 timeout／取消。
- 實際韌體的同毫秒短按、無逐鍵 ACK、延後完成屏障、NKRO／Consumer、優先釋放與 in-flight 取消。
- 滑鼠合併數值守恆及按鈕邊界、v2 六鍵限制、v2／v3 互斥與完成釋放後立即交接。
- 租約、重複／缺號、佇列溢位／50ms 過期、USB completion 失敗、拔線與重連不重播。
- bootloader 等待輸入釋放與回覆 USB 完成後才排定重啟。
- session ownership、失敗釋放重試／延後模式切換、舊版回退、閒置維護與去重長等待續租。
- 既有 ACK 分段／合併、子程序期限／取消、燒錄歧義／重試、滑鼠與延遲統計回歸。

鍵盘 descriptor 與 USB 替身不取代 Windows driver 或實體輸入測試。延遲終點仍為
Windows GetAsyncKeyState 觀察到輸入，沒有改成 MCU ACK／屏障時間。

## 待 Windows／USB 實機驗收

- [ ] Pico／XIAO 燒錄、USB 兩介面列舉、唯一序號與 Feature 14 能力、鎖定鍵 LED output。
- [ ] 主鍵區／數字鍵盤各數字與 Enter、左右修飾鍵；Num Lock 開／關不遺失來源。
- [ ] ISO／JIS 與七種 Consumer 鍵；超過六鍵同按、同毫秒短按與快速點擊。
- [ ] 點擊、五鍵、拖曳、雙軸滾輪、相對／絕對滑鼠、取消、失焦與模式切換。
- [ ] 多 session ownership、拔線／USB 擁塞、租約釋放、完成屏障與重新連線空狀態。
- [ ] 舊工具／MSCV 控制新版 RP2040；新版工具控制舊 RP2040／AVR；v2／v3 互斥與交接。
- [ ] PowerShell v2 bootloader 入口、管理工具燒錄後板型驗證、多裝置與下載失敗。
- [ ] Windows 原生 fmt／Clippy／測試／Release／打包、GUI 中文與 manifest／DPI、EvanRemote 軟體輸出。
- [ ] A 鍵一般／高精度／極限量測、取消／異常清理與真實按鍵到達延遲。

AVR 與 MSCV 未修改；其既有實機描述不是本次新版 RP2040 實機驗證證據。
