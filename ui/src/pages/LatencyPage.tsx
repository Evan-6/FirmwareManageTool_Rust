import { useEffect, useState } from "react";

import { Panel } from "../components/Panel";
import { describeError } from "../lib/errors";
import { call } from "../lib/ipc";
import { intValue } from "../lib/num";
import { DEFAULT_LATENCY } from "../lib/types";
import type { LatencyConfig, Mode, RoundOutcome } from "../lib/types";
import { useDeferredSave } from "../lib/useDeferredSave";
import { reportError, useStore } from "../store";

const MODES: { value: Mode; label: string }[] = [
  { value: "Normal", label: "一般" },
  { value: "HighPrecision", label: "高精度" },
  { value: "Extreme", label: "極限 REALTIME（僅送出）" },
];

const OUTCOME_LABEL: Record<RoundOutcome, string> = {
  Success: "成功",
  Failed: "失敗",
  Timeout: "逾時",
};

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
  const { state, dispatch, saveSettings } = useStore();
  const [config, setConfig] = useState<LatencyConfig>(state.settings?.latency ?? DEFAULT_LATENCY);

  useEffect(() => {
    const saved = state.settings?.latency;
    if (!saved) return;
    setConfig((current) => (JSON.stringify(current) === JSON.stringify(saved) ? current : saved));
  }, [state.settings]);

  // The chosen mode and round count used to survive only if a run finished;
  // a cancelled test threw away exactly the settings being iterated on.
  useDeferredSave(state.settings !== null, [config], () => {
    void saveSettings({ latency: config }).catch((error) => reportError(dispatch, error, "settings_save"));
  });

  const start = async () => {
    dispatch({ type: "busy", busy: true });
    try {
      await saveSettings({ latency: config }).catch((error) => reportError(dispatch, error, "settings_save"));
      await call("latency", { config });
    } catch (error) {
      reportError(dispatch, error, "latency");
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
            <button
              className="danger"
              onClick={() => void call("cancel", {}).catch((error) => reportError(dispatch, error, "latency"))}
            >
              停止
            </button>
          ) : (
            <button
              className="primary"
              onClick={() => void start()}
              disabled={!state.connected}
              title={state.connected ? "量測「送出到回報」的端到端延遲" : "先到「裝置」頁連線"}
            >
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
              onChange={(event) => setConfig({ ...config, rounds: intValue(event.target.value, config.rounds, 1, 10000) })}
            />
          </label>
        </div>
        <p className="muted">
          {state.connected
            ? "測試會另外啟動一個 worker 行程量測「送出到回報」的端到端延遲；Extreme 模式只送出、不等待回報。"
            : "需要先連線才能量測：測試會獨佔這塊板子的 session。"}
        </p>
      </Panel>

      <Panel title="統計">
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
        {state.report && (
          <p className="muted">
            {state.report.rounds.length} 輪，取消 {state.report.cancelled ? "是" : "否"}，清理確認{" "}
            {state.report.cleanup_confirmed ? "是" : "否"}
            {state.report.terminal_error
              ? `，錯誤：${describeError(state.report.terminal_error, "latency").title}`
              : ""}
          </p>
        )}
      </Panel>

      <Panel title="逐輪結果">
        <div className="console">
          {(state.report?.rounds ?? []).slice(-200).reverse().map((round) => (
            <div
              key={round.round}
              className={round.outcome === "Success" ? "" : "bad-line"}
              title={round.error || undefined}
            >
              #{round.round}{" "}
              {round.outcome === "Success"
                ? `${round.latency_ms?.toFixed(2)} ms`
                : `${OUTCOME_LABEL[round.outcome]}${round.error ? `：${describeError(round.error, "latency").title}` : ""}`}
            </div>
          ))}
          {state.report?.rounds.length ? null : <span className="muted">尚無資料。</span>}
        </div>
      </Panel>
    </div>
  );
}
