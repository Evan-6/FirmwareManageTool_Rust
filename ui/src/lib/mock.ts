// Browser-only fake backend (`?mock=1`, or any non-Tauri load). It implements
// every command and replays a believable status stream so the UI can be built,
// asserted and screenshotted without a board attached.
import type {
  AppInfo,
  Device,
  LatencyConfig,
  LatencyReport,
  Round,
  Settings,
  SessionInfo,
  Status,
  StatusExt,
  UiEvent,
  UploadConfig,
} from "./types";

const devices: Device[] = [
  {
    index: 0,
    product: "GDBOX KB8",
    serial: "4250304B38353817",
    vid: 0x1209,
    pid: 0x0b0b,
    label: "GDBOX KB8 1209:0B0B 4250304B38353817",
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
  debug: true,
  mouse: {
    curve: 1,
    jitterPx: 1,
    overshootPct: 0,
    settleMs: 0,
    ease: "Jerk",
    minStepPx: 1,
    organic: 0.6,
  },
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
  if (tick % 7 === 0) {
    const sessions: SessionInfo[] = [
      { id: 0x2a, leaseMs: 1600 + (tick % 300), held: 0, flags: 0 },
      { id: 0x9f31, leaseMs: 900 + (tick % 200), held: 2, flags: 1 },
    ];
    emit({ kind: "sessions", sessions });
  }
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
        hotkey: true,
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
          firmware: "3.2.0",
          lease_ms: 2000,
          mouse: true,
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
      emit({ kind: "log", line: `mock: mouse ${JSON.stringify(args.request)}` });
      return null;
    case "type_text":
      emit({ kind: "log", line: `mock: 輸入 ${JSON.stringify(args.config)}` });
      emit({ kind: "done" });
      return null;
    case "chord":
      emit({ kind: "log", line: `mock: 組合鍵 ${JSON.stringify(args.config)}` });
      emit({ kind: "done" });
      return null;
    case "trajectory_preview": {
      const profile = args.profile as { curve: number; jitterPx: number; overshootPct: number; settleMs: number };
      const width = 1920;
      const height = 1080;
      const startX = 600;
      const startY = 500;
      const nodes = [];
      const steps = 60;
      for (let step = 1; step <= steps; step += 1) {
        const p = step / steps;
        const t = p * p * p * (10 + p * (-15 + 6 * p));
        const jitter = profile.jitterPx * Math.sin(Math.PI * p) * Math.sin(6 * p);
        nodes.push({
          atMs: Math.round(260 * p),
          x: Math.round(startX + 300 * t + jitter),
          y: Math.round(startY + 120 * t + jitter * (step % 2 ? 1 : -1)),
        });
      }
      if (profile.overshootPct > 0) nodes.push({ atMs: 300, x: startX + 320, y: startY + 130 });
      if (profile.overshootPct > 0) nodes.push({ atMs: 380, x: startX + 300, y: startY + 120 });
      return { startX, startY, width, height, nodes };
    }
    case "macro_save":
      emit({ kind: "log", line: `mock: 巨集 ${String(args.name)} 已存（${(args.steps as unknown[]).length} 步）` });
      return `mock/${String(args.name)}.json`;
    case "macro_load":
      return [
        { kind: "chord", at: 0, config: { usages: [0xe0, 0x06], holdMs: 60 } },
        { kind: "text", at: 400, config: { text: "mock", intervalMs: 45, holdMs: 18, jitterMs: 30, typoRate: 0.02 } },
      ];
    case "set_debug":
      emit({ kind: "log", line: `mock: 除錯模式 ${args.enabled ? "開" : "關"}` });
      return null;
    case "macro_list":
      return ["mock-demo"];
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
  // Bootstrap goes to the subscriber only, like the real backend: broadcasting it
  // made every remount replay "已啟動" into the other (still attached) listeners.
  handler({ kind: "devices", devices });
  handler({ kind: "log", line: "mock 後端已啟動（?mock=1）" });
  return () => {
    listeners = listeners.filter((entry) => entry !== handler);
  };
}

export const mockFailsafe = (value: number) => {
  failsafe = value;
};
