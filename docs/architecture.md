# 架構與操作保證

GUI 只保存顯示狀態，透過型別化 AppCommand／AppEvent 與背景工作溝通。
同時只執行一項裝置工作，HID handle 由工作端獨占。cancel 使用原子旗標，
bootloader 選擇使用獨立 channel，長工作不阻塞 GUI。

HidTransport／ProcessRunner 可替換為假裝置／假子程序。裝置傳輸在 `src/hid/transport.rs`，
v3 I/O 在 `src/hid/v3.rs`，共用編解碼與
session 序號在 `shared/input-protocol`。業務端使用型別化輸入，不自行組合二進位封包。

Session 先查詢 Feature 14。v3 使用 64-byte 完整封包、session／sequence，
一般輸入無 ACK；OPEN／STATUS／BARRIER／RELEASE_ALL／BOOTLOADER 有對應回覆。
屏障等待先前 USB report 傳送完成，不代表 Windows 應用程式已處理。
所有短寫入、錯誤、timeout 或 failsafe 都使 session 失效，重連丟棄舊回應並建立自己的空輸入狀態。
畫面目標位置與擬人化曲線在主機計算，`input-protocol::pointer::RelativeTracker` 共用
Windows 游標回授的相對位移控制器；USB 只送 MOUSE_MOVE，韌體 3.1.0 只有相對滑鼠。
末端定位最多在路徑結束後繼續修正 300ms；無法到達回錯誤，拖曳以 ReleaseGuard 清理。
滑鼠工作最後等待屏障／ACK、查詢 status，並檢查 TX／HID 計數未增加。
failsafe 計數是全裝置共用，其他 session 的租約到期也會增加，不能據此判定本工作失敗；
本 session 或全裝置故障以帶 session 的 EVENT 判定，接收後立即使連線失效。

| 階段 | 期限 |
| --- | --- |
| HID 寫入 | hidapi Windows C 後端 1 秒；錯誤後不再重用 handle |
| HID ACK | 自發送起 1 秒；收取以 10ms 切片檢查取消 |
| 按下／放開 A | 每階段 1 秒 |
| 安裝依賴 | 每個子程序 15 分鐘 |
| 編譯 | 10 分鐘 |
| AVR 上傳 | 2 分鐘 |
| AVR bootloader | 10 秒 |
| RP2040 bootloader | 15 秒 |
| runtime 握手驗證 | 15 秒的偵測窗口，加上正在進行的有期限 HID 握手 |
| 極限子程序取消 | 2 秒後強制終止，再嘗試重新連線 reset |
| 關閉 GUI | 最多等待背景清理 5 秒 |

Windows 子程序在暫停狀態建立，先加入具有 KILL_ON_JOB_CLOSE 的 Job Object，
再恢復執行，避免子程序先啟動孫程序而逃過清理。Linux 測試使用 process group。
stdout／stderr 串流顯示，各自保存最多 10MiB；GUI 保存最近 1000 行日誌。

極限測試透過相同 exe 的內部 `--latency-worker` 入口執行，以 JSON 行傳遞要求、
逐輪結果和報告，stdin `cancel` 或 EOF 觸發取消。REALTIME／TIME_CRITICAL 僅套用
在量測子程序送出按下指令時，送出後先以 RAII 還原程序優先權，再觀察 Windows 輸入。
觀察執行緒與高精度模式相同，使用 ABOVE_NORMAL、有期限自旋及定期 yield；一般模式使用短 sleep 輪詢。
主 GUI 從不切換 REALTIME。持續以 REALTIME 忙等會阻礙 Windows 鍵盤輸入執行緒，
因此不能將這個優先權延伸至 GetAsyncKeyState 等待區間（[Microsoft 排程說明](https://learn.microsoft.com/zh-tw/windows/win32/procthread/scheduling-priorities)）。

QPC 計時包含 Rust 主機送出命令的 session 處理、USB 寫入、韌體、Windows driver
及 GetAsyncKeyState 輪詢，極限模式也包含送出後還原優先權的耗時，並非 MCU 單獨處理時間。只有完整按下／放開及
協定檢查成功的輪次納入統計；錯誤即停止，不繼續累積被污染的樣本。
成功率以實際執行輪數為分母，另列要求及未執行輪數；提前停止後未執行的輪次不算失敗。
每輪錯誤標明送出、Windows 觀察、USB 屏障或裝置狀態檢查階段。
P95／P99 使用 nearest-rank；標準差使用 n−1，單樣本不計。

燒錄先編譯再進 bootloader。自動重啟已選取 runtime 時，排除重啟前已存在的
bootloader，避免自動選到另一台板子。GUI 直接呼叫 arduino-cli，不呼叫舊 PowerShell
上傳腳本，也不依賴 runtime CDC touch。AVR 以 upload --verify 傳輸；RP2040
複製確切 sketch 的 UF2，任何 OS 複製錯誤保留為失敗。兩者都再驗證 runtime hello。
驗證排除傳輸前已存在的 runtime 裝置，避免別台板子的 hello 被當成此次燒錄成功。
AVR／RP2040 均以 v3 能力查詢與 OPEN 驗證，檢查 board id（Leonardo=3、Pico=1、XIAO=2）。
RP2040 使用 MCU unique ID 作為 USB 序號與能力識別，未提供映像 hash 或密碼學身分證明。重複序號／多個重新列舉候選視為歧義，不能驗證成功。

擬人化軌跡以主螢幕實體像素計算 minimum-jerk／Bézier，保留低振幅偏移、
重複像素省略及精確最後目標。排程遲到跳過過期節點，不補送所有舊點。
原始相對移動及滾輪以 ±1024 拆指令，8-bit USB report 拆分由韌體排程執行。
點擊／拖曳有釋放 guard，閒置時也每 500ms 續租。
斷線回報釋放狀態未知，重新連線先清空。兩種板子租約均為 2 秒。
RP2040 的 32 項輸出佇列保留按鍵／按鈕順序；滿載或等待超過 50ms 時停止 session 並優先釋放。

韌體 3.1.0 最多八個 v3 session 可同時控制，各自保存序號、租約與輸入狀態；
USB 按鍵／Consumer／滑鼠按鈕取聯集，位移與滾輪依接收順序輸出。
排程工作記錄 session 與各來源快照，個別 RELEASE_ALL／租約／協定故障取消自己的待送工作，
重建其他來源的歷史狀態並保留其短按、相對位移與屏障完成通知。
STATUS 的輸入／序號／租約／待送數屬於查詢者，錯誤計數共用。
共用 USB／佇列故障才釋放全部來源。燒錄前須關閉其他 v3 程式並等 2 秒回收 session。

Leonardo 共用 firmware/common 引擎，輸出／RX／TX 佇列深度為 2／1／8；RP2040 為 32／32／32。AVR USB packet API 不阻塞或發送尾端 ZLP，完成依主機 ACK 的 endpoint bank；USB reset 不允許舊 boot 回覆被誤當完成。Leonardo 固定序號 HP-KB-0024，不可視為唯一身分。

RP2040 已使用 `firmware/boards/rp2040/src/` 的獨立 `.cpp`：InputEngine 持有 session／scheduler／TX，FirmwareRuntime 組裝單一正式實例，平台層持有兩個 USB HID、RX、bootloader 與 LED。純引擎以主迴圈傳入的 uint32 時間運作，不直接呼叫硬體。callback 只發布同步通知，USB reset 透過 TinyUSB event hook 使舊傳送失效；core 1 只讀發布的 LED 狀態。測試正常編譯及連結這些來源，不再 include Firmware.cpp。重構規格見 [rp2040-refactor-spec.md](rp2040-refactor-spec.md)。

`firmware/common/` 與 Leonardo 的生成引擎維持原樣，RP2040 不再由同步腳本複製引擎標頭；按鍵表與 wire vectors 仍共用。過渡期間修改協議或排程需同時確認兩板。
