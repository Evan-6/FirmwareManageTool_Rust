# 鍵盘／滑鼠協定索引

- [protocol-v2.md](protocol-v2.md)：ASCII 行協定。AVR 與舊 RP2040 使用；新版 RP2040 保留相容層。
- [protocol-v3.md](protocol-v3.md)：RP2040 二進位協定、NKRO、Consumer controls、session 與完成屏障。
- [按鍵表](../shared/input-protocol/keys.json) 與 [封包測試向量](../shared/input-protocol/vectors.json)：跨 C++／Rust／JavaScript 的版本化資料。

新主機端先查詢 Vendor HID Feature report 14；未提供此 Feature 時使用 v2。Feature 存在但內容非法時停止連線，不以 v2 掩蓋錯誤。

v2 OUT／IN report id 為 10／11，v3 為 12／13；Vendor usage 保留 0xFF60:0x61，report 皆為 64 bytes（含 id）。AVR 韌體不需變更。
