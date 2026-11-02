import { useEffect, useState } from "react";
import { Panel } from "../components/Panel";
import { call, isMock } from "../lib/ipc";
import { reportError, useStore } from "../store";

// How often a disconnected tool looks for a board. Plugging the cable in is the
/// common case, so waiting for a click is the wrong default.
const RESCAN_MS = 2000;

export function DevicePage() {
  const { state, dispatch } = useStore();
  const [autoTried, setAutoTried] = useState<string | null>(null);

  const connect = async (index = state.selected) => {
    dispatch({ type: "busy", busy: true });
    try {
      await call("connect", { index });
    } catch (error) {
      reportError(dispatch, error, "connect");
    }
  };

  const disconnect = async () => {
    try {
      await call("disconnect", {});
      dispatch({ type: "busy", busy: false });
    } catch (error) {
      reportError(dispatch, error, "disconnect");
    }
  };

  const scan = async () => {
    try {
      const devices = await call("scan", {});
      dispatch({ type: "event", event: { kind: "devices", devices } });
    } catch (error) {
      reportError(dispatch, error, "scan");
    }
  };

  // Look once on mount, then keep looking while disconnected: plugging the board
  // in should be enough, the same way the firmware pages assume it is there.
  useEffect(() => {
    if (state.connected) return;
    void scan();
    const timer = setInterval(() => void scan(), RESCAN_MS);
    return () => clearInterval(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state.connected]);

  // One board on the bus and we have not tried it yet: connect to it. The guard
  // resets when the bus empties, so unplug/replug tries again.
  useEffect(() => {
    if (state.connected || state.busy) return;
    if (state.devices.length === 0) {
      if (autoTried !== null) setAutoTried(null);
      return;
    }
    if (state.devices.length !== 1) return;
    const key = state.devices[0].serial || state.devices[0].label;
    if (autoTried === key) return;
    setAutoTried(key);
    void connect(0);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state.devices, state.connected, state.busy, autoTried]);

  const run = (name: "reset_input" | "request_status") => {
    void call(name, {}).catch((error) => reportError(dispatch, error, name));
  };

  const capabilities = state.hello;
  const build = state.ext?.build ?? 0;

  return (
    <div className="page">
      <Panel
        title="Vendor HID 裝置"
        actions={
          <>
            <button onClick={() => void scan()}>
              重新掃描
            </button>
            {state.connected ? (
              <button className="danger" onClick={() => void disconnect()}>
                中斷
              </button>
            ) : (
              <button className="primary" disabled={state.devices.length === 0} onClick={() => void connect()}>
                連線
              </button>
            )}
          </>
        }
      >
        {state.devices.length === 0 ? (
          <p className="muted">
            找不到 Vendor HID（1209:0B0B，usage FF60:0061）。每 2 秒自動重新掃描，插上板子就會自動連線；
            只有一個裝置時不用選，直接連上。
          </p>
        ) : (
          <table className="table">
            <thead>
              <tr>
                <th />
                <th>產品</th>
                <th>VID:PID</th>
                <th>序號</th>
              </tr>
            </thead>
            <tbody>
              {state.devices.map((device, index) => (
                <tr
                  key={device.serial}
                  className={index === state.selected ? "selected" : ""}
                  onClick={() => dispatch({ type: "select", index })}
                >
                  <td>
                    <input type="radio" checked={index === state.selected} readOnly />
                  </td>
                  <td>{device.product}</td>
                  <td className="mono">
                    {device.vid.toString(16).toUpperCase().padStart(4, "0")}:
                    {device.pid.toString(16).toUpperCase().padStart(4, "0")}
                  </td>
                  <td className="mono">{device.serial}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
        {isMock() && <p className="muted">目前是 mock 後端（瀏覽器模式），所有命令都是假的。</p>}
      </Panel>

      <Panel title="能力與身分">
        {!capabilities ? (
          <p className="muted">尚未連線。</p>
        ) : (
          <div className="grid two">
            <dl className="kv">
              <dt>協定</dt>
              <dd>v{capabilities.protocol}</dd>
              <dt>韌體</dt>
              <dd className="mono">{capabilities.firmware}</dd>
              <dt>build</dt>
              <dd className="mono">
                {build ? build.toString(16).padStart(8, "0") : "未知"}
                {state.info?.commit && state.info.commit !== "unknown" && (
                  <span className={state.info.commit.startsWith(build.toString(16).padStart(8, "0")) ? "pill ok" : "pill warn"}>
                    {state.info.commit.startsWith(build.toString(16).padStart(8, "0")) ? "與程式相同" : `程式 ${state.info.commit}`}
                  </span>
                )}
              </dd>
              <dt>租約</dt>
              <dd>{capabilities.lease_ms} ms</dd>
              <dt>NKRO／多媒體／滑鼠</dt>
              <dd>
                {capabilities.nkro ? "✓" : "✗"} / {capabilities.media ? "✓" : "✗"} / {capabilities.mouse ? "✓" : "✗"}
              </dd>
            </dl>
            <dl className="kv">
              <dt>目前按鍵</dt>
              <dd>{state.status?.keys.length ? state.status.keys.join(" ") : "—"}</dd>
              <dt>滑鼠鍵</dt>
              <dd className="mono">{state.status?.buttons.toString(16).padStart(2, "0") ?? "00"}</dd>
              <dt>session</dt>
              <dd>{state.ext?.sessions ?? "—"}</dd>
              <dt>開機次數</dt>
              <dd>{state.ext?.bootCount ?? "—"}</dd>
              <dt>reset 原因</dt>
              <dd>{state.ext ? ["未知", "軟體重啟", "watchdog", "冷開機"][state.ext.resetReason] ?? state.ext.resetReason : "—"}</dd>
            </dl>
          </div>
        )}
        <div className="row">
          <button disabled={!state.connected} onClick={() => run("reset_input")}>
            全部放開
          </button>
          <span className={state.info?.hotkey ? "pill ok" : "pill warn"}>
            {state.info?.hotkey ? "緊急放開：Ctrl+Alt+Shift+Esc" : "緊急放開熱鍵不可用"}
          </span>
          <button disabled={!state.connected} onClick={() => run("request_status")}>
            立即讀取 STATUS
          </button>
        </div>
      </Panel>
    </div>
  );
}
