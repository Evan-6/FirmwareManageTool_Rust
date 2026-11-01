import { useEffect, useMemo, useRef, useState } from "react";

import { ErrorLine } from "../components/ErrorBanner";
import { Panel } from "../components/Panel";
import { describeError } from "../lib/errors";
import type { UiError } from "../lib/errors";
import { call } from "../lib/ipc";
import { intValue } from "../lib/num";
import { loadPref, savePref } from "../lib/prefs";
import type {
  ChordConfig,
  Ease,
  HumanProfile,
  MacroStep,
  MouseAction,
  MouseRequest,
  TrajectoryPreview,
  TypeTextConfig,
} from "../lib/types";
import { DEFAULT_PROFILE } from "../lib/types";
import { useDeferredSave } from "../lib/useDeferredSave";
import { reportError, useStore } from "../store";

const EASES: { value: Ease; label: string }[] = [
  { value: "Jerk", label: "jerk（人體加減速）" },
  { value: "EaseInOut", label: "ease-in-out" },
  { value: "Linear", label: "等速" },
];

function Slider(props: {
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  suffix?: string;
  onChange: (value: number) => void;
}) {
  return (
    <label className="field">
      <span>
        {props.label}：{props.value}
        {props.suffix ?? ""}
      </span>
      <input
        type="range"
        min={props.min}
        max={props.max}
        step={props.step}
        value={props.value}
        onChange={(event) => props.onChange(Number(event.target.value))}
      />
    </label>
  );
}

function TrajectoryView({ preview }: { preview: TrajectoryPreview | null }) {
  if (!preview || preview.nodes.length === 0) {
    return <p className="muted">調整參數後會畫出實際送出節點（需要游標在主顯示器內）。</p>;
  }
  const { width, height, startX, startY, nodes } = preview;
  const last = nodes[nodes.length - 1];
  const points = nodes.map((node) => `${node.x},${node.y}`).join(" ");
  return (
    <svg className="trajectory" viewBox={`0 0 ${width} ${height}`} preserveAspectRatio="xMidYMid meet">
      <rect x={0} y={0} width={width} height={height} fill="#12151c" stroke="#2d3240" />
      <polyline points={points} fill="none" stroke="#f0aa28" strokeWidth={Math.max(1, width / 900)} />
      {nodes.map((node, index) => (
        <circle key={index} cx={node.x} cy={node.y} r={Math.max(1, width / 700)} fill="#f0aa28" opacity={0.5} />
      ))}
      <circle cx={startX} cy={startY} r={Math.max(3, width / 300)} fill="#3fbf7f" />
      <circle cx={last.x} cy={last.y} r={Math.max(3, width / 300)} fill="#e05a5a" />
    </svg>
  );
}

export function MousePage() {
  const { state, dispatch, saveSettings } = useStore();
  const [profile, setProfile] = useState<HumanProfile>(state.settings?.mouse ?? DEFAULT_PROFILE);
  const [x, setX] = useState(120);
  const [y, setY] = useState(-80);
  const [duration, setDuration] = useState(260);
  const [button, setButton] = useState(1);
  const [count, setCount] = useState(1);
  const [wheel, setWheel] = useState(120);
  const [preview, setPreview] = useState<TrajectoryPreview | null>(null);
  const [previewError, setPreviewError] = useState<UiError | null>(null);
  const [text, setText] = useState("hello world");
  const [typing, setTyping] = useState<TypeTextConfig>({ text: "hello world", intervalMs: 45, holdMs: 18, jitterMs: 30 });
  const [chordKeys, setChordKeys] = useState("E0,E1,06");
  const [chordHold, setChordHold] = useState(60);
  const [history, setHistory] = useState<string[]>([]);
  const [recording, setRecording] = useState<MacroStep[] | null>(null);
  // `Settings` has no field for the macro being edited, so this one lives in
  // localStorage instead of the settings file.
  const [macroName, setMacroName] = useState(() => loadPref("macroName", "macro-1"));
  const [macros, setMacros] = useState<string[]>([]);
  const startedAt = useRef(Date.now());

  useEffect(() => {
    const saved = state.settings?.mouse;
    if (!saved) return;
    setProfile((current) => (JSON.stringify(current) === JSON.stringify(saved) ? current : saved));
  }, [state.settings]);

  // Sliders are cheap to move and easy to forget: persist the shape once the
  // drag settles, so a restart keeps the tuned profile.
  useDeferredSave(state.settings !== null, [profile], () => {
    void saveSettings({ mouse: profile }).catch((error) => reportError(dispatch, error, "settings_save"));
  });

  useEffect(() => {
    savePref("macroName", macroName);
  }, [macroName]);

  useEffect(() => {
    void call("macro_list", {})
      .then(setMacros)
      .catch(() => setMacros([]));
  }, []);

  // Preview the exact nodes the worker will send.
  useEffect(() => {
    const handle = window.setTimeout(() => {
      void call("trajectory_preview", {
        targetX: 0,
        targetY: 0,
        durationMs: duration,
        profile,
      })
        .then((result) => {
          // The command previews relative to the live cursor; shift so the path
          // is visible in a card instead of landing wherever the pointer is.
          const shifted: TrajectoryPreview = {
            ...result,
            startX: Math.round(result.width / 2),
            startY: Math.round(result.height / 2),
            nodes: result.nodes.map((node) => ({
              atMs: node.atMs,
              x: Math.round(result.width / 2 + (node.x - result.startX)),
              y: Math.round(result.height / 2 + (node.y - result.startY)),
            })),
          };
          setPreview(shifted);
          // A recovered preview has to clear the old failure, otherwise the red
          // line stayed under a perfectly good trajectory forever.
          setPreviewError(null);
        })
        .catch((error) => {
          setPreview(null);
          setPreviewError(describeError(error, "trajectory_preview"));
        });
    }, 180);
    return () => window.clearTimeout(handle);
  }, [profile, duration]);

  const path = useMemo(() => preview?.nodes.length ?? 0, [preview]);

  const record = (step: MacroStep) => {
    if (recording) setRecording([...recording, step]);
  };

  const send = async (action: MouseAction, label: string) => {
    const request: MouseRequest = { action, profile };
    try {
      await call("mouse", { request });
      setHistory((current) => [`${label}${preview ? `（${path} 節點）` : ""}`, ...current].slice(0, 20));
      record({ kind: "mouse", at: Date.now() - startedAt.current, request });
    } catch (error) {
      reportError(dispatch, error, "mouse");
    }
  };

  const saveProfile = async () => {
    try {
      await saveSettings({ mouse: profile });
      dispatch({ type: "event", event: { kind: "log", line: "已儲存擬人化參數" } });
    } catch (error) {
      reportError(dispatch, error, "settings_save");
    }
  };

  const runMacro = async (steps: MacroStep[], speed: number) => {
    let previous = 0;
    try {
      for (const step of steps) {
        const wait = Math.max(0, (step.at - previous) / speed);
        previous = step.at;
        await new Promise((resolve) => window.setTimeout(resolve, Math.min(wait, 2000)));
        if (step.kind === "mouse") await call("mouse", { request: step.request });
        if (step.kind === "text") await call("type_text", { config: step.config });
        if (step.kind === "chord") await call("chord", { config: step.config });
      }
    } catch (error) {
      reportError(dispatch, error, "macro_load");
    }
  };

  const typeText = () => {
    const config = { ...typing, text };
    void call("type_text", { config }).catch((error) => reportError(dispatch, error, "type_text"));
    record({ kind: "text", at: Date.now() - startedAt.current, config });
  };

  const sendChord = () => {
    const usages = chordKeys
      .split(",")
      .map((part) => Number.parseInt(part.trim(), 16))
      .filter((value) => Number.isFinite(value) && value > 0);
    if (usages.length === 0) {
      reportError(dispatch, new Error("組合鍵沒有送出：至少要有一個 16 進位鍵碼，例如 E0,E1,06"), "chord");
      return;
    }
    const config: ChordConfig = { usages, holdMs: chordHold };
    void call("chord", { config }).catch((error) => reportError(dispatch, error, "chord"));
    record({ kind: "chord", at: Date.now() - startedAt.current, config });
  };

  return (
    <div className="page">
      <Panel
        title="擬人化參數"
        actions={
          <>
            <button onClick={() => setProfile(DEFAULT_PROFILE)}>重設</button>
            <button onClick={() => void saveProfile()}>儲存為預設</button>
          </>
        }
      >
        <div className="grid two">
          <Slider label="曲率 curve" value={profile.curve} min={0} max={2} step={0.1} onChange={(curve) => setProfile({ ...profile, curve })} />
          <Slider label="抖動 jitter" value={profile.jitterPx} min={0} max={8} step={0.1} suffix=" px" onChange={(jitterPx) => setProfile({ ...profile, jitterPx })} />
          <Slider label="過衝 overshoot" value={profile.overshootPct} min={0} max={20} step={1} suffix=" %" onChange={(overshootPct) => setProfile({ ...profile, overshootPct })} />
          <Slider label="收尾停頓" value={profile.settleMs} min={0} max={300} step={10} suffix=" ms" onChange={(settleMs) => setProfile({ ...profile, settleMs })} />
          <Slider label="最小步距" value={profile.minStepPx} min={1} max={20} step={1} suffix=" px" onChange={(minStepPx) => setProfile({ ...profile, minStepPx })} />
          <label className="field">
            <span>時間曲線</span>
            <select value={profile.ease} onChange={(event) => setProfile({ ...profile, ease: event.target.value as Ease })}>
              {EASES.map((ease) => (
                <option key={ease.value} value={ease.value}>
                  {ease.label}
                </option>
              ))}
            </select>
          </label>
        </div>
        <TrajectoryView preview={preview} />
        {previewError && <ErrorLine error={previewError} />}
        <p className="muted">
          預覽與實際送出的節點是同一支 `trajectory_with`。目前 {path} 個節點
          {preview ? `、約 ${preview.nodes[preview.nodes.length - 1]?.atMs ?? 0} ms` : ""}。
        </p>
      </Panel>

      <Panel title="相對移動">
        <div className="grid four">
          <label className="field">
            <span>dx</span>
            <input type="number" value={x} onChange={(event) => setX(intValue(event.target.value, x))} />
          </label>
          <label className="field">
            <span>dy</span>
            <input type="number" value={y} onChange={(event) => setY(intValue(event.target.value, y))} />
          </label>
          <label className="field">
            <span>時間 (ms)</span>
            <input type="number" value={duration} onChange={(event) => setDuration(intValue(event.target.value, duration, 0, 60000))} />
          </label>
          <div className="field">
            <span>&nbsp;</span>
            <button disabled={!state.connected} onClick={() => void send({ MoveBy: { x, y, duration_ms: duration } }, `移動 ${x},${y}`)}>
              移動
            </button>
          </div>
        </div>
        <div className="row">
          <button disabled={!state.connected} onClick={() => void send({ Raw: { x, y } }, `原始 ${x},${y}`)}>
            直接送位移
          </button>
          <button disabled={!state.connected} onClick={() => void send({ MoveTo: { x, y, duration_ms: duration } }, `絕對 ${x},${y}`)}>
            移到絕對座標
          </button>
        </div>
      </Panel>

      <Panel title="點擊、拖曳、滾輪">
        <div className="grid four">
          <label className="field">
            <span>按鍵 1–5</span>
            <input type="number" min={1} max={5} value={button} onChange={(event) => setButton(intValue(event.target.value, button, 1, 5))} />
          </label>
          <label className="field">
            <span>次數</span>
            <input type="number" min={1} max={2} value={count} onChange={(event) => setCount(intValue(event.target.value, count, 1, 2))} />
          </label>
          <label className="field">
            <span>滾輪</span>
            <input type="number" value={wheel} onChange={(event) => setWheel(intValue(event.target.value, wheel, -12000, 12000))} />
          </label>
          <div className="field">
            <span>&nbsp;</span>
            <button disabled={!state.connected} onClick={() => void send({ Click: { button, count } }, `點擊 ${button}×${count}`)}>
              點擊
            </button>
          </div>
        </div>
        <div className="row">
          <button disabled={!state.connected} onClick={() => void send({ Drag: { button, x, y, duration_ms: duration } }, `拖曳 ${button}`)}>
            拖曳
          </button>
          <button disabled={!state.connected} onClick={() => void send({ Wheel: { vertical: wheel, horizontal: 0 } }, "垂直滾輪")}>
            垂直滾輪
          </button>
          <button disabled={!state.connected} onClick={() => void send({ Wheel: { vertical: 0, horizontal: wheel } }, "水平滾輪")}>
            水平滾輪
          </button>
          <button className="danger" disabled={!state.connected} onClick={() => void call("reset_input", {}).catch((error) => reportError(dispatch, error, "reset_input"))}>
            全部放開
          </button>
        </div>
      </Panel>

      <Panel title="文字輸入">
        <div className="grid two">
          <label className="field">
            <span>文字（US 配置，換行＝Enter）</span>
            <textarea value={text} rows={3} onChange={(event) => { setText(event.target.value); setTyping({ ...typing, text: event.target.value }); }} />
          </label>
          <div className="grid two">
            <Slider label="字間隔" value={typing.intervalMs} min={0} max={300} step={5} suffix=" ms" onChange={(intervalMs) => setTyping({ ...typing, intervalMs })} />
            <Slider label="按鍵持續" value={typing.holdMs} min={8} max={80} step={2} suffix=" ms" onChange={(holdMs) => setTyping({ ...typing, holdMs })} />
            <Slider label="抖動" value={typing.jitterMs} min={0} max={200} step={5} suffix=" ms" onChange={(jitterMs) => setTyping({ ...typing, jitterMs })} />
            <div className="field">
              <span>&nbsp;</span>
              <button disabled={!state.connected} onClick={typeText}>
                開始輸入
              </button>
            </div>
          </div>
        </div>
        <p className="muted">不支援的字元會被略過；實際字數與按鍵數可能不同。</p>
      </Panel>

      <Panel title="組合鍵">
        <div className="grid four">
          <label className="field">
            <span>usages（逗號分隔，16 進位）</span>
            <input value={chordKeys} onChange={(event) => setChordKeys(event.target.value)} />
          </label>
          <label className="field">
            <span>按住 (ms)</span>
            <input type="number" value={chordHold} onChange={(event) => setChordHold(intValue(event.target.value, chordHold, 8, 5000))} />
          </label>
          <div className="field">
            <span>&nbsp;</span>
            <button disabled={!state.connected} onClick={sendChord}>
              按下組合
            </button>
          </div>
          <div className="field">
            <span>常用</span>
            <div className="row">
              <button className="small" onClick={() => setChordKeys("E0,E1,06")}>Ctrl+Shift+C</button>
              <button className="small" onClick={() => setChordKeys("E0,1D")}>Ctrl+Z</button>
            </div>
          </div>
        </div>
        <p className="muted">組合鍵用一次 Snapshot 送出，所有鍵在同一個 HID frame 生效。</p>
      </Panel>

      <Panel
        title="巨集與歷史"
        actions={
          <>
            {recording ? (
              <button className="primary" onClick={() => setRecording([])}>
                重新錄製
              </button>
            ) : (
              <button onClick={() => { setRecording([]); startedAt.current = Date.now(); }}>
                開始錄製
              </button>
            )}
            <button
              disabled={!recording || recording.length === 0}
              onClick={() => {
                if (!recording) return;
                void call("macro_save", { name: macroName, steps: recording })
                  .then((path) => {
                    dispatch({ type: "event", event: { kind: "log", line: `巨集已存：${path}` } });
                    return call("macro_list", {});
                  })
                  .then(setMacros)
                  .catch((error) => reportError(dispatch, error, "macro_save"));
              }}
            >
              儲存巨集
            </button>
          </>
        }
      >
        <div className="grid two">
          <label className="field">
            <span>巨集名稱</span>
            <input value={macroName} onChange={(event) => setMacroName(event.target.value)} />
          </label>
          <div className="field">
            <span>已存巨集</span>
            <div className="row">
              {macros.length === 0 ? (
                <span className="muted">尚無</span>
              ) : (
                macros.map((name) => (
                  <button
                    key={name}
                    className="small"
                    onClick={() => {
                      setMacroName(name);
                      void call("macro_load", { name })
                        .then((steps) => runMacro(steps as MacroStep[], 1))
                        .catch((error) => reportError(dispatch, error, "macro_load"));
                    }}
                  >
                    播放 {name}
                  </button>
                ))
              )}
            </div>
          </div>
        </div>
        {recording && (
          <p className="muted">
            錄製中：{recording.length} 步
            <button
              className="small"
              disabled={recording.length === 0}
              onClick={() => void runMacro(recording, 1)}
            >
              播放錄製內容
            </button>
          </p>
        )}
        <div className="console small">
          {history.length === 0 ? <span className="muted">尚無動作。</span> : history.map((line, index) => <div key={index}>{line}</div>)}
        </div>
      </Panel>
    </div>
  );
}
