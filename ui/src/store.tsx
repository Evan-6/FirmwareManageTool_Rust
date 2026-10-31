// Single store for everything the five pages render. The worker already speaks in
// events, so the reducer just accumulates them: samples for the charts, logs for
// the console, rounds for the latency table.
import { createContext, useContext, useEffect, useMemo, useReducer } from "react";
import type { ReactNode } from "react";

import { call, onEvent } from "./lib/ipc";
import type {
  AppInfo,
  Device,
  Hello,
  LatencyReport,
  Round,
  Settings,
  Status,
  StatusExt,
  UiEvent,
  UploadOutcome,
} from "./lib/types";

export interface Sample {
  at: number;
  status: Status;
}

export interface Store {
  info: AppInfo | null;
  devices: Device[];
  selected: number;
  connected: boolean;
  device: Device | null;
  hello: Hello | null;
  status: Status | null;
  ext: StatusExt | null;
  samples: Sample[];
  logs: string[];
  errors: string[];
  busy: boolean;
  progress: { stage: number; message: string } | null;
  bootChoices: string[] | null;
  rounds: Round[];
  report: LatencyReport | null;
  upload: UploadOutcome | null;
  cleanup: boolean | null;
  settings: Settings | null;
}

const MAX_SAMPLES = 600; // 150 ms cadence → about 90 s of history
const MAX_LOGS = 500;

const initial: Store = {
  info: null,
  devices: [],
  selected: 0,
  connected: false,
  device: null,
  hello: null,
  status: null,
  ext: null,
  samples: [],
  logs: [],
  errors: [],
  busy: false,
  progress: null,
  bootChoices: null,
  rounds: [],
  report: null,
  upload: null,
  cleanup: null,
  settings: null,
};

type Action =
  | { type: "event"; event: UiEvent }
  | { type: "info"; info: AppInfo }
  | { type: "settings"; settings: Settings }
  | { type: "select"; index: number }
  | { type: "busy"; busy: boolean }
  | { type: "clearErrors" }
  | { type: "clearLogs" };

function reducer(state: Store, action: Action): Store {
  switch (action.type) {
    case "info":
      return { ...state, info: action.info };
    case "settings":
      return { ...state, settings: action.settings };
    case "select":
      return { ...state, selected: action.index };
    case "busy":
      return { ...state, busy: action.busy, progress: action.busy ? state.progress : null };
    case "clearErrors":
      return { ...state, errors: [] };
    case "clearLogs":
      return { ...state, logs: [] };
    case "event": {
      const event = action.event;
      switch (event.kind) {
        case "devices":
          return {
            ...state,
            devices: event.devices,
            selected: Math.min(state.selected, Math.max(0, event.devices.length - 1)),
          };
        case "connected":
          return {
            ...state,
            connected: true,
            device: event.device,
            hello: event.hello,
            errors: [],
          };
        case "disconnected":
          return {
            ...state,
            connected: false,
            device: null,
            hello: null,
            status: null,
            ext: null,
            samples: [],
            busy: false,
          };
        case "status": {
          const samples = [...state.samples, { at: Date.now(), status: event.status }];
          if (samples.length > MAX_SAMPLES) samples.splice(0, samples.length - MAX_SAMPLES);
          return { ...state, status: event.status, samples };
        }
        case "ext":
          return { ...state, ext: event.ext };
        case "log": {
          const logs = [...state.logs, event.line];
          if (logs.length > MAX_LOGS) logs.splice(0, logs.length - MAX_LOGS);
          return { ...state, logs };
        }
        case "error":
          return { ...state, errors: [...state.errors, event.message], busy: false };
        case "progress":
          return {
            ...state,
            busy: true,
            progress: { stage: event.stage, message: event.message },
          };
        case "bootChoices":
          return { ...state, bootChoices: event.candidates };
        case "round":
          return { ...state, rounds: [...state.rounds, event.round].slice(-500) };
        case "report":
          return { ...state, report: event.report };
        case "upload":
          return { ...state, upload: event.outcome, bootChoices: null };
        case "cleanup":
          return { ...state, cleanup: event.confirmed };
        case "done":
          return { ...state, busy: false, progress: null };
        case "stopped":
          return { ...state, busy: false, progress: null };
        default:
          return state;
      }
    }
    default:
      return state;
  }
}

const Context = createContext<{ state: Store; dispatch: React.Dispatch<Action> } | null>(null);

export function StoreProvider({ children }: { children: ReactNode }) {
  const [state, dispatch] = useReducer(reducer, initial);

  useEffect(() => {
    let dispose: (() => void) | undefined;
    void onEvent((event) => dispatch({ type: "event", event })).then((unlisten) => {
      dispose = unlisten;
    });
    void call("app_info", {}).then((info) => dispatch({ type: "info", info }));
    void call("settings_load", {}).then((settings) => dispatch({ type: "settings", settings }));
    return () => dispose?.();
  }, []);

  const value = useMemo(() => ({ state, dispatch }), [state]);
  return <Context.Provider value={value}>{children}</Context.Provider>;
}

export function useStore() {
  const context = useContext(Context);
  if (!context) throw new Error("useStore 必須在 StoreProvider 內使用");
  return context;
}

/** Monotonic counters wrap at 16 or 32 bits; gauges (lease, pending) do not. */
export function series(samples: Sample[], read: (status: Status) => number): number[] {
  return samples.map((sample) => read(sample.status));
}

export function deltaSeries(samples: Sample[], read: (status: Status) => number, wrap: number): number[] {
  const values: number[] = [];
  for (let index = 1; index < samples.length; index += 1) {
    const previous = read(samples[index - 1].status);
    const current = read(samples[index].status);
    values.push((current - previous + wrap) % wrap);
  }
  return values;
}

export const WRAP = { u16: 1 << 16, u32: 1 << 32 } as const;
