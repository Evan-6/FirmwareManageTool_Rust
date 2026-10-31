# Vendor HID protocol 3

本檔、`shared/input-protocol/keys.json` 與 `vectors.json` 是唯一規範。韌體 3.2.0 只講 Vendor HID v3：
VID/PID `0x03F0:0x0024`，bcdDevice `0x0310`，USB 序號取自 RP2040 unique id（不能當密碼學身分）。
`shared/input-protocol/src/v3.rs` 的常數是 opcode 數字的唯一來源，三個最小客戶端由它產生。

## 能力與 framing

- OUT id 12、IN id 13、Feature id 14。所有 report 含 id 為 64 bytes。Feature 14 的 `Get_Report` 必須回
  64 bytes，`buf[0]` 是 report id（只回 63-byte payload 會被 Windows 判為短報告）。
- Feature 14 只讀、不取得 ownership。63-byte payload：

| 位移 | 型別 | 內容 |
| --- | --- | --- |
| 0..4 | ascii | `FMT3` |
| 4 | u8 | protocol 3 |
| 5 | u8 | board（XIAO 為 2） |
| 6..9 | u8 ×3 | fw major／minor／patch（3.2.0） |
| 9..11 | u16 | lease ms（2000） |
| 11..13 | u16 | 保留，寫零 |
| 13 | u8 | poll ms（1） |
| 14..18 | u32 | capabilities |
| 18..46 | 28 B | keyboard bitmap |
| 46 | u8 | modifiers mask（`0xFF`） |
| 47 | u8 | consumer mask（`0x7F`） |
| 48..56 | 8 B | RP2040 unique id |
| 56..60 | u32 | build（commit 前四 bytes） |
| 60..63 | 3 B | 保留，寫零 |

- capabilities：bit0 NKRO、bit1 Consumer、bit2 relative mouse、bit3 多 session 併發；目前值 `15`。
- OUT／IN 的 63-byte payload 標頭為 `version:u8, opcode:u8, length:u8, flags:u8, session:u32, sequence:u32`。
  資料最多 51 bytes。整數 little-endian，flags 與未使用尾端 byte 必須為零，不另加 CRC（USB 已校驗）。
- `KeyId` 為 page u16 ＋ usage u16；只接受 `keys.json` 定義的 usage。
- 31-byte input snapshot：modifiers u8 ＋ keyboard bitmap[28] ＋ consumer u8 ＋ buttons u8。bitmap bit `u`
  代表 Keyboard usage `u`，E0–E7 放在 modifiers bit 0–7。未支援／保留 bit 必須為零。
- Consumer bits 0–6：Mute E2、Volume Up E9、Volume Down EA、Play/Pause CD、Next B5、Previous B6、
  Stop B7，page 均為 0C。滑鼠 bit 0–4：左、右、中、X1、X2。

## 命令

| opcode | 命令 | 資料 | 成功回覆 |
| --- | --- | --- | --- |
| 01 | OPEN | 空 | 81 |
| 02 | CLOSE | 空 | 82 |
| 03 | HEARTBEAT | 空 | 無 |
| 04 | BARRIER | 空 | 84 |
| 05 | RELEASE_ALL | 空 | 85 |
| 06 | STATUS | 空 | 86（固定 51 bytes） |
| 07 | STATUS_EXT | 空 | 87（固定 23 bytes） |
| 08 | SESSION_LIST | 空 | 88（固定 48 bytes） |
| 09 | BOOTLOADER | 空 | 89 |
| 10 | KEY | page u16、usage u16、down u8（0/1） | 無 |
| 11 | SNAPSHOT | 31-byte input snapshot | 無 |
| 12 | MOUSE_MOVE | dx／dy i16，各 ±1024 | 無 |
| 13 | MOUSE_BUTTONS | buttons u8（bit 0–4） | 無 |
| 14 | WHEEL | vertical／horizontal i16，各 ±1024 | 無 |
| 7F | EVENT | result u8（故障碼） | 無 |

OPEN 使用非零新 session，sequence=1；後續每個命令依序增加，包括心跳與查詢。耗盡 u32 前建立新連線，
不在同一 session wrap。最多同時八個 session，序號各自計算。未知 session 的輸入丟棄；相同 ID 的 OPEN
或名額已滿回 busy，不覆蓋原連線。重複／缺號使所屬 session 失效，避免重播相對位移或按鍵。
主機依 session、sequence、opcode 匹配回應，不能按回覆順序推測成功。結束時送 CLOSE 立刻歸還名額。

控制回應 opcode＝命令 `| 0x80`，payload 首 byte 為 result：0=成功、1=framing、2=busy（同名或名額已滿）、
3=invalid input、5=lease、6=HID send failed、7=USB disconnect、8=overflow／stale output、
9=duplicate sequence、10=sequence gap。STATUS、STATUS_EXT、SESSION_LIST 與 EVENT 的 payload 不是單純
result byte：前兩者固定長度且含自己的 result／診斷欄位，SESSION_LIST 沒有 result byte，EVENT 為 1 byte。

## STATUS（06，51 bytes）

result u8、received u32、completed u32、pending u8、lease_remaining u16、rx／tx／hid／failsafe 各 u16、
input snapshot[31]。計數為 u16 wrap。snapshot、received、completed、lease、pending 是查詢 session 自己的
狀態；錯誤計數為裝置共用。其他 session 的租約到期或協議錯誤也會增加 failsafe，不能僅依此判定本工作失敗。
本 session 故障以匹配自身 ID 的 EVENT／控制 timeout 判定。completed 序號才表示該 session 前面的 USB 輸出
已完成。

STATUS 不續租；OPEN、有效輸入、HEARTBEAT、STATUS_EXT、SESSION_LIST 會續租。

## STATUS_EXT（07，23 bytes）

| 位移 | 型別 | 內容 |
| --- | --- | --- |
| 0 | u8 | result（0 正常） |
| 1..5 | u32 | 開機至今 uptime ms |
| 5..7 | u16 | 上次讀取以來最大 1 ms tick 超時（µs），讀後歸零 |
| 7 | u8 | output 佇列待送工作數 |
| 8 | u8 | USB OUT 佇列深度 |
| 9 | u8 | TX 待送回應數 |
| 10 | u8 | 目前 session 數 |
| 11 | u8 | 最近故障碼（0＝開機後沒有故障） |
| 12..16 | u32 | 最近故障距今 ms（`0xFFFF_FFFF`＝沒有） |
| 16 | u8 | reset 原因：0 未知、1 軟體／forced、2 watchdog、3 冷開機 |
| 17..19 | u16 | 開機次數（watchdog scratch 保存，斷電歸零） |
| 19..23 | u32 | build：與 Feature 14 相同的 commit 前四 bytes |

## 連線擁有者（08，48 bytes）

八個 slot 各 6 bytes，空 slot 全零，順序固定（slot 0–7），所以主機不必解析可變長度清單。
payload 沒有 result byte，第一筆的 id 就在 byte 0；這命令與其他查詢一樣會續租。

| 位移 | 型別 | 內容 |
| --- | --- | --- |
| 0..2 | u16 | session id 的低 16 bit |
| 2..4 | u16 | 剩餘租約 ms |
| 4 | u8 | 該 session 目前按住的鍵數 |
| 5 | u8 | flags：bit0 有尚未送出的命令、bit1 已 RELEASE_ALL、bit2 有 BOOTSEL 請求 |

`STATUS_EXT` 只說「有幾個 session」，答不出「誰按著 W」；要除錯多程式同時連線就看這份清單。

## 完成與租約

- 只有一個相對滑鼠 HID top-level collection，report id 3 同時輸出位移、五鍵與雙軸滾輪；韌體不處理螢幕
  座標，目標位置／路徑由主機換算成相對位移。
- 一般輸入沒有 ACK；主機 I/O 寫入成功只表示已提交。需要可靠完成的階段送 BARRIER，等此前所有輸出由 USB
  完成（embassy-usb 的 IN transfer 完成事件）才回覆。
- RELEASE_ALL 取消該 session 先前未執行的輸入、釋放其 Keyboard／Consumer／滑鼠按鈕持有狀態，保留其他
  session 的狀態與排程，USB 完成後才回覆；session 保留。BOOTLOADER 若有其他 session 回 busy。
- output／RX／TX 佇列各 32 項；最舊待送超過 50 ms 或溢位即失效、清除並釋放。按鍵短按順序保留；相鄰
  待送同類移動可合併，但不跨按鈕、snapshot 或 barrier。相對／滾輪累加每軸上限 ±8192，超限亦失效。
- 租約 2000 ms；STATUS／BARRIER 不續租，其餘命令續租。主機每 500 ms 送心跳或完整快照。逾時終止
  session，晚到的心跳不能重播舊狀態。正常結束必須送 CLOSE，否則 owner 要等 2 秒才釋放並計入 failsafe。
- 同時的 session 各自持有 31-byte 狀態，USB 鍵盤／Consumer／按鈕輸出為位元聯集；相同按鍵須所有持有者
  都放開才放開，snapshot 只替換自己的狀態。位移與滾輪依接收順序作用於同一支游標。
- 主機 suspend／USB reset 走同一條故障路徑（碼 7）：所有 session 失效、按住的鍵放開。watchdog（2 s）
  處理主迴圈卡死。

## 最小操作端（Rust／C++／Python）

`clients/` 是自動產生的薄客戶端，只做「驗身分 → OPEN → 送命令 → 心跳 → CLOSE」，沒有掃描、GUI、燒錄
或設定管理。三份實作共用同一組 opcode（由 `shared/input-protocol/src/v3.rs` 產生），各自只要求呼叫端
提供三個傳輸方法：

| 語言 | 檔案 | 傳輸介面 |
| --- | --- | --- |
| Rust | `clients/rust/hid_v3.rs` | `feature() / write(&[u8;64]) / read() -> Option<[u8;64]>` |
| C++ | `clients/cpp/hid_v3.hpp` | `feature / write / read(timeout_ms)`（`hid_v3::Transport`） |
| Python | `clients/python/hid_v3.py` | `feature() / write(bytes) / read(timeout_ms)` |

API 一致：`verify()`、`open()`、`close()`、`heartbeat()`、`maybe_heartbeat()`、`status()`、`status_ext()`、
`sessions()`、`barrier()`、`release_all()`、`bootloader()`、`key()`、`snapshot()`、`mouse_move()`、
`buttons()`、`wheel()`。模板在 `scripts/clients/`，不要手改產物。

## 同步與測試

在 FirmwareManageTool_Rust 根目錄：

```sh
python scripts/sync-input-protocol.py            # 產生按鍵表與三個客戶端
python scripts/sync-input-protocol.py --check    # 產物必須是最新的
python scripts/check-clients.py                  # Python 行為測試、Rust 編譯、C++ 語法檢查
cargo test --locked --workspace --no-default-features
cargo test --manifest-path firmware/app/Cargo.toml
bash scripts/build-rust-firmware.sh
```

生成物需提交；`--check` 會拒絕漂移。USB report descriptor 凍結在
`firmware/app/src/usb_descriptors.rs`，由 `firmware/app/tests/descriptors.rs` 以雜湊釘住。
