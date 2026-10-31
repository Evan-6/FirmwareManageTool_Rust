import { Metric } from "../components/Metric";
import { Panel } from "../components/Panel";
import { Sparkline } from "../components/Sparkline";
import { deltaSeries, series, useStore, WRAP } from "../store";
import { FAULT_CODES, RESET_REASONS } from "../lib/types";

function rate(values: number[]): string {
  if (values.length === 0) return "—";
  const window = values.slice(-20);
  const mean = window.reduce((sum, value) => sum + value, 0) / window.length;
  return `${(mean * (1000 / 150)).toFixed(1)}/s`;
}

export function StatusPage() {
  const { state } = useStore();
  const samples = state.samples;

  const received = deltaSeries(samples, (s) => s.received, WRAP.u32);
  const completed = deltaSeries(samples, (s) => s.completed, WRAP.u32);
  const rx = deltaSeries(samples, (s) => s.rx, WRAP.u16);
  const tx = deltaSeries(samples, (s) => s.tx, WRAP.u16);
  const hid = deltaSeries(samples, (s) => s.hid, WRAP.u16);
  const failsafe = deltaSeries(samples, (s) => s.failsafe, WRAP.u16);
  const pending = series(samples, (s) => s.pending);
  const lease = series(samples, (s) => s.leaseMs);

  const transportBad = rx.at(-1)! > 0 || tx.at(-1)! > 0 || hid.at(-1)! > 0;
  const failsafeBad = failsafe.at(-1)! > 0;
  const health = transportBad
    ? { cls: "bad", text: "傳輸計數在增加：檢查線材／hub／USB 埠" }
    : failsafeBad
      ? { cls: "warn", text: "租約回收：有程式沒有送出 CLOSE（崩潰或直接拔線）" }
      : { cls: "ok", text: "傳輸正常" };

  return (
    <div className="page">
      {!(state.settings?.debug ?? true) && (
        <div className="banner warn">
          遊戲模式：不再輪詢裝置，圖表停在最後一筆；心跳仍每 500 ms 續租，鍵盤功能照常。
        </div>
      )}
      <div className={`banner ${health.cls}`}>{health.text}</div>

      {!state.connected ? (
        <p className="muted">尚未連線。連線後每秒約 6 筆樣本，保留最近 90 秒。</p>
      ) : (
        <>
          <div className="metrics">
            <Metric label="received" value={state.status?.received ?? 0} hint={`${rate(received)} 每樣本增量`} values={received.slice(-120)} />
            <Metric label="completed" value={state.status?.completed ?? 0} hint={`${rate(completed)} 每樣本增量`} values={completed.slice(-120)} colour="var(--ok)" />
            <Metric label="pending" value={state.status?.pending ?? 0} hint="待送工作（上限 32）" values={pending} colour="var(--warn)" />
            <Metric label="lease" value={`${state.status?.leaseMs ?? 0} ms`} hint="剩餘租約，STATUS 不續租" values={lease} colour="var(--violet)" />
            <Metric label="rx 錯誤" value={state.status?.rx ?? 0} hint="framing／序號" values={rx} colour="var(--bad)" />
            <Metric label="tx 錯誤" value={state.status?.tx ?? 0} hint="TX 佇列溢位" values={tx} colour="var(--bad)" />
            <Metric label="hid 失敗" value={state.status?.hid ?? 0} hint="主機拒收輸入報告" values={hid} colour="var(--bad)" />
            <Metric label="failsafe" value={state.status?.failsafe ?? 0} hint="租約／故障回收次數" values={failsafe} colour="var(--warn)" />
          </div>

          <Panel title="診斷（STATUS_EXT，每秒一次）">
            {!state.ext ? (
              <p className="muted">這顆韌體沒有回 STATUS_EXT。</p>
            ) : (
              <div className="grid two">
                <dl className="kv">
                  <dt>uptime</dt>
                  <dd>{(state.ext.uptimeMs / 1000).toFixed(1)} s</dd>
                  <dt>tick 最大延遲</dt>
                  <dd>{state.ext.lateMaxUs} µs</dd>
                  <dt>佇列 out／rx／tx</dt>
                  <dd className="mono">
                    {state.ext.outDepth} / {state.ext.rxDepth} / {state.ext.txDepth}
                  </dd>
                  <dt>session</dt>
                  <dd>{state.ext.sessions}</dd>
                </dl>
                <dl className="kv">
                  <dt>最近故障</dt>
                  <dd>
                    {state.ext.lastFault === 0
                      ? "無"
                      : `${FAULT_CODES[state.ext.lastFault] ?? state.ext.lastFault}${state.ext.lastFaultAgeMs != null ? `（${(state.ext.lastFaultAgeMs / 1000).toFixed(1)} s 前）` : ""}`}
                  </dd>
                  <dt>reset 原因</dt>
                  <dd>{RESET_REASONS[state.ext.resetReason] ?? "未知"}</dd>
                  <dt>開機次數</dt>
                  <dd>{state.ext.bootCount}</dd>
                </dl>
              </div>
            )}
          </Panel>

          <Panel title="連線擁有者（SESSION_LIST）">
            {state.sessions.length === 0 ? (
              <p className="muted">沒有其他連線。</p>
            ) : (
              <table className="table">
                <thead>
                  <tr>
                    <th>session</th>
                    <th>剩餘租約</th>
                    <th>按住鍵</th>
                    <th>狀態</th>
                  </tr>
                </thead>
                <tbody>
                  {state.sessions.map((session) => (
                    <tr key={session.id}>
                      <td className="mono">{session.id.toString(16).padStart(4, "0")}</td>
                      <td>{session.leaseMs} ms</td>
                      <td>{session.held}</td>
                      <td>
                        {[
                          session.flags & 1 ? "有待送命令" : null,
                          session.flags & 2 ? "已釋放" : null,
                          session.flags & 4 ? "要求 BOOTSEL" : null,
                        ]
                          .filter(Boolean)
                          .join("・") || "—"}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </Panel>

          <Panel title="原始 STATUS">
            <div className="console small">{state.status?.raw ?? ""}</div>
            <div className="row">
              <span className="muted">樣本數 {samples.length}</span>
              <Sparkline values={received.slice(-200)} height={26} />
            </div>
          </Panel>
        </>
      )}
    </div>
  );
}
