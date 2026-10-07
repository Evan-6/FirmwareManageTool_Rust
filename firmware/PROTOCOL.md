# 鍵鼠協議

只支援 [Vendor HID v3](protocol-v3.md)。Leonardo／Pico／XIAO 韌體 3.1.0 共用此協議，最多 8 個 session。

連線必須先讀取 Feature 14 並驗證 FMT3，再以獨立非零 session OPEN。
不提供 Feature 14、格式不正確或舊 v2 設備顯示不支援，不使用 serial 回退。
Vendor usage 為 0xFF60:0x61，OUT 12／IN 13／Feature 14 都是 64 bytes（含 id）。

RELEASE_ALL／停止／程式退出只釋放自己的 session。USB／共用佇列故障才釋放全部 session。
正常韌體沒有 CDC／COM；Leonardo 的 COM 僅用於 Caterina bootloader。
舊韌體第一次升級請實體 reset（AVR）或 BOOTSEL（RP2040），新版燒錄入口不再傳送 v2 指令。
