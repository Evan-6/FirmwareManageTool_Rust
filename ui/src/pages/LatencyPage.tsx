import { useEffect, useState } from "react";

import { Panel } from "../components/Panel";
import { call } from "../lib/ipc";
import type { LatencyConfig, Mode } from "../lib/types";
import { useStore } from "../store";

const MODES: { value: Mode; label: string }[] = [
  { value: "Normal", label: "一般" },
  { value: "HighPrecision", label: "高精度" },
  { value: "Extreme", label: "極限 REALTIME（僅送出）" },
];

function stats(values: number[]) {
  if (values.length === 0) return null;
  const sorted = [...values].sort((a, b) => a - b);
  const mean = sorted.reduce((sum, value) => sum + value, 0) / sorted.length;
  const pick = (q: number) => sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))];
  return {
    n: sorted.length,
    min: sorted[0],
    max: sorted[sorted.length - 1],
    mean,
    p50: pick(0.5),
    p95: pick(0.95),
    p99: pick(0.99),
  };
}

export function LatencyPage() {
  const { state, dispatch } = useStore();
  const [config, setConfig] = useState<LatencyConfig>(state.settings?.latency ?? { rounds: 50, mode: "Normal" });

  useEffect(() => {
    if (state.settings) setConfig(state.settings.latency);
  }, [state.settings]);

  const start = async () => {
    dispatch({ type: "busy", busy: true });
    try {
      await call("latency", { config });
      if (state.settings) {
        const settings = { ...state.settings, latency: config };
        await call("settings_save", { settings });
        dispatch({ type: "settings", settings });
      }
    } catch (error) {
      dispatch({ type: "event", event: { kind: "error", message: String(error) } });
    }
  };

  const [csvPath, setCsvPath] = useState("");

  const exportCsv = async () => {
    if (!state.report) return;
    try {
      const path = await call("export_report_csv", {
        report: state.report,
        path: state.settings?.csv_path ?? "",
      });
      setCsvPath(path);
      dispatch({ type: "event", event: { kind: "log", line: `CSV 已寫入 ${path}` } });
    } catch (error) {
      dispatch({ type: "event", event: { kind: "error", message: String(error) } });
    }
  };

  const values = (state.report?.rounds ?? []).filter((round) => round.outcome === "Success" && round.latency_ms != null).map((round) => round.latency_ms!);
  const summary = stats(values);

  return (
    <div className="page">
      <Panel
        title="輸入延遲測試"
        actions={
          state.busy ? (
            <button className="danger" onClick={() => void call("cancel", {})}>
              停止
            </button>
          ) : (
            <button className="primary" onClick={() => void start()}>
              開始
            </button>
          )
        }
      >
        <div className="grid two">
          <label className="field">
            <span>模式</span>
            <select value={config.mode} onChange={(event) => setConfig({ ...config, mode: event.target.value as Mode })}>
              {MODES.map((mode) => (
                <option key={mode.value} value={mode.value}>
                  {mode.label}
                </option>
              ))}
            </select>
          </label>
          <label className="field">
            <span>輪數（1–10000）</span>
            <input
              type="number"
              min={1}
              max={10000}
              value={config.rounds}
              onChange={(event) => setConfig({ ...config, rounds: Number(event.target.value) || 1 })}
            />
          </label>
        </div>
        <p className="muted">
          測試會另外啟動一個 worker 行程量測「送出到回報」的端到端延遲；Extreme 模式只送出、不等待回報。
        </p>
      </Panel>

      <Panel
        title="統計"
        actions={
          <button disabled={!state.report} onClick={() => void exportCsv()}>
            匯出 CSV
          </button>
        }
      >
        {!summary ? (
          <p className="muted">尚無資料。</p>
        ) : (
          <div className="metrics tight">
            {[
              ["樣本", summary.n.toString()],
              ["最小", `${summary.min.toFixed(2)} ms`],
              ["中位數", `${summary.p50.toFixed(2)} ms`],
              ["平均", `${summary.mean.toFixed(2)} ms`],
              ["p95", `${summary.p95.toFixed(2)} ms`],
              ["p99", `${summary.p99.toFixed(2)} ms`],
              ["最大", `${summary.max.toFixed(2)} ms`],
            ].map(([label, value]) => (
              <div className="metric" key={label}>
                <div className="metric-head">
                  <span>{label}</span>
                </div>
                <div className="metric-value">{value}</div>
              </div>
            ))}
          </div>
        )}
        {csvPath && <p className="muted mono">CSV：{csvPath}</p>}
        {state.report && (
          <p className="muted">
            {state.report.rounds.length} 輪，取消 {String(state.report.cancelled)}，清理確認{" "}
            {String(state.report.cleanup_confirmed)}
            {state.report.terminal_error ? `，錯誤：${state.report.terminal_error}` : ""}
          </p>
        )}
      </Panel>

      <Panel title="逐輪結果">
        <div className="console">
          {(state.report?.rounds ?? []).slice(-200).reverse().map((round) => (
            <div key={round.round} className={round.outcome === "Success" ? "" : "bad-line"}>
              #{round.round} {round.outcome === "Success" ? `${round.latency_ms?.toFixed(2)} ms` : `${round.outcome} ${round.error}`}
            </div>
          ))}
          {state.report?.rounds.length ? null : <span className="muted">尚無資料。</span>}
        </div>
      </Panel>
    </div>
  );
}
