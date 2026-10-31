import { useState } from "react";

import { Panel } from "../components/Panel";
import { call } from "../lib/ipc";
import type { MouseAction } from "../lib/types";
import { useStore } from "../store";

export function MousePage() {
  const { state, dispatch } = useStore();
  const [x, setX] = useState(120);
  const [y, setY] = useState(-80);
  const [duration, setDuration] = useState(220);
  const [button, setButton] = useState(1);
  const [count, setCount] = useState(1);
  const [wheel, setWheel] = useState(120);

  const send = async (action: MouseAction) => {
    try {
      await call("mouse", { action });
    } catch (error) {
      dispatch({ type: "event", event: { kind: "error", message: String(error) } });
    }
  };

  return (
    <div className="page">
      <Panel title="相對移動">
        <div className="grid four">
          <label className="field">
            <span>dx</span>
            <input type="number" value={x} onChange={(event) => setX(Number(event.target.value))} />
          </label>
          <label className="field">
            <span>dy</span>
            <input type="number" value={y} onChange={(event) => setY(Number(event.target.value))} />
          </label>
          <label className="field">
            <span>時間 (ms)</span>
            <input type="number" value={duration} onChange={(event) => setDuration(Number(event.target.value))} />
          </label>
          <div className="field">
            <span>&nbsp;</span>
            <button disabled={!state.connected} onClick={() => void send({ MoveBy: { x, y, duration_ms: duration } })}>
              移動
            </button>
          </div>
        </div>
        <div className="row">
          <button disabled={!state.connected} onClick={() => void send({ Raw: { x, y } })}>
            直接送位移（不插值）
          </button>
          <button disabled={!state.connected} onClick={() => void send({ MoveTo: { x, y, duration_ms: duration } })}>
            移到絕對座標
          </button>
        </div>
      </Panel>

      <Panel title="點擊與拖曳">
        <div className="grid four">
          <label className="field">
            <span>按鍵（1 左／2 右／3 中）</span>
            <input type="number" min={1} max={5} value={button} onChange={(event) => setButton(Number(event.target.value))} />
          </label>
          <label className="field">
            <span>次數</span>
            <input type="number" min={1} max={3} value={count} onChange={(event) => setCount(Number(event.target.value))} />
          </label>
          <div className="field">
            <span>&nbsp;</span>
            <button disabled={!state.connected} onClick={() => void send({ Click: { button, count } })}>
              點擊
            </button>
          </div>
          <div className="field">
            <span>&nbsp;</span>
            <button disabled={!state.connected} onClick={() => void send({ Drag: { button, x, y, duration_ms: duration } })}>
              拖曳
            </button>
          </div>
        </div>
        <div className="row">
          {[1, 2, 3, 4, 5].map((index) => (
            <button key={index} disabled={!state.connected} onClick={() => void send({ Button: { button: index, down: true } })}>
              {index} 按下
            </button>
          ))}
          <button className="danger" disabled={!state.connected} onClick={() => void send({ Button: { button, down: false } })}>
            放開 {button}
          </button>
          <button disabled={!state.connected} onClick={() => void call("reset_input", {})}>
            全部放開
          </button>
        </div>
      </Panel>

      <Panel title="滾輪">
        <div className="grid four">
          <label className="field">
            <span>垂直</span>
            <input type="number" value={wheel} onChange={(event) => setWheel(Number(event.target.value))} />
          </label>
          <label className="field">
            <span>水平</span>
            <input type="number" value={0} readOnly />
          </label>
          <div className="field">
            <span>&nbsp;</span>
            <button disabled={!state.connected} onClick={() => void send({ Wheel: { vertical: wheel, horizontal: 0 } })}>
              滾輪
            </button>
          </div>
          <div className="field">
            <span>&nbsp;</span>
            <button disabled={!state.connected} onClick={() => void send({ Wheel: { vertical: 0, horizontal: wheel } })}>
              水平滾輪
            </button>
          </div>
        </div>
      </Panel>
    </div>
  );
}
