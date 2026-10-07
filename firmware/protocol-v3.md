# Vendor HID protocol 3

本檔與 `shared/input-protocol/keys.json`、`vectors.json` 為跨 C++／Rust／JavaScript 的規範。韌體 3.1.0 僅提供 Vendor HID v3；已移除 v2 report、文字解析與回退。VID/PID 與 Vendor usage 不變，USB bcdDevice 為 0x0310。RP2040 使用唯一序號，Leonardo 固定為 HP-KB-0024。

## 能力與 framing

- OUT id 12、IN id 13、Feature id 14。所有 report 含 id 為 64 bytes。TinyUSB callback 的 id=0 形式含首 byte report id；非零形式僅有 63-byte payload。
- Feature 14 只讀、不取得 ownership。63-byte payload：`FMT3[4]`、protocol u8、board u8（1=Pico/其他，2=XIAO，3=Leonardo）、fw major/minor/patch 三個 u8、lease u16、reserved u16（新韌體寫零；主機忽略，以維持既有 v3 欄位位置）、poll_ms u8、capabilities u32、keyboard bitmap[28]、modifiers mask u8、consumer mask u8、device id[8]（Leonardo 是固定型號識別，不能作為唯一身分）、reserved[7]。capabilities 位元 0=NKRO、1=Consumer、2=relative mouse、3=absolute mouse（3.0.2 起固定為零）、4=concurrent v3 sessions（3.0.3 起）；目前 flags=23。
- OUT/IN 的 63-byte payload 標頭為 `version:u8, opcode:u8, length:u8, flags:u8, session:u32, sequence:u32`。資料最多 51 bytes。整數 little-endian，flags 與未使用尾端 byte 必須為零，不另加 CRC（USB 提供傳輸校驗）。
- `KeyId` 為 page u16 + usage u16；僅接受按鍵表中定義的 usage。
- 31-byte input snapshot：modifiers u8 + keyboard bitmap[28] + consumer u8 + buttons u8。bitmap bit `u` 代表 Keyboard usage `u`，E0–E7 放在 modifiers bit 0–7。未支援／保留 bit 必須為零。
- Consumer bits 0–6：Mute E2、Volume Up E9、Volume Down EA、Play/Pause CD、Next B5、Previous B6、Stop B7，page 均為 0C。滑鼠 bit 0–4：左、右、中、X1、X2。

## 命令

| opcode | 命令 | 資料 | 成功回覆 |
| --- | --- | --- | --- |
| 01 | OPEN | 空 | 81 |
| 02 | HEARTBEAT | 空 | 無 |
| 03 | STATUS | 空 | 83 |
| 04 | BARRIER | 空 | 84 |
| 05 | RELEASE_ALL | 空 | 85 |
| 06 | BOOTLOADER | 空 | 86 |
| 10 | KEY | page u16、usage u16、down u8（0/1） | 無 |
| 11 | SNAPSHOT | 31-byte input snapshot | 無 |
| 20 | MOUSE_MOVE | dx/dy i16，各 ±1024 | 無 |
| 21 | 舊 MOUSE_ABS，保留編號 | 不支援；invalid input 並釋放 | 故障事件 |
| 22 | MOUSE_BUTTONS | buttons u8 | 無 |
| 23 | WHEEL | vertical/horizontal i16，各 ±1024 | 無 |

OPEN 使用非零新 session，sequence=1；後續每個命令依序增加，包括心跳與查詢。耗盡 u32 前建立新連線，不在同一 session wrap。最多同時八個 session，序號各自計算。未知 session 的輸入丟棄；相同 ID 的 OPEN 或名額已滿回 busy，不覆蓋原連線。重複／缺號使所屬 session 失效，避免重播相對位移或按鍵。主機依 session、sequence、opcode 匹配回應，不能按回覆順序推測成功。

控制回應 opcode=命令|80，回應 payload 首 byte result：0=成功、1=framing、2=busy、3=invalid input、5=lease、6=HID send failed、7=USB disconnect、8=overflow/stale output、9=duplicate sequence、10=sequence gap。正常控制回應僅一個 result byte；STATUS 為固定 51-byte payload。

STATUS payload：result u8、received u32、completed u32、pending u8、lease_remaining u16、rx/tx/hid/failsafe 各 u16、input snapshot[31]。計數為 u16 wrap；主機比較工作前後計數，不能將該次工作中的變化視為成功。snapshot、received、completed、lease、pending 是查詢 session 自己的狀態；錯誤計數為裝置共用。completed 序號才表示該 session 前面的 USB 輸出已完成。

故障事件 opcode=7F，包含 result byte 與故障 session／最近接收序號。非法輸入、序號與租約故障只使對應 session 失效，取消該 session 待送工作並釋放其持有狀態，保留其他 session。USB／共用佇列故障使全部 session 失效，各自產生一個故障事件。TX overflow 另記計數；事件可能因傳輸問題遺失，控制 timeout 也須使主機 session 失效。

## 完成與租約

- 只有一個相對滑鼠 HID top-level collection，report id 3 同時輸出位移、五鍵與雙軸滾輪。沒有絕對 HID report id 4、座標快取或絕對輸出。目標位置／路徑在主機端計算並換算相對位移，韌體不處理螢幕座標。
- 一般輸入沒有 ACK；主機 I/O 寫入成功表示已提交，不能作為 USB 完成證明。需要可靠完成的階段送 BARRIER，等待此前所有輸出由 USB 完成確認（RP2040 為 TinyUSB callback，AVR 為 endpoint bank 已被主機 ACK）。
- RELEASE_ALL 取消該 session 先前未執行輸入，釋放其 Keyboard／Consumer／滑鼠按鈕持有狀態；保留其他 session 的狀態與排程，USB 完成後才回覆。session 保留，可再輸入或續租。BOOTLOADER 若有其他 session 則回 busy；成功時再等待回覆 USB 傳送完成後重啟。仍不保證目標應用程式處理時間。
- RP2040 output queue 32 項；Leonardo 2 項、RX 1 筆、TX 8 筆，最舊待送超過 50ms 或 queue overflow 即失效、清除及釋放。鍵盤與按鈕短按順序保留；相鄰待送同類移動可合併，但不跨按鈕、snapshot 或 barrier。相對／滾輪累加每軸上限 ±8192，超限亦失效。
- 同來源最後一筆排程可承載 BARRIER 完成序號，避免單次 down/up/barrier 額外佔用 AVR 佇列；完成仍等待所有先前 USB 輸出。
- v3 lease=2000ms，OPEN、有效輸入與 HEARTBEAT 續租；STATUS/BARRIER 不續租。主機每 500ms 傳送完整快照或心跳，即使當下沒有按鍵。逾時終止 session，不能靠晚到心跳重播舊狀態。
- 同時的 v3 session 各自持有 31-byte 狀態，USB 鍵盤／Consumer／按鈕輸出為位元聯集。相同按鍵須所有持有者都放開才放開；snapshot 只替換自己的狀態。不同 session 的相對位移與滾輪按接收順序作用於同一支游標，不能互相覆寫。BARRIER 回覆使用該 session 的序號，不能用另一 session 的完成序號代替。
- RP2040 非阻塞排程由 core 0 擁有，core 1 只處理 LED；Leonardo 在主迴圈排程。保留同一毫秒內的按下／放開；故障後不進行遲到事件補播。

## 同步與測試

在 FirmwareManageTool_Rust 根目錄：

```sh
python3 scripts/sync-input-protocol.py --peer ../EvanRemote --mscv-rust ../MSCV_RUST --mscv-cpp ../MSCV
python3 scripts/sync-input-protocol.py --check --peer ../EvanRemote --mscv-rust ../MSCV_RUST --mscv-cpp ../MSCV
cargo test --locked --workspace --no-default-features
bash scripts/test-rp2040.sh
bash scripts/test-leonardo.sh
```

同步腳本產生本地 Rust／C++／JavaScript 按鍵表、C++ wire vectors 與兩板的共享引擎 header 複本，並同步三個使用端。共用 firmware/common 是輸入狀態、排程、v3 解析與 USB report descriptor 的唯一來源。生成物需提交；只修改 Rust 一份或手改生成檔會被同步檢查拒絕。
