# Leonardo AVR 韌體 3.1.0

Arduino Leonardo／Micro／ATmega32U4 Pro Micro 使用內附 AVR core 1.8.8，FQBN `goosedevil:avr:keyboard`。
僅提供 Vendor HID v3，與 RP2040 共用 session／輸入排程／report descriptor。
鍵盤 NKRO（report 2）、Consumer 七鍵（report 5）、五鍵相對滑鼠與雙軸滾輪（report 3）；
Vendor OUT 12／IN 13／Feature 14。沒有 absolute HID、v2 文字指令或 runtime CDC／COM。
USB VID/PID 03F0:0024，固定序號 HP-KB-0024，Feature board=3、fw=3.1.0、flags=23，bcdDevice=0x0310。

最多 8 個 session，各自保存輸入、序號及 2 秒租約；主機每 500ms 續租。
按鍵與按鈕取所有來源聯集，RELEASE_ALL／租約／個別協議故障只清理自己的來源。
受 2560 bytes SRAM 限制，輸出佇列 2 筆、RX 1 筆、TX 8 筆；滿載或輸出過期回 fault 8 並清理全部 session。
短按 down/up/barrier 可共用最後一筆的完成序號；大量連續輸入需等待 BARRIER，故障後重新 OPEN。

共享引擎修改 `firmware/common/`，執行同步腳本更新 `modules/V3*.h`，不要手改生成物。
`modules/Platform.h` 提供 AVR 原子變數與有界佇列；`UsbTransport.h` 註冊兩個 PluggableUSB 介面。
內附 USB core 的 HidPackets.h 非阻塞提交單一 packet，不額外送 ZLP；bank 被主機 ACK 後才通知排程完成。
USB reset／suspend 會使舊 session 失效。Bootloader 先等輸入釋放與回覆 USB 完成，再用 watchdog／MAGIC_KEY 進 Caterina。

```powershell
firmware\boards\leonardo_avr\upload.ps1
firmware\boards\leonardo_avr\upload.ps1 -Port COM4
```

arduino-cli 的 user dir 指向本 sketch，偵測內附 `hardware/goosedevil/avr`。
正常升級以 Vendor HID v3 OPEN／BOOTLOADER；首次從舊韌體升級請按實體 reset，
再指定 Caterina bootloader COM。燒錄前關閉其他控制程式並等 2 秒。
多台 Leonardo 的序號相同，無法靠序號證明身分；自動燒錄遇到歧義會要求選擇。

Linux 已完成編譯與實際韌體配合假 USB 的測試；Windows／實體 AVR USB 仍待驗收。
線路規格見 [protocol-v3.md](../../protocol-v3.md)，對照 [RP2040](../rp2040/README.md)。
