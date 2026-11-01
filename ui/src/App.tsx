import { useEffect, useState } from "react";

import { ErrorBanner } from "./components/ErrorBanner";
import { DevicePage } from "./pages/DevicePage";
import { FirmwarePage } from "./pages/FirmwarePage";
import { LatencyPage } from "./pages/LatencyPage";
import { MousePage } from "./pages/MousePage";
import { StatusPage } from "./pages/StatusPage";
import { call, isMock } from "./lib/ipc";
import { reportError, useStore } from "./store";

const PAGES = [
  { id: "device", label: "裝置", component: DevicePage },
  { id: "status", label: "即時狀態", component: StatusPage },
  { id: "firmware", label: "韌體", component: FirmwarePage },
  { id: "latency", label: "延遲", component: LatencyPage },
  { id: "mouse", label: "滑鼠", component: MousePage },
] as const;

type PageId = (typeof PAGES)[number]["id"];

function StalePill({ build, commit, onOpen }: { build: number; commit: string; onOpen: () => void }) {
  const board = build.toString(16).padStart(8, "0");
  if (!build || commit === "unknown" || commit.startsWith(board)) return null;
  return (
    <button className="pill warn" onClick={onOpen} title="板的 build 與目前程式不同">
      板子韌體較舊（{board}）— 前往燒錄
    </button>
  );
}

export default function App() {
  const { state, dispatch, saveSettings } = useStore();
  const [page, setPage] = useState<PageId>("device");
  const Page = PAGES.find((entry) => entry.id === page)!.component;

  const choose = async (target: string) => {
    try {
      await call("choose_target", { target });
    } catch (error) {
      reportError(dispatch, error, "choose_target");
    }
  };

  // Mirrors Settings::debug; starts from the in-memory default until the file loads.
  const [debug, setDebug] = useState<boolean>(true);
  useEffect(() => {
    if (state.settings) setDebug(state.settings.debug);
  }, [state.settings]);
  // Keep the worker in step with the saved mode, including right after connect.
  useEffect(() => {
    if (state.connected) {
      void call("set_debug", { enabled: debug }).catch((error) => reportError(dispatch, error, "set_debug"));
    }
  }, [state.connected, debug, dispatch]);

  const toggleDebug = async () => {
    const next = !debug;
    setDebug(next);
    try {
      await call("set_debug", { enabled: next });
      // Persist even when settings_load has not landed yet: saveSettings starts
      // from the defaults, so the toggle no longer silently drops the change.
      await saveSettings({ debug: next });
    } catch (error) {
      setDebug(!next);
      reportError(dispatch, error, "set_debug");
    }
  };

  return (
    <div className="app">
      <aside className="sidebar">
        <div className="brand">
          <strong>FirmwareManageTool</strong>
          <span className="muted">{state.info ? `v${state.info.version} · ${state.info.commit}` : "…"}</span>
        </div>
        <nav>
          {PAGES.map((entry) => (
            <button key={entry.id} className={entry.id === page ? "nav active" : "nav"} onClick={() => setPage(entry.id)}>
              {entry.label}
            </button>
          ))}
        </nav>
        <div className="sidebar-foot">
          {isMock() ? <span className="pill warn">mock</span> : <span className="pill ok">live</span>}
          <span className={`pill ${state.connected ? "ok" : ""}`}>{state.connected ? "已連線" : "未連線"}</span>
        </div>
      </aside>

      <main className="main">
        <header className="topbar">
          <div className="device-line">
            {state.device ? (
              <>
                <span className="mono">{state.device.product}</span>
                <span className="muted">{state.device.serial}</span>
              </>
            ) : (
              <span className="muted">尚未連線</span>
            )}
          </div>
          <div className="topbar-actions">
            {state.ext && <span className="pill mono">fw {state.hello?.firmware} · build {state.ext.build.toString(16).padStart(8, "0")}</span>}
            {state.ext && state.info && (
              <StalePill build={state.ext.build} commit={state.info.commit} onOpen={() => setPage("firmware")} />
            )}
            <button
              className={`small ${debug ? "" : "primary"}`}
              title={
                debug
                  ? "除錯模式：150 ms STATUS、每秒診斷、每行 HID 都顯示"
                  : "遊戲模式：只保留 500 ms 心跳，不再輪詢裝置"
              }
              onClick={() => void toggleDebug()}
            >
              {debug ? "除錯模式" : "遊戲模式"}
            </button>
            {state.busy && <span className="pill warn">工作中…</span>}
            {state.connected && (
              <button
                className="danger small"
                onClick={() => void call("disconnect", {}).catch((error) => reportError(dispatch, error, "disconnect"))}
              >
                中斷
              </button>
            )}
          </div>
        </header>

        {state.errors.length > 0 && (
          <div className="errors">
            {state.errors.map((error, index) => (
              <ErrorBanner
                key={`${error.title}-${index}`}
                error={error}
                onDismiss={() => dispatch({ type: "dismissError", index })}
              />
            ))}
          </div>
        )}

        <Page />

        {state.bootChoices && (
          <div className="modal-backdrop">
            <div className="modal">
              <h3>選擇 bootloader 磁碟</h3>
              <p className="muted">偵測到多個候選，請選擇要寫入的 RPI-RP2。</p>
              <div className="row">
                {state.bootChoices.map((candidate) => (
                  <button key={candidate} className="primary" onClick={() => void choose(candidate)}>
                    {candidate}
                  </button>
                ))}
              </div>
            </div>
          </div>
        )}
      </main>
    </div>
  );
}
