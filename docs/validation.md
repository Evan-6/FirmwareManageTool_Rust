# 驗證紀錄

日期：2026-10-05（Asia/Taipei）。本次於 Linux 開發主機驗證，沒有連接 Leonardo
或 RP2040 實機；以下結果不等同 Windows／USB 實機驗收。

## 已完成

- Rust 1.98.1、Cargo.lock 固定依賴。
- `cargo fmt --all -- --check`。
- Linux `cargo clippy --locked --all-targets -- -D warnings`。
- Linux `cargo test --locked --all-targets`：32 個測試。
- Linux `cargo test --locked --no-default-features`：31 個純邏輯／子程序測試。
- Windows MSVC 目標 `cargo xwin clippy --locked --target x86_64-pc-windows-msvc --all-targets -- -D warnings`。
- Windows 測試執行檔交叉編譯（`cargo xwin test ... --all-targets --no-run`）；未在 Windows 執行。
- Windows MSVC Release 交叉建置（cargo-xwin 0.23.1、clang-cl／lld-link 14）。
- 產物檢查：PE32+、x86-64、Windows GUI subsystem；manifest 包含 PerMonitorV2
  與 asInvoker。靜態 CRT，不需要額外 MSVC runtime DLL。
- Linux Xvfb／Wgpu 軟體 Vulkan 啟動與四頁導航；預覽圖位於 `screenshots/`。
- 可攜式 ZIP 打包、ZIP 完整性、韌體副本與原專案逐檔 SHA256 比對。

測試涵蓋：分段／合併及延後 ACK、短寫入、最後錯誤、failsafe、timeout、取消、
斷線後建立新 session、重新連線時殘留回應、按住後取消點擊的釋放 guard、
子程序雙串流／退出碼與孫程序取消、依賴／編譯失敗後終止流程、bootloader
多候選選擇、Unicode／空白路徑、UF2 目的檔失敗的 OS 錯誤、重新列舉與握手
重試／歧義、排除既存 runtime 裝置的錯誤燒錄驗證、統計邊界、CSV、座標／delta、
軌跡終點與跳過過期節點，以及四頁 GUI 離線渲染。

Linux MSVC 交叉連結可能回報 LNK4099：下載的靜態 CRT 未包含 Microsoft 私有 PDB，
只影響相關 CRT debug symbols；建置成功。Windows 原生工具鏈仍須自行執行下列驗收。
Windows PowerShell 打包腳本及 GitHub Actions workflow 已提供，尚未在 Windows／遠端執行。

## 待 Windows 11／USB 實機驗收

- [ ] 在 Windows 11 x64 解壓 ZIP，正常啟動、顯示繁體中文，且不出現主控台。
- [ ] 執行 `scripts/build.ps1`，確認 Windows 原生 fmt、Clippy、測試、Release 與打包。
- [ ] 首次安裝 arduino-cli、Core、NeoPixel；檢查無 winget、安裝失敗、離線及取消。
- [ ] 分別編譯及燒錄 Leonardo、XIAO RP2040、Pico；檢查手動 reset／BOOTSEL。
- [ ] 兩台以上裝置／bootloader 候選時，確認選擇正確，不把別台 runtime 的 hello 當成驗證。
- [ ] 複製或上傳失敗、磁碟拔除、runtime 未重新列舉時，不顯示驗證成功。
- [ ] A 鍵一般／高精度／極限量測；無成功樣本、單輪、timeout、取消、實體鍵盤干擾。
- [ ] 極限量測子程序取消、強制終止與異常退出後，GUI 正常、優先權還原、按鍵已放開。
- [ ] 五鍵、雙擊、拖曳、垂直／水平滾輪及原始相對位移，兩板行為相同。
- [ ] 按住、拖曳或點擊時拔線／取消／關閉，確認釋放未知提示、重新連線 reset 與韌體 failsafe。
- [ ] 100%／150%／200% DPI、不同主顯示器解析度及多螢幕，實體座標與終點一致。
- [ ] USB endpoint 擁塞及排程遲到時，軌跡不趕送所有過期節點，最後點已確認。

既有韌體 README 的實機狀態敘述互有差異，本次保留原始檔案，沒有把它們視為
這個 Rust 主機版本已通過實機驗收的證據。
