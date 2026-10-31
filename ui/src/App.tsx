import { useState } from "react";

import { DevicePage } from "./pages/DevicePage";
import { FirmwarePage } from "./pages/FirmwarePage";
import { LatencyPage } from "./pages/LatencyPage";
import { MousePage } from "./pages/MousePage";
import { StatusPage } from "./pages/StatusPage";
import { call, isMock } from "./lib/ipc";
import { useStore } from "./store";

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
  const { state, dispatch } = useStore();
  const [page, setPage] = useState<PageId>("device");
  const Page = PAGES.find((entry) => entry.id === page)!.component;

  const choose = async (target: string) => {
    await call("choose_target", { target });
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
            {state.busy && <span className="pill warn">工作中…</span>}
            {state.connected && (
              <button className="danger small" onClick={() => void call("disconnect", {})}>
                中斷
              </button>
            )}
          </div>
        </header>

        {state.errors.length > 0 && (
          <div className="errors">
            {state.errors.slice(-3).map((message, index) => (
              <div className="banner bad" key={index}>
                {message}
                <button className="small" onClick={() => dispatch({ type: "clearErrors" })}>
                  知道了
                </button>
              </div>
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
