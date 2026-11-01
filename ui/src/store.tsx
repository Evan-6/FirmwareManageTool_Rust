// Single store for everything the five pages render. The worker already speaks in
// events, so the reducer just accumulates them: samples for the charts, logs for
// the console, rounds for the latency table.
import { createContext, useCallback, useContext, useEffect, useMemo, useReducer, useRef } from "react";
import type { ReactNode } from "react";

import { describeError } from "./lib/errors";
import type { UiError } from "./lib/errors";
import { call, onEvent } from "./lib/ipc";
import { DEFAULT_SETTINGS } from "./lib/types";
import type {
  AppInfo,
  Device,
  Hello,
  LatencyReport,
  Round,
  Settings,
  SessionInfo,
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
  sessions: SessionInfo[];
  samples: Sample[];
  logs: string[];
  errors: UiError[];
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
const MAX_ERRORS = 5;

const initial: Store = {
  info: null,
  devices: [],
  selected: 0,
  connected: false,
  device: null,
  hello: null,
  status: null,
  ext: null,
  sessions: [],
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
  | { type: "error"; error: UiError }
  | { type: "dismissError"; index: number }
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
    case "error": {
      const errors = [...state.errors, action.error].slice(-MAX_ERRORS);
      return { ...state, errors, busy: false };
    }
    case "dismissError":
      return { ...state, errors: state.errors.filter((_, index) => index !== action.index) };
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
            // A successful connect ends the "工作中…" pill the page started.
            busy: false,
            progress: null,
          };
        case "disconnected":
          return {
            ...state,
            connected: false,
            device: null,
            hello: null,
            status: null,
            ext: null,
            sessions: [],
            samples: [],
            busy: false,
            progress: null,
            bootChoices: null,
            // The report described a session that no longer exists; keeping it
            // left the latency page showing statistics for a disconnected board.
            report: null,
            rounds: [],
          };
        case "status": {
          const samples = [...state.samples, { at: Date.now(), status: event.status }];
          if (samples.length > MAX_SAMPLES) samples.splice(0, samples.length - MAX_SAMPLES);
          return { ...state, status: event.status, samples };
        }
        case "ext":
          return { ...state, ext: event.ext };
        case "sessions":
          return { ...state, sessions: event.sessions };
        case "log": {
          const logs = [...state.logs, event.line];
          if (logs.length > MAX_LOGS) logs.splice(0, logs.length - MAX_LOGS);
          return { ...state, logs };
        }
        case "error":
          return {
            ...state,
            errors: [...state.errors, describeError(event.message, "worker")].slice(-MAX_ERRORS),
            busy: false,
          };
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

const Context = createContext<StoreValue | null>(null);

export interface StoreValue {
  state: Store;
  dispatch: React.Dispatch<Action>;
  /** Merge one section into the saved settings and write it to disk. */
  saveSettings: (patch: Partial<Settings>) => Promise<Settings>;
}

/** Pushes a caught error into the one error area, formatted for humans. */
export function reportError(dispatch: React.Dispatch<Action>, error: unknown, source = "") {
  dispatch({ type: "error", error: describeError(error, source) });
}

export function StoreProvider({ children }: { children: ReactNode }) {
  const [state, dispatch] = useReducer(reducer, initial);

  useEffect(() => {
    // StrictMode mounts twice in dev, and `listen()` resolves asynchronously: the
    // first effect's cleanup ran before its promise landed, so its listener stayed
    // attached and every worker line/sample arrived twice (duplicated log pane,
    // doubled sample counters). Cancel whichever arrives after unmount.
    let active = true;
    let dispose: (() => void) | undefined;
    void onEvent((event) => {
      // The discarded first mount's handler must stay silent too: React keeps the
      // reducer state across StrictMode's simulated remount, so its events landed.
      if (active) dispatch({ type: "event", event });
    }).then((unlisten) => {
      if (active) dispose = unlisten;
      else unlisten();
    });
    void call("app_info", {}).then((info) => dispatch({ type: "info", info }));
    void call("settings_load", {})
      .then((settings) => dispatch({ type: "settings", settings }))
      .catch((error) => reportError(dispatch, error, "settings_load"));
    return () => {
      active = false;
      dispose?.();
    };
  }, []);

  // Every page writes through here so two pages editing different sections never
  // overwrite each other with a stale snapshot of the whole Settings object.
  const settingsRef = useRef<Settings | null>(state.settings);
  settingsRef.current = state.settings;

  const saveSettings = useCallback(async (patch: Partial<Settings>) => {
    const settings: Settings = { ...DEFAULT_SETTINGS, ...(settingsRef.current ?? {}), ...patch };
    if (settingsRef.current && JSON.stringify(settingsRef.current) === JSON.stringify(settings)) {
      return settingsRef.current; // nothing changed: do not rewrite the file
    }
    settingsRef.current = settings;
    dispatch({ type: "settings", settings });
    await call("settings_save", { settings });
    return settings;
  }, []);

  const value = useMemo(() => ({ state, dispatch, saveSettings }), [state, saveSettings]);
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
