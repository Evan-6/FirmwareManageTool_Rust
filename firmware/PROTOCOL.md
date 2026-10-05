# FirmwareManageTool 鍵盤／滑鼠韌體通訊協定（protocol=2）

本檔是**線路協定的單一事實來源**，`leonardo_avr` 與 `rp2040` 兩個板子的實作都必須符合本規格。
若程式碼與本檔不符，以程式碼為準並回報差異（見 `CLAUDE.md` 硬規則 3）。

對照實作：`boards/leonardo_avr/Firmware.cpp`、`boards/rp2040/Firmware.cpp`。

## 傳輸通道

**指令通道就是 Vendor HID，出貨韌體不列舉 CDC 序列埠（不會出現 COM 埠）。**

| 通道 | 用途 | 備註 |
|------|------|------|
| Vendor HID | 唯一指令通道 | Usage Page `0xFF60`、Usage `0x61`。OUT report id `10`（host→device）、IN report id `11`（device→host）。單封包 payload = `EP_SIZE - 1` bytes。 |

協定使用純 ASCII 行，兩塊板子都**刻意關掉 CDC**，因此對外只露出 HID 鍵盤、HID 滑鼠 + Vendor HID：

- `leonardo_avr`：板子以 `-DCDC_DISABLED` 編譯（`boards.txt`），`Firmware.cpp` 的序列碼被 `#if defined(CDC_ENABLED)` 整段編掉；鍵盤、相對滑鼠與絕對滑鼠共用 Arduino HID 介面。
- `rp2040`：只使用 Vendor HID 指令通道；鍵盤與滑鼠命令均使用相同的
  ASCII 行協定；鍵盤、相對滑鼠與絕對滑鼠共用 TinyUSB HID 介面，不列舉 CDC/COM。

代價（兩板相同）：沒有序列 1200bps-touch 自動重置，燒錄需靠實體 reset / BOOTSEL。

Host 端使用 Vendor HID（比對 VID/PID `03F0:0024`、usage `0xFF60`），以 `hello` 交握。

## 行格式

- ASCII、以行為單位。行結束符可為 `LF`、`CRLF` 或 `CR`。
- 只接受可見 ASCII（`0x20`–`0x7E`）。遇到其他位元組 → 回 `err:invalid_byte` 並丟棄該行。
- 單行上限 127 字元，超過 → `err:line_too_long` 並丟棄該行。
- 一行開始接收後 1000ms 內沒收到結束符 → `err:line_timeout` 並丟棄。
- 指令大小寫不敏感、前後空白會被去除。

## 指令

| 指令 | 說明 |
|------|------|
| `hello` | 回報協定與參數。 |
| `ping` | 心跳；**會續租** failsafe 租約。 |
| `status` | 回報目前鍵盤／滑鼠按鍵狀態與計數；**不續租**。 |
| `d:<key>` / `+<key>` | 按下單一鍵。 |
| `u:<key>` / `-<key>` | 放開單一鍵。 |
| `sync:<k,k,...>` / `=<k,k,...>` | 快照：把整個鍵盤狀態設成清單內容（空清單＝全放開）。 |
| `reset` / `r:all` / `release:all` / `allup` | 立即放開全部鍵盤鍵與滑鼠按鍵。 |
| `mouse_move:x,y` | 傳送相對滑鼠移動；各軸每個命令範圍為 -1024–1024，韌體會拆成 HID report。 |
| `mouse_abs:x,y` | 傳送絕對滑鼠座標；`x`/`y` 範圍 0–65535，對應目標主顯示器整個畫面（見下方說明）。 |
| `mouse_button:n,down/up` | 傳送滑鼠按鍵；`n`：1=左、2=中、3=右、4/5=側鍵。按鍵狀態會同步反映在相對與絕對滑鼠 collection。 |
| `mouse_wheel:n` / `mouse_hwheel:n` | 傳送垂直／水平滾輪；每個命令範圍為 -1024–1024。 |
| `enter_bootloader` / `bootloader` | 全放開後進入下載模式（板子相依，見各板 README）。 |

## 回應

- 交握：`ok:hello,protocol=2,fw=2.0,lease_ms=<n>,max_nonmod=6,mouse=1`
- 心跳：`pong`
- 狀態：`ok:status,p=<按下總數>,n=<非修飾鍵數>,m=<修飾位元HEX>,k=<usage/usage/...>,lease=<剩餘ms>,fs=<failsafe次數>,rx=<接收錯誤數>,tx=<回應丟棄數>,hid=<鍵盤與滑鼠HID送出失敗數>,mb=<滑鼠按鍵位元HEX>`
- 滑鼠：`ok:mouse_move` / `ok:mouse_abs` / `ok:mouse_button` / `ok:mouse_wheel` / `ok:mouse_hwheel`
- 按鍵：`ok:down` / `ok:up` / `ok:already_down` / `ok:already_up`
- 快照：`ok:sync` / `ok:sync_unchanged`
- 全放開：`ok:release_all`
- 下載模式：`ok:enter_bootloader`
- 就緒橫幅：HID-only 建置不主動送出；主機使用 `hello` 交握。
- 錯誤：`err:hid_report_full`、`err:hid_send_failed`、`err:release_all_send_failed`、`err:bad_mouse`、`err:bad_sync`、`err:unknown_key`、`err:bad_command`、`err:invalid_byte`、`err:line_too_long`、`err:line_timeout`、`err:bad_report_id`
- 警告：`warn:failsafe_release_all`

## Failsafe 租約

- 只要有任何鍵盤鍵或滑鼠按鍵處於按下狀態，就啟動租約計時。
- 租約長度 **30000 ms**（`FailsafeReleaseMs`）。
- **會續租**的事件：`ping`、被接受的鍵盤、`sync` 或滑鼠指令。
- **不續租**：`status`。
- 逾時 → 自動全放開，並送出 `warn:failsafe_release_all`。
- Vendor HID 連線中斷時，主機應重新送 `reset`；韌體保留既有鍵盤／滑鼠 failsafe 租約。

> 主機端建議：每 250–1000 ms 送一次完整快照（`sync:` / `=`），比逐鍵增量更能自我修復。

## 絕對滑鼠定位

兩塊韌體都在共用的 HID 介面上提供獨立的 report id（`AbsMouseReportId = 4`），
宣告成 Generic Desktop / Mouse，但 Input flag 為 Absolute 而非 Relative（俗稱
"USB tablet" 手法，VirtualBox/VMware 的絕對滑鼠裝置也是這樣做）。收到
`mouse_abs:x,y` 時韌體直接組出一份 `{buttons, x, y}` 的絕對座標報告送出，
Windows 端的滑鼠 class driver 會把 0–65535 的邏輯範圍對應到主顯示器整個畫面。
因為這是獨立的 top-level collection，按鍵狀態不會自動與相對滑鼠報告
（`MouseReportId = 3`）同步，韌體在 `mouse_button` / `release_all` / failsafe
放開按鍵時都會重送一份帶著最新按鍵狀態的絕對座標報告，維持兩邊一致。

## 擬人化軌跡

擬人化在主機端 `FirmwareManageTool` 產生，線路上仍只使用標準
`mouse_abs:x,y` 節點，因此兩塊板子的韌體行為一致，也不會在 MCU protocol loop
中加入阻塞延遲。工具使用：

- minimum-jerk 時間曲線，模擬人手起步與停止時的加減速；
- 每次重新取樣的三次 Bézier 控制點與低振幅曲線偏移；
- 約 8ms 的節點節奏，最後一點強制精確收斂到目標；
- 持久 Vendor HID handle 並持續排空 ACK，避免逐點重開裝置或塞滿回應佇列。

互動工具支援擬人化絕對／相對移動、拖曳、單擊／雙擊、按住／放開、
垂直／水平滾輪與原始相對移動。

## 鍵盤上限

- 8 個修飾鍵（左右 Ctrl/Shift/Alt/GUI）＋ 最多 6 個非修飾鍵（標準 6KRO boot-style array）。

## 鍵名（token）

- 單字元：`a`–`z`、`0`–`9`
- 功能鍵：`f1`–`f24`
- 方向：`left` `right` `up` `down`
- 修飾鍵：`ctrl`/`control`/`left_ctrl`/`right_ctrl`、`shift`/`left_shift`/`right_shift`、`alt`/`left_alt`/`right_alt`、`win`/`gui`/`left_gui`/`right_gui`
- 編輯／導覽：`enter`/`return` `esc`/`escape` `backspace` `tab` `space` `caps_lock` `insert` `delete` `home` `end` `page_up`/`pageup` `page_down`/`pagedown` `print_screen` `scroll_lock` `pause` `menu`/`application`
- 符號：`minus` `equal` `left_bracket` `right_bracket` `backslash` `semicolon` `quote`/`apostrophe` `grave`/`backtick` `comma` `period`/`dot` `slash`
- 數字鍵盤：`num_lock` `kp_slash` `kp_asterisk` `kp_minus` `kp_plus` `kp_enter` `kp_0`–`kp_9` `kp_dot`

完整對照見 `Firmware.cpp` 內 `parseKeyToken()` 的 USB HID usage 對應表。
