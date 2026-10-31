// One place where the webview talks to Rust. Commands are a typed map, so a
// renamed command or a wrong payload is a TypeScript error instead of a runtime
// "command not found".
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type {
  AppInfo,
  Device,
  LatencyConfig,
  LatencyReport,
  MouseAction,
  Settings,
  UiEvent,
  UploadConfig,
} from "./types";

export interface Commands {
  app_info: { args: Record<string, never>; result: AppInfo };
  scan: { args: Record<string, never>; result: Device[] };
  connect: { args: { index: number }; result: null };
  disconnect: { args: Record<string, never>; result: null };
  request_status: { args: Record<string, never>; result: null };
  reset_input: { args: Record<string, never>; result: null };
  upload: { args: { config: UploadConfig }; result: null };
  latency: { args: { config: LatencyConfig }; result: null };
  mouse: { args: { action: MouseAction }; result: null };
  choose_target: { args: { target: string }; result: null };
  cancel: { args: Record<string, never>; result: null };
  export_report_csv: { args: { report: LatencyReport; path?: string | null }; result: string };
  settings_load: { args: Record<string, never>; result: Settings };
  settings_save: { args: { settings: Settings }; result: null };
}

export type CommandName = keyof Commands;

export class IpcError extends Error {
  constructor(
    readonly command: CommandName,
    readonly detail: string,
  ) {
    super(`${command}: ${detail}`);
  }
}

export const isTauri = (): boolean =>
  typeof (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ !==
  "undefined";

export const isMock = (): boolean =>
  new URLSearchParams(window.location.search).get("mock") === "1" || !isTauri();

export async function call<K extends CommandName>(
  name: K,
  args: Commands[K]["args"],
): Promise<Commands[K]["result"]> {
  if (isMock()) {
    const { mockCall } = await import("./mock");
    return (await mockCall(name, args)) as Commands[K]["result"];
  }
  try {
    return (await invoke(name, args)) as Commands[K]["result"];
  } catch (error) {
    throw new IpcError(name, String(error));
  }
}

/** Quiet variant for background polling: a failure must not toast the user. */
export async function callQuiet<K extends CommandName>(
  name: K,
  args: Commands[K]["args"],
): Promise<Commands[K]["result"] | null> {
  try {
    return await call(name, args);
  } catch {
    return null;
  }
}

export async function onEvent(handler: (event: UiEvent) => void): Promise<UnlistenFn> {
  if (isMock()) {
    const { mockEvents } = await import("./mock");
    return mockEvents(handler);
  }
  return listen<UiEvent>("app://event", (payload) => handler(payload.payload));
}
