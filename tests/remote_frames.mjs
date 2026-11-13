// How the remote picture feels, measured in a real (headless) Chrome. Run
// by `headless_remote_frames_in_chrome`:
//
//   node tests/remote_frames.mjs <port> <password> <shots-dir> <chrome>
//
// Signs in, then measures at a retina laptop's size (2x): a key press to
// its frame arriving (and drawn), frames per second and bytes per frame
// while typing fast, scrolling and streaming a terminal, and frames sent
// while idle. Prints "METRIC <json>" lines, then "OK".
import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync, writeFileSync, existsSync, createWriteStream } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const [port, password, shots, chrome] = process.argv.slice(2);
let step = "starting";
const fail = msg => { console.error("FAIL " + msg); process.exit(1); };
const sleep = ms => new Promise(r => setTimeout(r, ms));

const profile = mkdtempSync(join(tmpdir(), "rox-chrome-"));
const browser = spawn(chrome, [
  // WebGL in software (the page draws with it): works with no GPU (CI).
  "--headless=new", "--use-mock-keychain", "--password-store=basic", "--use-angle=swiftshader", "--enable-unsafe-swiftshader",
  "--no-first-run", "--no-default-browser-check",
  "--ignore-certificate-errors", "--no-proxy-server", "--enable-logging=stderr",
  `--user-data-dir=${profile}`, "--remote-debugging-port=0", "--window-size=1200,800", "about:blank",
], { stdio: ["ignore", "ignore", "pipe"] });
browser.stderr.pipe(createWriteStream(join(shots, "chrome-frames-stderr.log")));
process.on("exit", () => browser.kill());

let devtools;
// No deadline (a hang is a bug the test found): until Chrome says, or quits.
while (!devtools && browser.exitCode === null) {
  const f = join(profile, "DevToolsActivePort");
  if (existsSync(f)) devtools = readFileSync(f, "utf8").split("\n")[0];
  else await sleep(100);
}
if (!devtools) fail("Chrome didn't start");
const targets = await (await fetch(`http://127.0.0.1:${devtools}/json/list`)).json();
const ws = new WebSocket(targets.find(t => t.type === "page").webSocketDebuggerUrl);
await new Promise((ok, err) => { ws.onopen = ok; ws.onerror = err; });
let nextId = 1;
const pending = new Map();
ws.onmessage = e => {
  const m = JSON.parse(e.data);
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); }
};
const cdp = (method, params = {}) => new Promise(ok => {
  const id = nextId++;
  pending.set(id, ok);
  ws.send(JSON.stringify({ id, method, params }));
});
const evaluate = async expr => (await cdp("Runtime.evaluate", { expression: expr, returnByValue: true, awaitPromise: true })).result?.result?.value;
const waitFor = async (expr, what) => {
  step = what;
  while (!(await evaluate(expr))) await sleep(100);
};
const key = async (ch, code, modifiers = 0, text = ch) => {
  await cdp("Input.dispatchKeyEvent", { type: "keyDown", key: ch, code, text, modifiers });
  await cdp("Input.dispatchKeyEvent", { type: "keyUp", key: ch, code, modifiers });
};
const typeLetter = ch => key(ch, "Key" + ch.toUpperCase());
async function typeLine(s) {
  for (const ch of s) {
    const code = /[a-z]/i.test(ch) ? "Key" + ch.toUpperCase() : /[0-9]/.test(ch) ? "Digit" + ch : ch === " " ? "Space" : ch;
    await key(ch, code);
  }
  await key("Enter", "Enter", 0, "\r");
}

ws.onclose = () => fail("DevTools closed at: " + step);
await cdp("Page.enable");
await cdp("Page.navigate", { url: `https://localhost:${port}/` });
await waitFor(`!!document.querySelector('input[type=password]')`, "the sign-in page");
await evaluate(`(() => { const i = document.querySelector('input[type=password]'); i.value = ${JSON.stringify(password)}; i.form.submit(); })()`);
await waitFor(`!!document.getElementById('screen')`, "the app page");
await waitFor(`document.getElementById('dot').classList.contains('live')`, "a live link");
await cdp("Emulation.setFocusEmulationEnabled", { enabled: true });
await cdp("Emulation.setDeviceMetricsOverride", { width: 1200, height: 800, deviceScaleFactor: 2, mobile: false });
await waitFor(`document.getElementById('screen').width > 1500`, "a 2x frame on the canvas");
await sleep(1500);

// Watch the link: every frame's arrival and size; the time from a key press
// to the first frame after it (arrived, and drawn when the page says so).
await evaluate(`(() => {
  const m = window.__m = { keyAt: 0, arrive: false, draw: false, lat: [], drawLat: [], frames: [] };
  addEventListener("keydown", () => { m.keyAt = performance.now(); m.arrive = m.draw = true; }, true);
  ws.addEventListener("message", e => {
    if (typeof e.data === "string") return;
    const t = performance.now();
    m.frames.push([t, e.data.size ?? e.data.byteLength]);
    if (m.arrive) { m.arrive = false; m.lat.push(t - m.keyAt); }
  });
  addEventListener("rox-frame", () => { if (m.draw) { m.draw = false; m.drawLat.push(performance.now() - m.keyAt); } });
})()`);
await evaluate(`document.getElementById('screen').focus()`);
const pct = (a, p) => { const s = [...a].sort((x, y) => x - y); return s.length ? s[Math.min(s.length - 1, Math.floor(p * s.length))] : null; };
const round = x => x == null ? null : Math.round(x * 10) / 10;
// Frames between two marks: per second, and bytes each.
async function window_(name, run, ms) {
  const t0 = await evaluate(`performance.now()`);
  await run();
  if (ms) await sleep(ms);
  const [t1, frames] = await evaluate(`[performance.now(), __m.frames]`);
  const got = frames.filter(([t]) => t >= t0 && t <= t1);
  const bytes = got.map(f => f[1]);
  const out = { name, secs: round((t1 - t0) / 1000), frames: got.length, fps: round(got.length / ((t1 - t0) / 1000)),
    kbPerFrame: round(bytes.reduce((a, b) => a + b, 0) / Math.max(1, got.length) / 1024),
    kbMax: round(Math.max(0, ...bytes) / 1024), mbps: round(bytes.reduce((a, b) => a + b, 0) * 8 / ((t1 - t0) / 1000) / 1e6) };
  console.log("METRIC " + JSON.stringify(out));
}

// 1. Key to frame, one key at a time.
step = "latency";
for (let i = 0; i < 20; i++) { await typeLetter("abcdefghij"[i % 10]); await sleep(250); }
const [lat, drawLat] = await evaluate(`[__m.lat, __m.drawLat]`);
console.log("METRIC " + JSON.stringify({ name: "key-to-frame", n: lat.length, arrivedMs: { p50: round(pct(lat, 0.5)), p90: round(pct(lat, 0.9)), min: round(Math.min(...lat)) },
  drawnMs: drawLat.length ? { p50: round(pct(drawLat, 0.5)), p90: round(pct(drawLat, 0.9)) } : null }));

// 2. Typing fast: a key every 30 ms.
step = "typing";
await window_("typing", async () => { for (let i = 0; i < 50; i++) { await typeLetter("klmnop"[i % 6]); await sleep(30); } }, 300);
// 3. Idle: the caret blinks, nothing else.
step = "idle";
await sleep(1500);
await window_("idle", async () => {}, 3000);
// 4. Scrolling: a wheel tick every 16 ms over the editor.
step = "scrolling";
const [cx, cy] = await evaluate(`(() => { const r = document.getElementById('screen').getBoundingClientRect(); return [r.left + r.width * 0.6, r.top + r.height * 0.5]; })()`);
await cdp("Input.dispatchMouseEvent", { type: "mouseMoved", x: cx, y: cy });
await window_("scrolling", async () => {
  for (let i = 0; i < 120; i++) { await cdp("Input.dispatchMouseEvent", { type: "mouseWheel", x: cx, y: cy, deltaX: 0, deltaY: 40 }); await sleep(16); }
}, 300);
// What the page drew from frames of changes is exactly what a new link,
// starting from every texture whole, draws (all but the blinking caret).
step = "exactness";
await sleep(800);
const exact = await evaluate(`(async () => {
  const c = document.getElementById('screen');
  const pixels = () => { const t = document.createElement('canvas'); t.width = c.width; t.height = c.height;
    const x = t.getContext('2d'); x.drawImage(c, 0, 0); return x.getImageData(0, 0, t.width, t.height).data; };
  const before = pixels();
  const seen = __roxStats.frames;
  ws.close();
  while (__roxStats.frames === seen) await new Promise(r => setTimeout(r, 50));
  await new Promise(r => setTimeout(r, 300));
  const after = pixels();
  let differ = 0;
  for (let i = 0; i < after.length; i += 4) if (after[i] !== before[i] || after[i + 1] !== before[i + 1] || after[i + 2] !== before[i + 2]) differ++;
  return { pixels: after.length / 4, differ, reconnected: __roxStats.frames > seen };
})()`);
console.log("METRIC " + JSON.stringify({ name: "exactness", ...exact }));
await evaluate(`(() => { const m = window.__m; ws.addEventListener("message", e => { if (typeof e.data !== "string") m.frames.push([performance.now(), e.data.byteLength]); }); })()`);
writeFileSync(join(shots, "f1-scrolled.png"), Buffer.from((await cdp("Page.captureScreenshot", { format: "png" })).result.data, "base64"));
// 5. A terminal streaming output.
step = "terminal";
await key("†", "KeyT", 1);            // ⌥T: the terminal
await sleep(2500);
await typeLine("for i in $(seq 1 400000); do echo line $i $RANDOM $RANDOM; done");
await sleep(500);
await window_("terminal", async () => {}, 3000);
writeFileSync(join(shots, "f2-terminal.png"), Buffer.from((await cdp("Page.captureScreenshot", { format: "png" })).result.data, "base64"));
await key("c", "KeyC", 2);            // ⌃C
await sleep(500);
const stats = await evaluate(`window.__roxStats ? JSON.stringify(window.__roxStats) : null`);
if (stats) console.log("METRIC " + JSON.stringify({ name: "page", ...JSON.parse(stats) }));
console.log("OK");
process.exit(0);
