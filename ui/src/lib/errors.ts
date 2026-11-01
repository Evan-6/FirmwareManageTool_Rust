// Every Rust error reaches the webview as a flat string: commands return
// `format!("{e:#}")` and the worker forwards `AppEvent::Error(format!("{e:#}"))`,
// so an anyhow chain arrives as "中文提示：English SDK text: os error 2". That is
// what a maintainer needs and what a user cannot act on, so we split it once,
// here: an actionable Chinese headline plus a hint, with the untouched text kept
// as `detail` for bug reports.
import { IpcError } from "./ipc";

export interface UiError {
  /** One short line: what went wrong, in the user's language. */
  title: string;
  /** The next thing to try. Empty when we genuinely do not know. */
  hint: string;
  /** The raw chain, byte for byte — never reworded, never truncated. */
  detail: string;
  /** Command or event that produced it, so a report says where to look. */
  source: string;
}

interface Rule {
  test: RegExp;
  title: string;
  hint: string;
}

// Order matters: the first match wins, so device/protocol signatures sit above
// the generic "os error" catch-alls.
const RULES: Rule[] = [
  {
    test: /未選取裝置|沒有連線|請先連線/,
    title: "尚未連線到裝置",
    hint: "先到「裝置」頁按「重新掃描」，選一列後按「連線」。",
  },
  {
    test: /名額已滿|裝置忙碌|同名 session|Busy/i,
    title: "裝置已被其他程式佔用",
    hint: "關掉其他正在使用這塊板子的程式（probe、其他工具或另一份本程式），等 2 秒再連線。",
  },
  {
    test: /沒有回應|無法開啟|已斷線|unable to open/i,
    title: "裝置沒有回應或已斷線",
    hint: "先按「中斷」再重新連線；仍然不行就拔插 USB（避開 hub）。",
  },
  {
    test: /相對滑鼠未在期限內抵達目標|抵達目標/,
    title: "滑鼠沒有在時限內到達目標",
    hint: "確認游標在主螢幕內，並把時間 (ms) 拉長或移動距離縮小後重試。",
  },
  {
    test: /找不到 cargo|cargo.*not found|rustup.*not found/i,
    title: "找不到 Rust 工具鏈（cargo）",
    hint: "在「韌體」頁填 cargo.exe 的完整路徑（例如 C:/Users/<你>/.cargo/bin/cargo.exe），或先安裝 rustup。",
  },
  {
    test: /BOOTSEL|RPI-RP2|UF2 磁碟|bootloader/i,
    title: "找不到 BOOTLOADER 磁碟",
    hint: "按住 BOOTSEL 重新插上板子，等檔案總管出現 RPI-RP2（15 秒內）再試一次；也確認沒有其他視窗開著那個磁碟。",
  },
  {
    test: /hello|握手/i,
    title: "燒錄後握手失敗",
    hint: "板子已寫入但沒有回應 hello：拔插一次再按「連線」；連續失敗就重燒一次。",
  },
  {
    test: /租約|lease|逾時|timeout|期限/i,
    title: "裝置沒有回應（逾時）",
    hint: "確認用的是 USB 資料線、沒有經過 hub，然後按下「中斷」再重新連線。",
  },
  {
    test: /協議|framing|序號|回應長度|短寫入|OPEN v3/i,
    title: "裝置回應不符協議",
    hint: "板子上的韌體可能不是 3.2.0；到「韌體」頁重新建置並燒錄，再連線一次。",
  },
  {
    test: /HID|WriteFile|ReadFile|endpoint|pipe/i,
    title: "USB／HID 通訊失敗",
    hint: "換一個 USB 埠或換短線；也確認沒有其他鍵盤／巨集軟體獨佔這顆裝置。",
  },
  {
    test: /找不到裝置|No such device|裝置已移除|enumerate/i,
    title: "找不到裝置",
    hint: "插上板子後按「重新掃描」；仍然看不到就換一個 USB 埠。",
  },
  {
    test: /存取被拒|Access is denied|os error 5|權限|privilege|denied/i,
    title: "權限不足",
    hint: "用同一個使用者身分重新開啟程式；若是裝置被獨佔，關掉其他鍵盤／巨集工具。",
  },
  {
    test: /os error 2|os error 3|No such file|找不到指定的檔案/i,
    title: "找不到檔案或磁碟",
    hint: "路徑可能已失效：重新指定 cargo.exe、firmware 目錄或磁碟機代號。",
  },
  {
    test: /invalid type|missing field|serde|JSON|expected /i,
    title: "資料格式不符",
    hint: "設定或巨集檔可能損壞：關掉程式、刪除 %APPDATA%\\FirmwareManageTool 下的 settings.json（或該巨集檔）後重開。",
  },
  {
    test: /NaN|infinite|out of range|invalid value|不合法/i,
    title: "參數超出允許範圍",
    hint: "把數值改成範圍內再送出。",
  },
  {
    test: /不支援|Unsupported|僅支援/i,
    title: "此環境不支援這個操作",
    hint: "實體裝置操作只在 Windows 11 x64 上可用。",
  },
  {
    test: /取消|cancel/i,
    title: "操作已取消",
    hint: "",
  },
];

interface Fallback {
  title: string;
  hint: string;
}

const BY_SOURCE: Record<string, Fallback> = {
  scan: { title: "掃描不到裝置", hint: "確認板子插好、線是資料線，然後再按一次「重新掃描」。" },
  connect: { title: "連線失敗", hint: "按「重新掃描」後再選一次裝置；持續失敗就拔插 USB。" },
  disconnect: { title: "中斷連線時出錯", hint: "程式會在 2 秒租約到期後自動釋放按鍵，可以重新連線。" },
  request_status: { title: "讀不到裝置狀態", hint: "確認板子還插著；必要時重新連線。" },
  reset_input: { title: "放開按鍵失敗", hint: "立刻按緊急熱鍵 Ctrl+Alt+Shift+Esc，或重新連線讓租約到期。" },
  upload: { title: "燒錄失敗", hint: "看「輸出」面板的最後幾行找出中止的階段，修正後再燒一次。" },
  latency: { title: "延遲測試中斷", hint: "確認裝置仍連線；測試需要獨佔這塊板子。" },
  mouse: { title: "滑鼠動作沒有完成", hint: "確認目標在主螢幕內，並縮小距離或拉長時間。" },
  type_text: { title: "文字輸入中斷", hint: "確認裝置仍連線，且文字只含 US 配置可打的字元。" },
  chord: { title: "組合鍵沒有送出", hint: "確認 usages 是逗號分隔的 16 進位鍵碼，且裝置仍連線。" },
  trajectory_preview: { title: "無法預覽軌跡", hint: "把游標移到主螢幕內再調整參數（絕對座標只支援主螢幕）。" },
  macro_save: { title: "巨集沒有存檔", hint: "名稱只能用英數字、空白、- 和 _；確認 %APPDATA% 可寫入。" },
  macro_load: { title: "讀不到巨集", hint: "該巨集可能被刪除或改壞：按一次「播放」清單重新整理。" },
  macro_list: { title: "讀不到巨集清單", hint: "確認 %APPDATA%\\FirmwareManageTool\\macros 存在且可讀。" },
  settings_save: { title: "設定沒有存檔", hint: "確認 %APPDATA% 可寫入；這次的變更下次開啟會還原。" },
  settings_load: { title: "讀不到設定", hint: "會先用預設值；修好 settings.json 或刪掉後重開即可。" },
  choose_target: { title: "沒有選定 BOOTLOADER 磁碟", hint: "重新按「建置並燒錄」，並在跳出的視窗選正確的 RPI-RP2。" },
  worker: { title: "裝置回報錯誤", hint: "看下方技術細節；重複發生就重新連線。" },
  default: { title: "操作失敗", hint: "" },
};

/** The raw text, without the "command: " prefix `IpcError` adds for logs. */
export function errorText(error: unknown): string {
  if (error instanceof IpcError) return error.detail;
  if (error instanceof Error) return error.message;
  return String(error);
}

/** anyhow's `{:#}` joins its chain with "："/": " — keep the tail as detail. */
const CJK_HEAD = /^([^\s：:]{1,24}[\u4e00-\u9fff][^：:]{0,24})[：:]\s*(.+)$/;

export function describeError(error: unknown, source = ""): UiError {
  const detail = errorText(error).trim() || "（沒有錯誤訊息）";
  const rule = RULES.find((entry) => entry.test.test(detail));
  const fallback = BY_SOURCE[source] ?? BY_SOURCE.default;
  const head = CJK_HEAD.exec(detail);
  const title = rule?.title ?? head?.[1] ?? fallback.title;
  return {
    title,
    hint: rule?.hint ?? fallback.hint,
    detail,
    source,
  };
}
