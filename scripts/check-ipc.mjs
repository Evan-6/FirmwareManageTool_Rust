// Contract guard between the Rust shell and the React UI.
//
// Two things drift silently in a Tauri app: a command that is registered but not
// typed in the UI (runtime "command not found"), and an event variant the UI
// never handles (an event that vanishes). Both are mechanical here: the list of
// truth is `generate_handler![...]` and `enum UiEvent`, not the #[tauri::command]
// attribute count.
import { readFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";

const root = path.resolve(import.meta.dirname, "..");
const read = (relative) => readFileSync(path.join(root, relative), "utf8");

const camel = (name) => name.charAt(0).toLowerCase() + name.slice(1);

function commandsFromRust() {
  const source = read("src-tauri/src/shell.rs");
  const block = source.match(/generate_handler!\[([\s\S]*?)\]/);
  if (!block) throw new Error("找不到 generate_handler! 清單");
  return [...block[1].matchAll(/crate::commands::([a-z_0-9]+)/g)].map((m) => m[1]);
}

function commandsFromTs() {
  const source = read("ui/src/lib/ipc.ts");
  const block = source.match(/export interface Commands \{([\s\S]*?)\n\}/);
  if (!block) throw new Error("找不到 ui 的 Commands 介面");
  return [...block[1].matchAll(/^\s{2}([a-z_0-9]+):\s*\{/gm)].map((m) => m[1]);
}

function eventsFromRust() {
  const source = read("src-tauri/src/dto.rs");
  const block = source.match(/pub enum UiEvent \{([\s\S]*?)\n\}/);
  if (!block) throw new Error("找不到 UiEvent enum");
  return [...block[1].matchAll(/^\s{4}([A-Z][A-Za-z0-9]*)\s*(?:\{|,)/gm)].map((m) => camel(m[1]));
}

function eventsFromTs() {
  const source = read("ui/src/lib/types.ts");
  const block = source.match(/export type UiEvent =([\s\S]*?);\r?\n/);
  if (!block) throw new Error("找不到 ui 的 UiEvent 型別");
  return [...block[1].matchAll(/kind:\s*"([a-zA-Z0-9]+)"/g)].map((m) => m[1]);
}

function diff(label, rust, ts) {
  const missingInTs = rust.filter((name) => !ts.includes(name));
  const missingInRust = ts.filter((name) => !rust.includes(name));
  const unique = (list) => [...new Set(list)];
  if (missingInTs.length || missingInRust.length) {
    console.error(`${label} 不一致`);
    if (missingInTs.length) console.error(`  Rust 有、UI 沒有：${unique(missingInTs).join(", ")}`);
    if (missingInRust.length) console.error(`  UI 有、Rust 沒有：${unique(missingInRust).join(", ")}`);
    return false;
  }
  console.log(`${label} 一致（${unique(rust).length} 項）`);
  return true;
}

const ok =
  diff("命令清單", commandsFromRust(), commandsFromTs()) &&
  diff("事件清單", eventsFromRust(), eventsFromTs());

process.exit(ok ? 0 : 1);
