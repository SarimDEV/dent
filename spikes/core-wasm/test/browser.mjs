// The web client in Chrome against spike 05's daemon: add the machine by
// address, pair with its code, send a prompt, watch the Markdown reply.
// Takes stills (light, dark, narrow, wide) and an optional recording of the
// page only.
//
//   node spikes/core-wasm/test/browser.mjs --shots DIR [--record DIR]

import { spawn, spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { chromium } from "playwright-core";

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, "../../..");
const web = path.join(here, "../web");
const args = process.argv.slice(2);
const opt = (k) => (args.includes(k) ? args[args.indexOf(k) + 1] : null);
const shots = opt("--shots") ?? fs.mkdtempSync(path.join(os.tmpdir(), "dent-web-shots-"));
fs.mkdirSync(shots, { recursive: true });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const freePort = () =>
  new Promise((res) => {
    const s = net.createServer().listen(0, "127.0.0.1", () => {
      const p = s.address().port;
      s.close(() => res(p));
    });
  });

const types = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".wasm": "application/wasm", ".ttf": "font/ttf" };
const server = http.createServer((req, res) => {
  const p = path.join(web, decodeURIComponent(new URL(req.url, "http://x").pathname.replace(/\/$/, "/index.html")));
  if (!p.startsWith(web) || !fs.existsSync(p)) return res.writeHead(404).end();
  res.writeHead(200, { "content-type": types[path.extname(p)] ?? "application/octet-stream" });
  fs.createReadStream(p).pipe(res);
});
const webPort = await freePort();
await new Promise((r) => server.listen(webPort, "127.0.0.1", r));

const state = fs.mkdtempSync(path.join(os.tmpdir(), "dent-spike07-web-"));
const port = await freePort();
let daemon = spawn(path.join(repo, "target/debug/spike-link"),
  ["daemon", "--name", "studio", "--state", state, "--lan", `0.0.0.0:${port}`, "--no-mdns", "--agent-style", "markdown", "--agent-ms", "45"],
  { stdio: "ignore" });
const cleanup = () => {
  daemon.kill("SIGKILL");
  server.close();
  try {
    const hash = crypto.createHash("sha256").update(fs.realpathSync(state)).digest("hex").slice(0, 12);
    const dir = path.join(os.tmpdir(), `dent-${hash}`);
    process.kill(Number(fs.readFileSync(path.join(dir, "holder.lock"), "utf8").trim()), "SIGKILL");
    fs.rmSync(dir, { recursive: true, force: true });
  } catch {}
  fs.rmSync(state, { recursive: true, force: true });
};
process.on("exit", cleanup);
process.on("SIGINT", () => process.exit(130));
process.on("SIGTERM", () => process.exit(143));
await sleep(500);

const browser = await chromium.launch({ executablePath: "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", headless: true });
const rows = [];
const check = (name, ok, detail) => {
  console.log(`${ok ? "PASS" : "FAIL"} ${name} (${detail})`);
  rows.push({ name, ok, detail });
};

// Chrome's own screencast: frames of the page only, with their times, made
// into a video with ffmpeg afterwards.
const frames = [];
async function screencast(page, dir) {
  fs.mkdirSync(dir, { recursive: true });
  const cdp = await page.context().newCDPSession(page);
  cdp.on("Page.screencastFrame", async (f) => {
    const file = path.join(dir, `f${String(frames.length).padStart(5, "0")}.jpg`);
    fs.writeFileSync(file, Buffer.from(f.data, "base64"));
    frames.push({ file, t: f.metadata.timestamp });
    await cdp.send("Page.screencastFrameAck", { sessionId: f.sessionId }).catch(() => {});
  });
  await cdp.send("Page.startScreencast", { format: "jpeg", quality: 90, everyNthFrame: 1 });
}

function encode(dir) {
  if (frames.length < 2) return;
  const list = frames.map((f, i) => {
    const d = i + 1 < frames.length ? frames[i + 1].t - f.t : 1;
    return `file '${f.file}'\nduration ${d.toFixed(3)}`;
  });
  list.push(`file '${frames.at(-1).file}'`);
  fs.writeFileSync(path.join(dir, "frames.txt"), list.join("\n"));
  spawnSync("ffmpeg", ["-y", "-loglevel", "error", "-f", "concat", "-safe", "0", "-i", path.join(dir, "frames.txt"),
    "-vf", "fps=30,scale=trunc(iw/2)*2:trunc(ih/2)*2", "-pix_fmt", "yuv420p", "-c:v", "libx264", "-crf", "20", path.join(dir, "web.mp4")]);
}

async function session(theme, viewport, record) {
  const ctx = await browser.newContext({ viewport, deviceScaleFactor: 2, colorScheme: theme });
  const page = await ctx.newPage();
  if (record) await screencast(page, record);
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  await page.goto(`http://127.0.0.1:${webPort}/?fresh&addr=127.0.0.1:${port}&theme=${theme}`);
  await page.waitForSelector("#connect:not([hidden])");
  await sleep(record ? 900 : 100);
  await page.click("#connect-btn");
  await page.waitForSelector("#code:not([hidden])", { timeout: 8000 });
  fs.rmSync(path.join(state, "pairing-code"), { force: true });
  daemon.kill("SIGHUP");
  for (let i = 0; i < 100 && !(fs.existsSync(path.join(state, "pairing-code")) && fs.readFileSync(path.join(state, "pairing-code"), "utf8").length === 6); i++) await sleep(20);
  const code = fs.readFileSync(path.join(state, "pairing-code"), "utf8");
  await sleep(record ? 700 : 0);
  await page.type("#code-input", code, { delay: record ? 140 : 0 });
  const t = Date.now();
  await page.click("#pair-btn");
  await page.waitForSelector("#composer:not([hidden])", { timeout: 8000 });
  const paired = Date.now() - t;
  await sleep(record ? 800 : 200);
  await page.type("#prompt", "Why does the sync test fail after a reconnect?", { delay: record ? 35 : 0 });
  await page.keyboard.press("Enter");
  await page.waitForSelector(".msg.agent", { timeout: 8000 });
  await sleep(1200);
  return { ctx, page, errors, paired };
}

try {
  // Light, phone width, recorded.
  let s = await session("light", { width: 420, height: 760 }, opt("--record"));
  await s.page.screenshot({ path: path.join(shots, "web-streaming.png") });
  await s.page.waitForSelector(".msg.agent .end", { timeout: 20000 });
  await sleep(1200);
  await s.page.screenshot({ path: path.join(shots, "web-reply.png") });
  const counts = await s.page.evaluate(() => ({
    code: document.querySelectorAll(".msg.agent pre.code").length,
    items: document.querySelectorAll(".msg.agent .item").length,
    heading: document.querySelectorAll(".msg.agent h3").length,
    quote: document.querySelectorAll(".msg.agent .q").length,
    overflow: document.documentElement.scrollWidth > document.documentElement.clientWidth,
    // Once paired, only the machine, the chat and the composer show.
    stray: ["connect", "code"].filter((id) => document.getElementById(id).offsetHeight > 0),
  }));
  check("pairs and streams a reply in Chrome",
    counts.code === 1 && counts.items === 3 && counts.heading === 1 && counts.quote === 1 && !s.errors.length,
    `paired in ${s.paired} ms; ${counts.heading} heading, ${counts.items} items, ${counts.code} code block, ${counts.quote} quote; errors ${s.errors.length}`);
  check("no sideways scroll at 420 px", !counts.overflow, `overflow: ${counts.overflow}`);
  check("once paired, the add and code steps are gone", counts.stray.length === 0, counts.stray.join(", ") || "none showing");

  // Reload: it reconnects from what it saved, without a code.
  await s.page.goto(`http://127.0.0.1:${webPort}/`);
  await s.page.waitForFunction(() => window.dent?.status === "online", null, { timeout: 8000 });
  check("a reload reconnects without a code", true, "online again");
  // The machine restarts mid-reply.
  await s.page.type("#prompt", "And the second flaky test?", { delay: opt("--record") ? 30 : 0 });
  await s.page.keyboard.press("Enter");
  await s.page.waitForFunction(() => document.querySelectorAll(".msg.agent").length >= 2 && document.querySelectorAll(".msg.agent")[1].querySelectorAll(".b").length >= 3, null, { timeout: 10000 });
  daemon.kill("SIGKILL");
  await sleep(700);
  await s.page.screenshot({ path: path.join(shots, "web-reconnecting.png") });
  daemon = spawn(path.join(repo, "target/debug/spike-link"),
    ["daemon", "--name", "studio", "--state", state, "--lan", `0.0.0.0:${port}`, "--no-mdns", "--agent-style", "markdown", "--agent-ms", "45"],
    { stdio: "ignore" });
  await s.page.waitForFunction(() => document.querySelectorAll(".msg.agent .end").length >= 2, null, { timeout: 20000 });
  await sleep(1500);
  const whole = await s.page.evaluate(() => document.querySelectorAll(".msg.agent")[1].innerText.includes("three lines"));
  check("the machine restarts mid-reply: the reply finishes in the page", whole, `finished: ${whole}`);
  await s.page.screenshot({ path: path.join(shots, "web-after-restart.png") });
  await s.ctx.close();
  if (opt("--record")) encode(opt("--record"));

  // Dark, phone width. And wide, light.
  for (const [theme, vp, name] of [["dark", { width: 420, height: 760 }, "web-dark"], ["light", { width: 1100, height: 760 }, "web-wide"]]) {
    s = await session(theme, vp, null);
    await s.page.waitForSelector(".msg.agent .end", { timeout: 20000 });
    await sleep(600);
    await s.page.screenshot({ path: path.join(shots, `${name}.png`) });
    await s.ctx.close();
  }
  // The code step, and a wrong code.
  const ctx = await browser.newContext({ viewport: { width: 420, height: 760 }, deviceScaleFactor: 2 });
  const page = await ctx.newPage();
  await page.goto(`http://127.0.0.1:${webPort}/?fresh&addr=127.0.0.1:${port}`);
  await page.click("#connect-btn");
  await page.waitForSelector("#code:not([hidden])", { timeout: 8000 });
  daemon.kill("SIGHUP");
  await sleep(300);
  await page.type("#code-input", "000001");
  await page.click("#pair-btn");
  await page.waitForSelector("#code-error:not([hidden])", { timeout: 8000 });
  await page.screenshot({ path: path.join(shots, "web-wrong-code.png") });
  check("a wrong code says so plainly", true, await page.textContent("#code-error"));
  await ctx.close();
} catch (e) {
  check("browser harness", false, String(e?.stack ?? e));
} finally {
  await browser.close();
  console.log(`\n${rows.filter((r) => r.ok).length} of ${rows.length} passed; shots in ${shots}`);
  process.exit(rows.every((r) => r.ok) ? 0 : 1);
}
