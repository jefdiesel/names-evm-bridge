#!/usr/bin/env node
// DEV ONLY. Screenshots of harness scenarios through the Chrome DevTools Protocol, with real
// device emulation (plain `--headless --window-size` cannot go below ~500 px wide).
// Also runs page-side probes (`--eval`), e.g. decoding the page's QR with BarcodeDetector.
//
//   node dev/shoot.mjs --base http://127.0.0.1:8731 --out <dir> [--scheme light|dark] scenario...
//   node dev/shoot.mjs --base ... --eval-file probe.js --url <page>     (prints the result)
//
// Uses the installed Google Chrome with a throwaway profile; downloads nothing.

import { spawn } from 'node:child_process';
import { mkdtempSync, writeFileSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const args = process.argv.slice(2);
const opt = (k, d) => {
  const i = args.indexOf(k);
  if (i === -1) return d;
  const v = args[i + 1];
  args.splice(i, 2);
  return v;
};
const base = opt('--base', 'http://127.0.0.1:8731');
const out = opt('--out', '.');
const schemes = opt('--scheme', 'light,dark').split(',');
const widths = opt('--widths', '390,1280').split(',').map(Number);
const evalFile = opt('--eval-file', null);
const evalUrl = opt('--url', null);
const wait = Number(opt('--wait', '2500'));
const clipH = Number(opt('--clip-h', '4000'));
const setStorage = opt('--set-storage', null); // k=v, set in localStorage before loading --url
const shotName = opt('--name', 'page');
const vh = Number(opt('--vh', '0')); // desktop viewport height (default 900)
const scenarios = args;

const CHROME = process.env.CHROME || '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const profile = mkdtempSync(join(process.env.TMPDIR || tmpdir(), 'shoot-'));
const port = 9300 + Math.floor(Math.random() * 500);
const chrome = spawn(CHROME, ['--headless=new', `--remote-debugging-port=${port}`, '--remote-debugging-address=127.0.0.1', `--user-data-dir=${profile}`, '--no-first-run', '--no-default-browser-check', '--hide-scrollbars', 'about:blank'], { stdio: 'ignore' });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let wsUrl;
for (let i = 0; i < 50 && !wsUrl; i++) {
  await sleep(200);
  try {
    const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
    wsUrl = list.find((t) => t.type === 'page')?.webSocketDebuggerUrl;
  } catch {
    /* not up yet */
  }
}
if (!wsUrl) throw new Error('Chrome did not start');

const ws = new WebSocket(wsUrl);
await new Promise((r) => (ws.onopen = r));
let id = 0;
const pending = new Map();
const consoleLines = [];
ws.onmessage = (m) => {
  const msg = JSON.parse(m.data);
  if (msg.id && pending.has(msg.id)) {
    pending.get(msg.id)(msg);
    pending.delete(msg.id);
  } else if (msg.method === 'Runtime.consoleAPICalled' || msg.method === 'Runtime.exceptionThrown' || msg.method === 'Log.entryAdded') {
    consoleLines.push(JSON.stringify(msg.params).slice(0, 400));
  }
};
const cmd = (method, params = {}) =>
  new Promise((resolve, reject) => {
    const n = ++id;
    pending.set(n, (msg) => (msg.error ? reject(new Error(`${method}: ${msg.error.message}`)) : resolve(msg.result)));
    ws.send(JSON.stringify({ id: n, method, params }));
  });

await cmd('Page.enable');
await cmd('Runtime.enable');
await cmd('Log.enable');

async function load(url, width, scheme) {
  const mobile = width < 600;
  await cmd('Emulation.setDeviceMetricsOverride', { width, height: mobile ? 844 : vh || 900, deviceScaleFactor: mobile ? 2 : 1, mobile });
  await cmd('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: scheme }] });
  await cmd('Page.navigate', { url });
  for (let i = 0; i < 100; i++) {
    await sleep(100);
    const r = await cmd('Runtime.evaluate', { expression: 'document.documentElement.dataset.harnessDone || (document.readyState === "complete" && !location.pathname.includes("harness") ? "1" : "")', returnByValue: true });
    if (r.result.value) break;
  }
  await sleep(wait);
}

if (evalUrl && !evalFile) {
  if (setStorage) {
    await load(evalUrl.split('#')[0], 1280, 'light');
    const [k, v] = setStorage.split('=');
    await cmd('Runtime.evaluate', { expression: `localStorage.setItem(${JSON.stringify(k)}, ${JSON.stringify(v)})` });
  }
  for (const width of widths) {
    for (const scheme of schemes) {
      await cmd('Page.navigate', { url: 'about:blank' });
      await load(evalUrl, width, scheme);
      const m = await cmd('Page.getLayoutMetrics');
      const h = Math.ceil(m.cssContentSize.height);
      const shot = await cmd('Page.captureScreenshot', { format: 'png', captureBeyondViewport: true, clip: { x: 0, y: 0, width, height: Math.min(h, clipH), scale: 1 } });
      const file = join(out, `${shotName}-${width}-${scheme}.png`);
      writeFileSync(file, Buffer.from(shot.data, 'base64'));
      const txt = (await cmd('Runtime.evaluate', { expression: '[...document.querySelectorAll(".checks li")].map(l => l.textContent).join("\\n")', returnByValue: true })).result.value;
      console.log(file + '\n' + txt);
    }
  }
} else if (evalFile) {
  await load(evalUrl, 1280, 'light');
  const r = await cmd('Runtime.evaluate', { expression: readFileSync(evalFile, 'utf8'), awaitPromise: true, returnByValue: true });
  console.log(JSON.stringify(r.exceptionDetails ? r.exceptionDetails.exception?.description || r.exceptionDetails : r.result.value));
} else {
  for (const sc of scenarios) {
    for (const width of widths) {
      for (const scheme of schemes) {
        await load(`${base}/dev/harness.html?s=${sc}`, width, scheme);
        const sw = (await cmd('Runtime.evaluate', { expression: 'document.documentElement.scrollWidth', returnByValue: true })).result.value;
        const m = await cmd('Page.getLayoutMetrics');
        const h = Math.ceil(m.cssContentSize.height);
        const w = Math.max(Math.ceil(m.cssContentSize.width), sw);
        const shot = await cmd('Page.captureScreenshot', { format: 'png', captureBeyondViewport: true, clip: { x: 0, y: 0, width, height: Math.min(h, clipH), scale: 1 } });
        const file = join(out, `${sc}-${width}-${scheme}.png`);
        writeFileSync(file, Buffer.from(shot.data, 'base64'));
        const head = (await cmd('Runtime.evaluate', { expression: '(document.getElementById("action-h")||{}).textContent || ""', returnByValue: true })).result.value;
        // the words a person sees on the card (closed <details> and hidden parts excluded)
        const words = (await cmd('Runtime.evaluate', { expression: '((document.getElementById("card")||{}).innerText||"").split(/\\s+/).filter(Boolean).length', returnByValue: true })).result.value;
        const state = (await cmd('Runtime.evaluate', { expression: '[(document.getElementById("card-h")||{}).textContent, (document.getElementById("step-h")||{}).textContent, document.getElementById("acct") ? "ACCOUNT FIELD" : "no account field", location.hash.slice(0, 20)].join(" | ")', returnByValue: true })).result.value;
        console.log(`${file}  content ${w}x${h}${w > width ? '  OVERFLOWS HORIZONTALLY' : ''}  words ${words}  [${head}]  ${state}`);
      }
    }
  }
}
if (consoleLines.length) console.log('console:\n' + consoleLines.join('\n'));
ws.close();
chrome.kill();
await sleep(300);
rmSync(profile, { recursive: true, force: true });
