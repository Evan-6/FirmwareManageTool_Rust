// Browser-only fake backend (`?mock=1`, or any non-Tauri load). It implements
// every command and replays a believable status stream so the UI can be built,
// asserted and screenshotted without a board attached.
import type {
  AppInfo,
  Device,
  LatencyConfig,
  LatencyReport,
  MouseAction,
  Round,
  Settings,
  Status,
  StatusExt,
  UiEvent,
  UploadConfig,
} from "./types";

const devices: Device[] = [
  {
    index: 0,
    product: "HP Keyboard",
    serial: "4250304B38353817",
    vid: 0x03f0,
    pid: 0x0024,
    label: "HP Keyboard 03F0:0024 4250304B38353817",
  },
];

let connected = false;
let submitted = 0;
let completed = 0;
let failsafe = 3;
let tick = 0;
let bootCount = 4;
let listeners: Array<(event: UiEvent) => void> = [];

const settings: Settings = {
  upload: {
    cargo_path: "",
    firmware_path: "",
    skip_dependencies: false,
    auto_bootloader: true,
    boot_target: "",
  },
  latency: { rounds: 50, mode: "Normal" },
  csv_path: "",
};

function emit(event: UiEvent) {
  for (const listener of listeners) listener(event);
}

function status(): Status {
  return {
    received: submitted,
    completed,
    pending: Math.max(0, submitted - completed),
    leaseMs: 1500 + (tick % 400),
    rx: 0,
    tx: 0,
    hid: 0,
    failsafe,
    held: 0,
    buttons: 0,
    keys: [],
    raw: `mock received=${submitted} completed=${completed}`,
  };
}

function ext(): StatusExt {
  return {
    uptimeMs: tick * 150,
    lateMaxUs: 12 + (tick % 7),
    outDepth: Math.max(0, submitted - completed),
    rxDepth: 0,
    txDepth: 0,
    sessions: 1,
    lastFault: 0,
    lastFaultAgeMs: null,
    resetReason: 3,
    bootCount,
    lockKeys: 0,
    build: 0x079d5ae9,
  };
}

setInterval(() => {
  tick += 1;
  if (!connected) return;
  submitted += Math.random() < 0.2 ? 1 : 0;
  completed = submitted;
  emit({ kind: "status", status: status() });
  if (tick % 7 === 0) emit({ kind: "ext", ext: ext() });
}, 150);

export async function mockCall(name: string, args: Record<string, unknown>): Promise<unknown> {
  switch (name) {
    case "app_info":
      return {
        version: "0.1.0",
        product: "FirmwareManageTool",
        commit: "mock",
        target: "thumbv6m-none-eabi",
        firmwareDir: "D:/WorkSpace/FirmwareManageTool_Rust/firmware",
      } satisfies AppInfo;
    case "scan":
      return devices;
    case "connect":
      connected = true;
      submitted = 12;
      completed = 11;
      emit({ kind: "devices", devices });
      emit({
        kind: "connected",
        device: devices[0],
        hello: {
          protocol: 3,
          nkro: true,
          media: true,
          firmware: "3.1.0",
          lease_ms: 2000,
          mouse: true,
          raw: "mock hello",
        },
      });
      emit({ kind: "status", status: status() });
      emit({ kind: "ext", ext: ext() });
      return null;
    case "disconnect":
      connected = false;
      emit({ kind: "cleanup", confirmed: true });
      emit({ kind: "disconnected" });
      return null;
    case "request_status":
      emit({ kind: "status", status: status() });
      return null;
    case "reset_input":
      emit({ kind: "log", line: "mock: 全部放開" });
      return null;
    case "upload": {
      const config = args.config as UploadConfig;
      emit({ kind: "log", line: `mock: cargo build ${config.cargo_path || "(PATH)"}` });
      const stages = ["檢查工具鏈", "編譯引擎", "編譯 app", "打包 UF2", "等待 BOOTLOADER", "複製 UF2", "驗證"];
      stages.forEach((message, index) => {
        emit({ kind: "progress", stage: index, message });
        emit({ kind: "log", line: `mock: ${message}` });
      });
      if (config.auto_bootloader) emit({ kind: "bootChoices", candidates: ["E:", "F:"] });
      emit({ kind: "upload", outcome: { Verified: "mock xiao-firmware.uf2" } });
      emit({ kind: "done" });
      return null;
    }
    case "latency": {
      const config = args.config as LatencyConfig;
      const rounds: Round[] = [];
      for (let index = 1; index <= config.rounds; index += 1) {
        const latency = 3.1 + Math.random() * 1.4;
        rounds.push({ round: index, latency_ms: latency, outcome: "Success", error: "" });
        emit({ kind: "round", round: rounds[index - 1] });
      }
      const report: LatencyReport = {
        config,
        device: devices[0],
        unix_ms: Date.now(),
        rounds,
        cancelled: false,
        cleanup_confirmed: true,
        terminal_error: null,
      };
      emit({ kind: "report", report });
      emit({ kind: "done" });
      return null;
    }
    case "mouse":
      emit({ kind: "log", line: `mock: mouse ${JSON.stringify(args.action as MouseAction)}` });
      return null;
    case "choose_target":
      emit({ kind: "log", line: `mock: bootloader ${String(args.target)}` });
      return null;
    case "cancel":
      emit({ kind: "stopped" });
      return null;
    case "settings_load":
      return settings;
    case "settings_save":
      Object.assign(settings, args.settings as Settings);
      return null;
    default:
      throw new Error(`mock 未實作 ${name}`);
  }
}

export async function mockEvents(handler: (event: UiEvent) => void): Promise<() => void> {
  listeners.push(handler);
  emit({ kind: "devices", devices });
  emit({ kind: "log", line: "mock 後端已啟動（?mock=1）" });
  return () => {
    listeners = listeners.filter((entry) => entry !== handler);
  };
}

export const mockFailsafe = (value: number) => {
  failsafe = value;
};
