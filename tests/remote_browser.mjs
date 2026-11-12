// End-to-end check of the remote page in a real (headless) Chrome, driven
// over the DevTools protocol. Run by `headless_remote_in_chrome`:
//
//   node tests/remote_browser.mjs <port> <password> <shots-dir> <chrome> [input]
//
// Signs in through the real form, waits for the window to be drawn on the
// canvas, screenshots it, and types through Chrome's real keyboard path.
// Prints "OK ..." on success; exits non-zero with the reason otherwise.
//
// With "input" (`headless_remote_input_in_chrome`), it instead runs real
// key, mouse, wheel and touch sequences, printing "STEP <name>" after each
// so the test can look at the window's state right then, and "CLIP<n>
// <json>" for what reached this browser's clipboard.
import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const [port, password, shots, chrome, mode] = process.argv.slice(2);
let step = "starting";
const fail = msg => { console.error("FAIL " + msg); process.exit(1); };
const sleep = ms => new Promise(r => setTimeout(r, ms));
setTimeout(() => fail("timed out at: " + step), 180000).unref();

const profile = mkdtempSync(join(tmpdir(), "rox-chrome-"));
const browser = spawn(chrome, [
  "--headless=new", "--use-mock-keychain", "--password-store=basic", "--disable-gpu", "--no-first-run", "--no-default-browser-check",
  "--ignore-certificate-errors", // the server's own self-signed certificate
  // A fresh profile otherwise waits on proxy auto-detection (~30s) before
  // its first request.
  "--no-proxy-server",
  "--enable-logging=stderr",
  `--user-data-dir=${profile}`, "--remote-debugging-port=0", "--window-size=1200,800", "about:blank",
], { stdio: ["ignore", "ignore", "pipe"] });
// Chrome's own log, for when something hangs.
import { createWriteStream } from "node:fs";
browser.stderr.pipe(createWriteStream(join(shots, "chrome-stderr.log")));
process.on("exit", () => browser.kill());

// The DevTools port, from the profile once Chrome is up.
let devtools;
step = "launching Chrome";
for (let i = 0; i < 100 && !devtools; i++) {
  const f = join(profile, "DevToolsActivePort");
  if (existsSync(f)) devtools = readFileSync(f, "utf8").split("\n")[0];
  else await sleep(100);
}
if (!devtools) fail("Chrome didn't start");
step = "connecting to Chrome";
const targets = await (await fetch(`http://127.0.0.1:${devtools}/json/list`)).json();
const page = targets.find(t => t.type === "page");
step = "the DevTools socket";
const ws = new WebSocket(page.webSocketDebuggerUrl);
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
const waitFor = async (expr, what, ms = 15000) => {
  step = what;
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await evaluate(expr)) return; await sleep(150); }
  fail("never: " + what);
};
const shot = async name => {
  const r = await cdp("Page.captureScreenshot", { format: "png" });
  writeFileSync(join(shots, name), Buffer.from(r.result.data, "base64"));
};

// ---- real input, end to end ----------------------------------------------

const MOD = { alt: 1, ctrl: 2, meta: 4, shift: 8 };
// A US Mac keyboard: the code for a character, and whether it needs ⇧.
function usKey(ch) {
  if (/^[a-z]$/.test(ch)) return ["Key" + ch.toUpperCase(), false];
  if (/^[A-Z]$/.test(ch)) return ["Key" + ch, true];
  if (/^[0-9]$/.test(ch)) return ["Digit" + ch, false];
  const table = { " ": ["Space", false], "$": ["Digit4", true], "(": ["Digit9", true], ")": ["Digit0", true],
                  "+": ["Equal", true], "-": ["Minus", false], "!": ["Digit1", true], ":": ["Semicolon", true],
                  "/": ["Slash", false], ".": ["Period", false] };
  return table[ch];
}
const keyEvent = (type, key, code, modifiers = 0, extra = {}) =>
  cdp("Input.dispatchKeyEvent", { type, key, code, modifiers, ...extra });
// A key on its own, with `modifiers` already held.
async function press(key, code, modifiers = 0, text, extra = {}) {
  await keyEvent(text ? "keyDown" : "rawKeyDown", key, code, modifiers, text ? { text, ...extra } : extra);
  await keyEvent("keyUp", key, code, modifiers);
}
const MOD_KEYS = { meta: ["Meta", "MetaLeft"], alt: ["Alt", "AltLeft"], ctrl: ["Control", "ControlLeft"], shift: ["Shift", "ShiftLeft"] };
// A chord as a person plays it: modifiers down one by one, the key, then up.
async function chord(mods, key, code, text) {
  let m = 0;
  for (const name of mods) { m |= MOD[name]; await keyEvent("rawKeyDown", ...MOD_KEYS[name], m); }
  await press(key, code, m, text);
  for (const name of [...mods].reverse()) { m &= ~MOD[name]; await keyEvent("keyUp", ...MOD_KEYS[name], m); }
}
async function type(text) {
  for (const ch of text) {
    const [code, shift] = usKey(ch);
    if (shift) await keyEvent("rawKeyDown", "Shift", "ShiftLeft", MOD.shift);
    await press(ch, code, shift ? MOD.shift : 0, ch);
    if (shift) await keyEvent("keyUp", "Shift", "ShiftLeft", 0);
  }
}
const mark = async name => { await sleep(350); console.log("STEP " + name); await sleep(350); };
const mouse = (type, x, y, extra = {}) => cdp("Input.dispatchMouseEvent", { type, x, y, ...extra });
async function click(x, y, { button = "left", clickCount = 1, modifiers = 0 } = {}) {
  const buttons = { left: 1, right: 2, middle: 4 }[button];
  await mouse("mouseMoved", x, y, { modifiers });
  await mouse("mousePressed", x, y, { button, buttons, clickCount, modifiers });
  await mouse("mouseReleased", x, y, { button, buttons: 0, clickCount, modifiers });
}
const touches = (type, points) => cdp("Input.dispatchTouchEvent", { type, touchPoints: points.map(([x, y], id) => ({ x, y, id })) });
const clipboard = () => evaluate(`navigator.clipboard.readText()`);

async function inputScenario() {
  step = "input: setup";
  // The page keeps focus as a person's would (clipboard needs it).
  await cdp("Emulation.setFocusEmulationEnabled", { enabled: true });
  const granted = await cdp("Browser.grantPermissions", { origin: `https://localhost:${port}`,
    permissions: ["clipboardReadWrite", "clipboardSanitizedWrite"] });
  if (granted.error) fail("clipboard permission: " + JSON.stringify(granted.error));
  await evaluate(`document.getElementById('screen').focus()`);
  const [left, top, aw, ah] = await evaluate(`(() => { const r = document.getElementById('screen').getBoundingClientRect();
    return [r.left, r.top, r.width, r.height]; })()`);
  // Window spots: the editor's first row, its right side, the file tree's empty bottom.
  const at = (x, y) => [left + x, top + y];
  const row0 = at(aw * 0.7, 32 + 36 + 26 + 6), lower = at(aw * 0.7, ah * 0.55), mid = at(aw * 0.6, ah * 0.45);
  const tree = at(60, 32 + 36 + 36 + 26 - 8); // below the tree's search box
  await mark("start");

  step = "input: typing";
  await chord(["shift"], "H", "KeyH", "H");      // ⇧H
  await type("i");
  await chord(["alt"], "Dead", "KeyE");          // ⌥E: a dead key, types nothing…
  await press("é", "KeyE", 0, "é");              // …then E types é
  await chord(["alt"], "å", "KeyA", "å");        // ⌥A isn't a shortcut: types å
  await type(" x");
  await press("Backspace", "Backspace");
  await mark("typed");

  step = "input: lines";
  await chord(["meta"], "ArrowRight", "ArrowRight");  // ⌘→: end of line
  await press("Enter", "Enter", 0, "\r");
  await press("Tab", "Tab");
  await type("t");
  // A held key repeats: one press, then repeats, then one release.
  await keyEvent("keyDown", "r", "KeyR", 0, { text: "r" });
  for (let i = 0; i < 2; i++) await keyEvent("keyDown", "r", "KeyR", 0, { text: "r", autoRepeat: true });
  await keyEvent("keyUp", "r", "KeyR");
  await mark("lines");

  step = "input: wheel";
  await mouse("mouseMoved", ...mid);
  for (let i = 0; i < 12; i++) { await mouse("mouseWheel", ...mid, { deltaX: 0, deltaY: 25 }); await sleep(16); }
  await mark("wheel-sent");
  // Where the first row is now shows how far it scrolled: click there, type W.
  await sleep(1200);
  await click(...row0);
  await type("W");
  await mark("wheeled");
  await chord(["meta"], "ArrowUp", "ArrowUp");       // ⌘↑: back to the top
  await mark("top");

  step = "input: triple-click + ⌘C";
  await sleep(500);
  for (const clickCount of [1, 2, 3]) { await click(...row0, { clickCount }); await sleep(80); }
  await chord(["meta"], "c", "KeyC");
  await sleep(600);
  console.log("CLIP1 " + JSON.stringify(await clipboard()));
  await mark("copied");

  step = "input: shift-click + ⌘X";
  // Separate clicks (more than egui's 0.3 s double-click time apart),
  // however bunched they arrive.
  await sleep(500);
  await click(...row0);                                // caret at the end of line 1
  await sleep(500);
  await click(...lower, { modifiers: MOD.shift });     // ⇧-click extends to here
  await chord(["meta"], "x", "KeyX");
  await sleep(600);
  console.log("CLIP2 " + JSON.stringify(await clipboard()));
  await mark("cut");

  step = "input: ⌘V";
  await evaluate(`navigator.clipboard.writeText("P!")`);
  await keyEvent("rawKeyDown", "Meta", "MetaLeft", MOD.meta);
  await keyEvent("rawKeyDown", "v", "KeyV", MOD.meta, { commands: ["paste"] });
  await keyEvent("keyUp", "v", "KeyV", MOD.meta);
  await keyEvent("keyUp", "Meta", "MetaLeft", 0);
  await mark("pasted");

  step = "input: focus lost with ⇧ held";
  await keyEvent("rawKeyDown", "Shift", "ShiftLeft", MOD.shift);
  await evaluate(`window.dispatchEvent(new Event("blur")); window.dispatchEvent(new Event("focus"))`);
  await press("k", "KeyK", 0, "k");                  // the ⇧ was let go while away
  await keyEvent("keyUp", "Shift", "ShiftLeft", 0);
  await mark("refocused");

  step = "input: pinch";
  await mouse("mouseWheel", ...mid, { deltaX: 0, deltaY: -40, modifiers: MOD.ctrl }); // a trackpad pinch
  await mark("pinched");

  step = "input: right-click";
  await click(...tree, { button: "right" });
  await sleep(300);
  await shot("w5a-menu.png");
  await mark("menu");
  await press("Escape", "Escape");
  await click(...row0);
  await mark("menu-closed");

  step = "input: terminal";
  await chord(["alt"], "†", "KeyT", "†");            // ⌥T opens the terminal, types nothing
  await sleep(1500);
  await type("sleep 30");
  await press("Enter", "Enter", 0, "\r");
  await sleep(800);
  await chord(["ctrl"], "c", "KeyC");                // ⌃C from a Mac: the shell's interrupt, not Copy
  await sleep(500);
  await type("echo done-$((1+1))");
  await press("Enter", "Enter", 0, "\r");
  await mark("terminal");
  // A link from the terminal opens in this browser, not on the Mac.
  step = "input: link";
  await evaluate(`window.open = u => { window.__opened = u; return {}; }`);
  await type("echo https://example.com/rox");
  await press("Enter", "Enter", 0, "\r");
  await sleep(500);
  await chord(["meta", "shift"], "o", "KeyO");       // ⇧⌘O: the terminal's links
  await sleep(300);
  await press("Enter", "Enter", 0, "\r");            // the link: opened
  await sleep(800);
  console.log("OPENED " + JSON.stringify(await evaluate(`window.__opened`)));
  await mark("link-opened");
  await chord(["meta"], "j", "KeyJ");                // ⌘J: back to the editor
  await mark("editor-again");
  await chord(["meta"], "p", "KeyP");                // ⌘P: Go to File, the tree's search (Chrome's Print otherwise)
  await mark("palette");
  await press("Escape", "Escape");
  await mark("palette-closed");
  await shot("w5-input-desktop.png");

  // The window list: Switch Window's key (shown on the name) opens it; while
  // it's up its keys are its own, and esc closes it. The name's button too.
  step = "input: window list";
  const wins = `!document.getElementById('wins').hidden`;
  const winKey = await evaluate(`document.getElementById('winKey').textContent`);
  if (!winKey.includes("S")) fail("the name should show Switch Window's key: " + winKey);
  await chord(["meta", "alt", "shift"], "s", "KeyS");
  await waitFor(wins, "the window list from its key");
  await press("ArrowDown", "ArrowDown");
  await shot("w5b-windows.png");
  await press("Escape", "Escape");
  await waitFor(`!(${wins})`, "esc closing the window list");
  await evaluate(`document.getElementById('win').click()`);
  await waitFor(wins, "the window list from the name");
  await click(...mid);
  await waitFor(`!(${wins})`, "a click elsewhere closing it");
  await mark("windows");
  await chord(["meta"], "j", "KeyJ");                // ⌘J into the terminal, ⌥T hides it
  await chord(["alt"], "†", "KeyT", "†");

  // ⛶ (a real click): full screen with every key captured (Keyboard Lock).
  step = "input: full screen";
  // Headless Chrome 157+ grants full screen only with this (a real
  // browser asks nothing): the click below still has to work.
  await cdp("Browser.setPermission", {
    permission: { name: "fullscreen", allowWithoutGesture: true }, setting: "granted", origin: await evaluate("location.origin"),
  });
  const full = await evaluate(`(() => { const r = document.getElementById('full').getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2]; })()`);
  await click(...full);
  await sleep(800);
  const lock = await evaluate(`(async () => [!!document.fullscreenElement, document.body.classList.contains('locked'),
    await navigator.keyboard.lock().then(() => 'lock ok', e => e.name)])()`);
  console.log("LOCK " + JSON.stringify(lock));
  if (!(lock[0] && lock[1] && lock[2] === "lock ok")) fail("full screen should lock the keyboard: " + JSON.stringify(lock));
  await evaluate(`document.exitFullscreen()`);
  await sleep(800);

  // ---- touch ----
  step = "input: touch";
  await cdp("Emulation.setTouchEmulationEnabled", { enabled: true, maxTouchPoints: 5 });
  await chord(["meta"], "ArrowUp", "ArrowUp");
  await mark("touch-top");
  // One finger drags up slowly and stops before lifting (no coasting),
  // then flicks the same distance (coasting on): a tap on the first row and
  // a letter typed there show how far each went.
  const [fx, fy] = mid;
  const tapType = async ch => {
    await touches("touchStart", [row0]);
    await touches("touchEnd", []);
    await sleep(300);
    await type(ch);
  };
  await touches("touchStart", [[fx, fy + 120]]);
  for (let i = 1; i <= 6; i++) { await touches("touchMove", [[fx, fy + 120 - i * 40]]); await sleep(60); }

  await sleep(300);
  await touches("touchEnd", []);
  await sleep(1000);
  await tapType("A");
  await mark("dragged");
  await chord(["meta"], "ArrowUp", "ArrowUp");
  await sleep(300);
  await touches("touchStart", [[fx, fy + 120]]);
  for (let i = 1; i <= 6; i++) { await touches("touchMove", [[fx, fy + 120 - i * 40]]); await sleep(8); }
  await touches("touchEnd", []);
  await sleep(2000);
  await tapType("F");
  await mark("flicked");
  await shot("w6a-flicked.png");
  // A tap clicks: the caret goes to the end of line 1.
  await chord(["meta"], "ArrowUp", "ArrowUp");
  await sleep(300);
  await touches("touchStart", [row0]);
  await touches("touchEnd", []);
  await sleep(300);
  await type("T");
  await mark("tapped");
  // A long press right-clicks: the file tree's menu.
  await touches("touchStart", [tree]);
  await sleep(800);
  await touches("touchEnd", []);
  await mark("long-pressed");
  await press("Escape", "Escape");
  // Two fingers pinching zoom the page, not the app: nothing clicks or
  // scrolls there, and once back to normal size a tap still works.
  await touches("touchStart", [[fx - 40, fy], [fx + 40, fy]]);
  for (let i = 1; i <= 4; i++) await touches("touchMove", [[fx - 40 - i * 20, fy], [fx + 40 + i * 20, fy]]);
  await touches("touchEnd", []);
  await sleep(500);
  const scale = await evaluate(`visualViewport.scale`);
  if (!(scale > 1)) fail("a two-finger pinch should zoom the page, scale " + scale);
  await mark("page-pinched");
  await cdp("Emulation.setPageScaleFactor", { pageScaleFactor: 1 });
  await sleep(300);
  await touches("touchStart", [row0]);
  await touches("touchEnd", []);
  await sleep(300);
  await type("Z");
  await mark("after-pinch");
  await shot("w6-input-touch.png");

  // ---- a phone's on-screen keyboard ----
  step = "input: phone keyboard";
  await evaluate(`document.getElementById('screen').focus()`);
  await chord(["meta"], "ArrowUp", "ArrowUp");
  await chord(["meta"], "ArrowRight", "ArrowRight");
  await evaluate(`document.getElementById('kbd').click()`);   // ⌨: the keyboard comes up
  await sleep(200);
  if (!await evaluate(`document.activeElement === document.getElementById('kb')`)) fail("⌨ should focus the keyboard's field");
  await cdp("Input.insertText", { text: "hello" });
  // An input method composes, then commits something else.
  await cdp("Input.imeSetComposition", { text: "に", selectionStart: 1, selectionEnd: 1 });
  await cdp("Input.imeSetComposition", { text: "にほ", selectionStart: 2, selectionEnd: 2 });
  await sleep(100);
  await cdp("Input.insertText", { text: "日本" });
  await cdp("Input.insertText", { text: "teh " });
  // Autocorrect: the keyboard replaces the word it just typed.
  await evaluate(`(() => { const kb = document.getElementById('kb'), n = kb.value.length; kb.setSelectionRange(n - 4, n); })()`);
  await cdp("Input.insertText", { text: "the " });
  // Backspace as the field's own delete (Android sends no real key).
  const backspace = async () => {
    await keyEvent("rawKeyDown", "Backspace", "Backspace", 0, { commands: ["deleteBackward"] });
    await keyEvent("keyUp", "Backspace", "Backspace");
  };
  await backspace();
  await backspace();
  // A new line, then Backspace in the now empty field joins it back.
  await cdp("Input.insertText", { text: "\n" });
  await backspace();
  await mark("soft-keyboard");
  await evaluate(`document.getElementById('kbd').click()`);   // keyboard away

  // ---- a phone: an edge swipe brings out the file tree ----
  step = "input: edge swipe";
  await cdp("Emulation.setDeviceMetricsOverride", { width: 390, height: 844, deviceScaleFactor: 2, mobile: true });
  await waitFor(`(() => { const c = document.getElementById('screen'), a = document.getElementById('area').getBoundingClientRect();
    return a.width < 400 && Math.abs(c.width - Math.floor(a.width) * devicePixelRatio) <= 2; })()`, "a frame sized for the phone");
  await sleep(600);
  const [pl, pt] = await evaluate(`(() => { const r = document.getElementById('screen').getBoundingClientRect(); return [r.left, r.top]; })()`);
  await touches("touchStart", [[pl + 4, pt + 300]]);
  for (let i = 1; i <= 6; i++) { await touches("touchMove", [[pl + 4 + i * 25, pt + 300 + i]]); await sleep(16); }
  await touches("touchEnd", []);
  await mark("swiped");
  await shot("w7-phone-drawer.png");
  // A tap beside it puts it away; the terminal's header and the tabs are
  // finger-sized.
  await touches("touchStart", [[pl + 375, pt + 400]]);
  await touches("touchEnd", []);
  await sleep(300);
  await evaluate(`document.getElementById('screen').focus()`);
  await chord(["alt"], "†", "KeyT", "†");
  await sleep(1500);
  await shot("w8-phone-touch.png");
}

step = "Page.enable";
await cdp("Page.enable");
step = "opening the page";
await cdp("Page.navigate", { url: `https://localhost:${port}/` });
await waitFor(`!!document.querySelector('input[type=password]')`, "the sign-in page");
await shot("w0-sign-in.png");

// A wrong password first: told so, still signed out.
await evaluate(`(() => { const i = document.querySelector('input[type=password]'); i.value = 'nope'; i.form.submit(); })()`);
await waitFor(`location.search.includes('wrong') && document.body.innerText.includes("didn't work")`, "the wrong-password message");

await evaluate(`(() => { const i = document.querySelector('input[type=password]'); i.value = ${JSON.stringify(password)}; i.form.submit(); })()`);
await waitFor(`!!document.getElementById('screen')`, "the app page after signing in");
await waitFor(`document.querySelectorAll('.win').length >= 1`, "the window list");
await waitFor(`document.getElementById('dot').classList.contains('live')`, "a live link");
// Drawn: the canvas has a frame (sized by the server, not the 300x150 default).
await waitFor(`(() => { const c = document.getElementById('screen'); return c.width > 400 && c.height > 300; })()`, "a frame on the canvas");
await sleep(500);
const sizes = await evaluate(`(() => { const c = document.getElementById('screen'), a = document.getElementById('area').getBoundingClientRect();
  return [c.width, c.height, Math.floor(a.width), Math.floor(a.height), devicePixelRatio]; })()`);
const [cw, ch, aw, ah, dpr] = sizes;
if (Math.abs(cw - aw * dpr) > 2 || Math.abs(ch - ah * dpr) > 2) fail(`frame ${cw}x${ch} doesn't fit the page area ${aw}x${ah}@${dpr}`);
const listed = await evaluate(`[...document.querySelectorAll('.win b')].map(b => b.textContent).join(',')`);
await shot("w1-remote-page.png");
if (mode === "input") {
  await inputScenario();
  console.log(`OK listed=${listed} frame=${cw}x${ch}`);
  process.exit(0);
}

// Type through Chrome's real keyboard path, into the window's editor.
await evaluate(`document.getElementById('screen').focus()`);
for (const ch of "zq") {
  await cdp("Input.dispatchKeyEvent", { type: "keyDown", key: ch, code: "Key" + ch.toUpperCase(), text: ch });
  await cdp("Input.dispatchKeyEvent", { type: "keyUp", key: ch, code: "Key" + ch.toUpperCase() });
}
await sleep(800);
await shot("w2-typed.png");

// ⌥T as a Mac keyboard sends it: key T with Option held, typing "†". It
// should open the terminal and type nothing (checked by the test after).
step = "⌥T";
await cdp("Input.dispatchKeyEvent", { type: "keyDown", modifiers: 1, key: "†", code: "KeyT", text: "†", windowsVirtualKeyCode: 84 });
await cdp("Input.dispatchKeyEvent", { type: "keyUp", modifiers: 1, key: "†", code: "KeyT", windowsVirtualKeyCode: 84 });
await sleep(1500);
await shot("w2b-alt-t.png");

// A phone: the window is drawn for the phone's screen (resized, at its
// pixel ratio); the window list is a sheet across it.
await cdp("Emulation.setDeviceMetricsOverride", { width: 390, height: 844, deviceScaleFactor: 3, mobile: true });
await cdp("Emulation.setTouchEmulationEnabled", { enabled: true });
await waitFor(`(() => { const c = document.getElementById('screen'), a = document.getElementById('area').getBoundingClientRect();
  return Math.abs(c.width - Math.floor(a.width) * devicePixelRatio) <= 2 && a.width < 400; })()`, "a frame sized for the phone");
await sleep(400);
await shot("w3-phone.png");
await evaluate(`document.getElementById('win').click()`);
await sleep(300);
const sheet = await evaluate(`document.getElementById('wins').getBoundingClientRect().width`);
if (!(sheet > 360)) fail("on a phone the window list should span the screen: " + sheet);
await shot("w4-phone-menu.png");
await evaluate(`document.getElementById('win').click()`);

// Remote access turned off on the Mac (the test does it on this line):
// once the link has been down a moment, a dialog says so, and keeps trying.
step = "remote access turned off";
const lostShown = `!document.getElementById('lost').hidden`;
if (await evaluate(lostShown)) fail("the disconnected dialog showed while connected");
console.log("STEP turn-off");
await waitFor(lostShown, "the disconnected dialog", 20000);
// It first says the link dropped; once the server is found gone, why.
const lostWhy = `document.getElementById('lostTitle').textContent + " | " + document.getElementById('lostText').textContent`;
await waitFor(`(${lostWhy}).includes("turned off")`, "the dialog saying why", 10000);
const lost = await evaluate(lostWhy);
if (!lost.includes("Disconnected") || /\bMac\b/.test(lost)) fail("it should say why, for any computer: " + lost);
await press("Enter", "Enter");                     // ↵: try now (still gone)
await sleep(1200);
if (!(await evaluate(lostShown))) fail("still disconnected: the dialog stays");
await shot("w4b-phone-disconnected.png");
await cdp("Emulation.clearDeviceMetricsOverride");
await sleep(400);
await shot("w4c-disconnected.png");
console.log(`OK listed=${listed} frame=${cw}x${ch}`);
process.exit(0);
