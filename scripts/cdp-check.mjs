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

if (script === "hardware") {
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
