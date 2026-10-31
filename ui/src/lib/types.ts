// Payload shapes emitted by src-tauri/src/dto.rs. Keep the two in step: the
// contract check in scripts/check-ipc.mjs compares this file with a Rust fixture.

export interface Device {
  index: number | null;
  product: string;
  serial: string;
  vid: number;
  pid: number;
  label: string;
}

export interface Hello {
  protocol: number;
  nkro: boolean;
  media: boolean;
  firmware: string;
  lease_ms: number;
  mouse: boolean;
  raw: string;
}

export interface Status {
  received: number;
  completed: number;
  pending: number;
  leaseMs: number;
  rx: number;
  tx: number;
  hid: number;
  failsafe: number;
  held: number;
  buttons: number;
  keys: string[];
  raw: string;
}

export interface StatusExt {
  uptimeMs: number;
  lateMaxUs: number;
  outDepth: number;
  rxDepth: number;
  txDepth: number;
  sessions: number;
  lastFault: number;
  lastFaultAgeMs: number | null;
  resetReason: number;
  bootCount: number;
  lockKeys: number;
  build: number;
}

export interface SessionInfo {
  id: number;
  leaseMs: number;
  held: number;
  flags: number;
}

export type RoundOutcome = "Success" | "Failed" | "Timeout";

export interface Round {
  round: number;
  latency_ms: number | null;
  outcome: RoundOutcome;
  error: string;
}

export type Mode = "Normal" | "HighPrecision" | "Extreme";

export interface LatencyConfig {
  rounds: number;
  mode: Mode;
}

export interface LatencyReport {
  config: LatencyConfig;
  device: Device;
  unix_ms: number;
  rounds: Round[];
  cancelled: boolean;
  cleanup_confirmed: boolean;
  terminal_error: string | null;
}

export type UploadOutcome =
  | { TransferFailed: string }
  | { TransferredUnverified: string }
  | { Verified: string };

export interface UploadConfig {
  cargo_path: string;
  firmware_path: string;
  skip_dependencies: boolean;
  auto_bootloader: boolean;
  boot_target: string;
}

export interface Settings {
  upload: UploadConfig;
  latency: LatencyConfig;
  mouse: HumanProfile;
}

export type MouseAction =
  | { MoveTo: { x: number; y: number; duration_ms: number } }
  | { MoveBy: { x: number; y: number; duration_ms: number } }
  | { Drag: { button: number; x: number; y: number; duration_ms: number } }
  | { Click: { button: number; count: number } }
  | { Button: { button: number; down: boolean } }
  | { Wheel: { vertical: number; horizontal: number } }
  | { Raw: { x: number; y: number } };

export type UiEvent =
  | { kind: "devices"; devices: Device[] }
  | { kind: "connected"; device: Device; hello: Hello }
  | { kind: "disconnected" }
  | { kind: "status"; status: Status }
  | { kind: "ext"; ext: StatusExt }
  | { kind: "sessions"; sessions: SessionInfo[] }
  | { kind: "log"; line: string }
  | { kind: "error"; message: string }
  | { kind: "progress"; stage: number; message: string }
  | { kind: "bootChoices"; candidates: string[] }
  | { kind: "round"; round: Round }
  | { kind: "report"; report: LatencyReport }
  | { kind: "upload"; outcome: UploadOutcome }
  | { kind: "cleanup"; confirmed: boolean }
  | { kind: "done" }
  | { kind: "stopped" };

export interface AppInfo {
  version: string;
  product: string;
  commit: string;
  target: string;
  firmwareDir: string;
  hotkey: boolean;
}

export function outcomeLabel(outcome: UploadOutcome): string {
  if ("Verified" in outcome) return "燒錄完成且握手通過";
  if ("TransferredUnverified" in outcome) return "燒錄完成，未驗證";
  return "燒錄失敗";
}

export function outcomeDetail(outcome: UploadOutcome): string {
  if ("Verified" in outcome) return outcome.Verified;
  if ("TransferredUnverified" in outcome) return outcome.TransferredUnverified;
  return outcome.TransferFailed;
}

export const RESET_REASONS = ["未知", "軟體重啟", "watchdog", "冷開機"];

export const FAULT_CODES: Record<number, string> = {
  0: "無",
  1: "framing",
  2: "命令尚未完成",
  3: "輸入不合法",
  5: "租約到期",
  6: "HID 傳送失敗",
  7: "USB reset／suspend",
  8: "佇列溢位／輸出過期",
  9: "序號重複",
  10: "序號跳號",
};

export type Ease = "Jerk" | "EaseInOut" | "Linear";

export interface HumanProfile {
  curve: number;
  jitterPx: number;
  overshootPct: number;
  settleMs: number;
  ease: Ease;
  minStepPx: number;
}

export const DEFAULT_PROFILE: HumanProfile = {
  curve: 1,
  jitterPx: 1,
  overshootPct: 0,
  settleMs: 0,
  ease: "Jerk",
  minStepPx: 1,
};

export interface MouseRequest {
  action: MouseAction;
  profile: HumanProfile;
}

export interface TypeTextConfig {
  text: string;
  intervalMs: number;
  holdMs: number;
  jitterMs: number;
}

export interface ChordConfig {
  usages: number[];
  holdMs: number;
}

export interface PreviewNode {
  atMs: number;
  x: number;
  y: number;
}

export interface TrajectoryPreview {
  startX: number;
  startY: number;
  width: number;
  height: number;
  nodes: PreviewNode[];
}

export type MacroStep =
  | { kind: "mouse"; at: number; request: MouseRequest }
  | { kind: "text"; at: number; config: TypeTextConfig }
  | { kind: "chord"; at: number; config: ChordConfig };
