import { useEffect, useState } from "react";

import { Panel } from "../components/Panel";
import { call } from "../lib/ipc";
import { DEFAULT_UPLOAD } from "../lib/types";
import type { UploadConfig } from "../lib/types";
import { outcomeDetail, outcomeLabel } from "../lib/types";
import { useDeferredSave } from "../lib/useDeferredSave";
import { reportError, useStore } from "../store";

const STAGES = ["工具鏈", "引擎", "編譯", "打包 UF2", "BOOTLOADER", "複製", "驗證"];

export function FirmwarePage() {
  const { state, dispatch, saveSettings } = useStore();
  const [config, setConfig] = useState<UploadConfig>(state.settings?.upload ?? DEFAULT_UPLOAD);

  useEffect(() => {
    const saved = state.settings?.upload;
    if (!saved) return;
    // Adopting the saved value unconditionally would loop: saveSettings dispatches
    // a fresh Settings object, which re-arms the debounced autosave below.
    setConfig((current) => (JSON.stringify(current) === JSON.stringify(saved) ? current : saved));
  }, [state.settings]);

  const patch = (update: Partial<UploadConfig>) => setConfig((current) => ({ ...current, ...update }));

  const save = async () => {
    try {
      await saveSettings({ upload: config });
      dispatch({ type: "event", event: { kind: "log", line: "已儲存燒錄設定" } });
    } catch (error) {
      reportError(dispatch, error, "settings_save");
    }
  };

  // Paths are easy to type and easy to lose: persist them once the edit settles.
  // The explicit button stays as the "I am sure" path.
  useDeferredSave(state.settings !== null, [config], () => {
    void saveSettings({ upload: config }).catch((error) => reportError(dispatch, error, "settings_save"));
  });

  const start = async () => {
    dispatch({ type: "busy", busy: true });
    try {
      // Persist what is about to be built first; otherwise a failed flash also
      // loses the paths that were typed to debug it.
      await saveSettings({ upload: config }).catch((error) => reportError(dispatch, error, "settings_save"));
      await call("upload", { config });
    } catch (error) {
      reportError(dispatch, error, "upload");
    }
  };

  const stage = state.progress?.stage ?? -1;

  return (
    <div className="page">
      <Panel
        title="韌體燒錄"
        actions={
          <>
            <button onClick={() => void save()}>儲存設定</button>
            {state.busy ? (
              <button className="danger" onClick={() => void call("cancel", {}).catch((error) => reportError(dispatch, error, "upload"))}>
                取消
              </button>
            ) : (
              <button className="primary" onClick={() => void start()}>
                建置並燒錄
              </button>
            )}
          </>
        }
      >
        <div className="grid two">
          <label className="field">
            <span>cargo.exe（留空＝PATH）</span>
            <input value={config.cargo_path} onChange={(event) => patch({ cargo_path: event.target.value })} placeholder="C:/Users/…/.cargo/bin/cargo.exe" />
          </label>
          <label className="field">
            <span>firmware 目錄（留空＝內建）</span>
            <input value={config.firmware_path} onChange={(event) => patch({ firmware_path: event.target.value })} placeholder={state.info?.firmwareDir ?? ""} />
          </label>
          <label className="field checkbox">
            <input type="checkbox" checked={config.skip_dependencies} onChange={(event) => patch({ skip_dependencies: event.target.checked })} />
            <span>略過 rustup target add（已知已安裝）</span>
          </label>
          <label className="field checkbox">
            <input type="checkbox" checked={config.auto_bootloader} onChange={(event) => patch({ auto_bootloader: event.target.checked })} />
            <span>自動請板子進入 BOOTSEL</span>
          </label>
          <label className="field">
            <span>指定 bootloader 磁碟（留空＝自動）</span>
            <input value={config.boot_target} onChange={(event) => patch({ boot_target: event.target.value })} placeholder="E:" />
          </label>
          <div className="field">
            <span>目標</span>
            <div className="mono readonly">thumbv6m-none-eabi · features=hardware</div>
          </div>
        </div>

        <div className="stages">
          {STAGES.map((name, index) => (
            <div key={name} className={`stage ${index < stage ? "done" : index === stage ? "active" : ""}`}>
              <span className="stage-index">{index + 1}</span>
              <span>{name}</span>
            </div>
          ))}
        </div>
        {state.progress && <p className="muted">{state.progress.message}</p>}

        {state.upload && (
          <div className={`banner ${"Verified" in state.upload ? "ok" : "warn"}`}>
            {outcomeLabel(state.upload)}：{outcomeDetail(state.upload)}
          </div>
        )}
      </Panel>

      <Panel
        title="輸出"
        actions={
          <button onClick={() => dispatch({ type: "clearLogs" })}>清空</button>
        }
      >
        <div className="console">
          {state.logs.length === 0 ? <span className="muted">尚無輸出。</span> : state.logs.map((line, index) => <div key={index}>{line}</div>)}
        </div>
      </Panel>
    </div>
  );
}
