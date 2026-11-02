// Minimal CDP driver: connects to the Tauri webview, evaluates expressions and
// prints the results. Used to verify the React UI inside the real WebView2.
const list = await (await fetch("http://127.0.0.1:9333/json/list")).json();
const target = list.find((entry) => entry.type === "page");
if (!target) {
  console.error("no page target");
  process.exit(1);
}
const ws = new WebSocket(target.webSocketDebuggerUrl);
let nextId = 1;
const pending = new Map();
const logs = [];

ws.addEventListener("message", (message) => {
  const data = JSON.parse(message.data);
  if (data.id && pending.has(data.id)) {
    pending.get(data.id)(data);
    pending.delete(data.id);
  } else if (data.method === "Runtime.consoleAPICalled") {
    logs.push(
      `${data.params.type}: ` +
        data.params.args.map((arg) => arg.value ?? arg.description ?? "").join(" "),
    );
  } else if (data.method === "Runtime.exceptionThrown") {
    logs.push(`exception: ${data.params.exceptionDetails.text}`);
  }
});

const send = (method, params = {}) =>
  new Promise((resolve) => {
    const id = nextId++;
    pending.set(id, resolve);
    ws.send(JSON.stringify({ id, method, params }));
  });

await new Promise((resolve) => ws.addEventListener("open", resolve));
await send("Runtime.enable");

async function evaluate(expression) {
  const result = await send("Runtime.evaluate", {
    expression,
    awaitPromise: true,
    returnByValue: true,
  });
  if (result.result?.exceptionDetails) {
    return { error: result.result.exceptionDetails.text };
  }
  return result.result?.result?.value;
}

const script = process.argv[2];
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

if (script === "p1") {
  // Humanised movement: profile panel, real trajectory preview, one real move,
  // one text run and one chord — all through the worker.
  await evaluate(`document.querySelectorAll('nav .nav')[0].click()`);
  await sleep(300);
  await evaluate(`Array.from(document.querySelectorAll('button')).find(b => b.textContent.includes('重新掃描')).click()`);
  await sleep(1200);
  await evaluate(`Array.from(document.querySelectorAll('button')).find(b => b.textContent.trim() === '連線').click()`);
  await sleep(2500);

  await evaluate(`document.querySelectorAll('nav .nav')[4].click()`);
  await sleep(1200);
  const panel = await evaluate(`(() => {
    const svg = document.querySelector('svg.trajectory');
    const poly = svg?.querySelector('polyline');
    return {
      sliders: document.querySelectorAll('input[type=range]').length,
      panels: document.querySelectorAll('.panel').length,
      polyPoints: poly ? poly.getAttribute('points').split(' ').length : 0,
      text: document.body.innerText.includes('擬人化參數'),
    };
  })()`);
  console.log("mouse page:", JSON.stringify(panel));

  const before = await evaluate(`(async () => (await window.__TAURI_INTERNALS__.invoke("app_info"), document.body.innerText.match(/received[^0-9]*([0-9]+)/)?.[1] ?? "0"))()`);
  await evaluate(`Array.from(document.querySelectorAll('button')).find(b => b.textContent.trim() === '移動').click()`);
  await sleep(2500);
  const afterMove = await evaluate(`(() => {
    const consoleText = document.querySelector('.console.small')?.textContent ?? '';
    return { history: consoleText.split(String.fromCharCode(10)).slice(0, 2) };
  })()`);
  console.log("after move:", JSON.stringify(afterMove));

  const textResult = await evaluate(`window.__TAURI_INTERNALS__.invoke("type_text", { config: { text: "x", intervalMs: 40, holdMs: 15, jitterMs: 10 } }).then(() => "ok").catch((e) => String(e))`);
  await sleep(1200);
  const chordResult = await evaluate(`window.__TAURI_INTERNALS__.invoke("chord", { config: { usages: [225], holdMs: 50 } }).then(() => "ok").catch((e) => String(e))`);
  await sleep(800);
  console.log("type_text:", textResult, "chord:", chordResult);

  const status = await evaluate(`document.querySelectorAll('nav .nav')[1].click(), null`);
  await sleep(1500);
  const counters = await evaluate(`(() => {
    const metrics = Array.from(document.querySelectorAll('.metric')).slice(0, 3).map(m => m.textContent);
    return { metrics, receivedBefore: ${JSON.stringify(before)} };
  })()`);
  console.log("counters:", JSON.stringify(counters));

  await evaluate(`document.querySelectorAll('nav .nav')[0].click()`);
  await sleep(300);
  await evaluate(`Array.from(document.querySelectorAll('button')).find(b => b.textContent.trim() === '中斷')?.click()`);
  await sleep(1000);
} else if (script === "hotkey") {
  // Connect, fire Ctrl+Alt+Shift+Esc through the OS, then read the log the
  // handler emitted. This is the only way to prove a global hotkey works.
  await evaluate(`document.querySelectorAll('nav .nav')[0].click()`);
  await sleep(300);
  await evaluate(`Array.from(document.querySelectorAll('button')).find(b => b.textContent.includes('重新掃描')).click()`);
  await sleep(1200);
  await evaluate(`Array.from(document.querySelectorAll('button')).find(b => b.textContent.trim() === '連線').click()`);
  await sleep(2500);

  const { execFileSync } = await import("node:child_process");
  execFileSync("powershell", [
    "-NoProfile",
    "-Command",
    "Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.SendKeys]::SendWait('^%+{ESC}')",
  ]);
  await sleep(1500);

  await evaluate(`document.querySelectorAll('nav .nav')[2].click()`);
  await sleep(600);
  const logs = await evaluate(`document.querySelector('.console')?.textContent ?? ''`);
  const hits = String(logs)
    .split(String.fromCharCode(10))
    .filter((line) => line.includes("熱鍵") || line.includes("放開"));
  console.log("hotkey log lines:", JSON.stringify(hits));

  await evaluate(`document.querySelectorAll('nav .nav')[0].click()`);
  await sleep(300);
  await evaluate(`Array.from(document.querySelectorAll('button')).find(b => b.textContent.trim() === '中斷')?.click()`);
  await sleep(1000);
} else if (script === "features") {
  // Sessions table + CSV export + hotkey registration, all through the real IPC.
  await evaluate(`document.querySelectorAll('nav .nav')[0].click()`);
  await sleep(300);
  await evaluate(`Array.from(document.querySelectorAll('button')).find(b => b.textContent.includes('重新掃描')).click()`);
  await sleep(1200);
  await evaluate(`Array.from(document.querySelectorAll('button')).find(b => b.textContent.trim() === '連線').click()`);
  await sleep(3000);

  await evaluate(`document.querySelectorAll('nav .nav')[1].click()`);
  await sleep(2000);
  const sessions = await evaluate(`(() => {
    const rows = Array.from(document.querySelectorAll('.panel'))
      .find(p => p.textContent.includes('SESSION_LIST'))
      ?.querySelectorAll('tbody tr');
    return rows ? Array.from(rows).map(r => Array.from(r.children).map(c => c.textContent)) : [];
  })()`);
  console.log("sessions:", JSON.stringify(sessions));

  const csv = await evaluate(`(async () => {
    const report = {
      config: { rounds: 1, mode: "Normal" },
      device: { path: "test", product: "GDBOX KB8", serial: "TEST", vid: 4609, pid: 2827 },
      unix_ms: Date.now(),
      rounds: [{ round: 1, latency_ms: 1.5, outcome: "Success", error: "" }],
      cancelled: false,
      cleanup_confirmed: true,
      terminal_error: null,
    };
    return await window.__TAURI_INTERNALS__.invoke("export_report_csv", { report });
  })()`);
  console.log("csv:", csv);

  const info = await evaluate(`window.__TAURI_INTERNALS__.invoke("app_info")`);
  console.log("hotkey registered:", info.hotkey);

  await evaluate(`document.querySelectorAll('nav .nav')[2].click()`);
  await sleep(600);
  const logs = await evaluate(`document.querySelector('.console')?.textContent ?? ''`);


  await evaluate(`document.querySelectorAll('nav .nav')[0].click()`);
  await sleep(300);
  await evaluate(`Array.from(document.querySelectorAll('button')).find(b => b.textContent.trim() === '中斷')?.click()`);
  await sleep(1000);
} else if (script === "hardware") {
  // Real end-to-end against the board: scan, connect, then read the live page.
  await evaluate(`document.querySelectorAll('nav .nav')[0].click()`);
  await sleep(300);
  await evaluate(`Array.from(document.querySelectorAll('button')).find(b => b.textContent.includes('重新掃描')).click()`);
  await sleep(1500);
  const rows = await evaluate(`document.querySelectorAll('.table tbody tr').length`);
  console.log("device rows:", rows);
  if (!rows) {
    console.log("no device found; is the board plugged in?");
    process.exit(0);
  }
  await evaluate(`Array.from(document.querySelectorAll('button')).find(b => b.textContent.trim() === '連線').click()`);
  await sleep(2500);
  const connected = await evaluate(`document.body.innerText.includes('已連線')`);
  console.log("connected:", connected);

  await evaluate(`document.querySelectorAll('nav .nav')[1].click()`);
  await sleep(2500);
  const live = await evaluate(`(() => {
    const metrics = Array.from(document.querySelectorAll('.metric')).map(m => ({
      label: m.querySelector('.metric-head span')?.textContent,
      value: m.querySelector('.metric-value')?.textContent,
    }));
    const kv = Array.from(document.querySelectorAll('.kv dt')).map(dt => dt.textContent);
    return {
      metricCount: metrics.length,
      metrics: metrics.slice(0, 8),
      diagnostics: kv,
      hasDiagnostics: document.body.innerText.includes('STATUS_EXT'),
      undefinedCount: (document.body.innerText.match(/undefined/g) || []).length,
      rawHead: (document.querySelector('.console')?.textContent ?? '').slice(0, 80),
    };
  })()`);
  console.log(JSON.stringify(live, null, 2));

  // Disconnect cleanly so the board is free for the probe.
  await evaluate(`document.querySelectorAll('nav .nav')[0].click()`);
  await sleep(300);
  await evaluate(`Array.from(document.querySelectorAll('button')).find(b => b.textContent.trim() === '中斷')?.click()`);
  await sleep(1200);
  const after = await evaluate(`document.body.innerText.includes('未連線')`);
  console.log("disconnected:", after);
} else if (script === "pages") {
  const names = await evaluate(`Array.from(document.querySelectorAll('nav .nav')).map(b => b.textContent)`);
  console.log("nav:", JSON.stringify(names));
  const pages = Array.isArray(names) ? names : [];
  for (let index = 0; index < pages.length; index += 1) {
    await evaluate(`document.querySelectorAll('nav .nav')[${index}].click()`);
    await new Promise((resolve) => setTimeout(resolve, 350));
    const info = await evaluate(`(() => {
      const text = document.body.innerText;
      return {
        page: ${JSON.stringify(pages[index])},
        chars: text.length,
        undefinedCount: (text.match(/undefined/g) || []).length,
        nanCount: (text.match(/NaN/g) || []).length,
        panels: document.querySelectorAll('.panel').length,
        metrics: document.querySelectorAll('.metric').length,
        head: text.slice(0, 90).replace(/\\n/g, ' | '),
      };
    })()`);
    console.log(JSON.stringify(info));
  }
} else {
  const info = await evaluate(`(() => {
    const text = document.body.innerText;
    return {
      title: document.title,
      url: location.href,
      chars: text.length,
      undefinedCount: (text.match(/undefined/g) || []).length,
      nanCount: (text.match(/NaN/g) || []).length,
      internals: typeof window.__TAURI_INTERNALS__,
      legacyGlobal: typeof window.__TAURI__,
      nav: Array.from(document.querySelectorAll('nav .nav')).map(b => b.textContent),
      firstLine: text.split('\\n').slice(0, 6).join(' | '),
    };
  })()`);
  console.log(JSON.stringify(info, null, 2));
}
if (logs.length) console.log("console:", JSON.stringify(logs.slice(-15), null, 2));
ws.close();
