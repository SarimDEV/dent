// The wasm build of the core, in Node, against spike 05's daemon: pairing,
// a streamed prompt and a 100,000-event catch-up. Same host code as the
// browser page.
//
//   node spikes/core-wasm/test/node.mjs [--out results.json]

import { spawn } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import * as core from "../web/pkg/dent_core_web.js";
import { Machine, Mirror, hex } from "../web/host.js";

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, "../../..");
core.initSync({ module: fs.readFileSync(path.join(here, "../web/pkg/dent_core_web_bg.wasm")) });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const rows = [];
const check = (name, ok, detail) => {
  console.log(`${ok ? "PASS" : "FAIL"} ${name} (${detail})`);
  rows.push({ name, ok, detail });
};

function freePort() {
  return new Promise((res) => {
    const s = net.createServer().listen(0, "127.0.0.1", () => {
      const p = s.address().port;
      s.close(() => res(p));
    });
  });
}

async function until(f, ms) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    if (f()) return true;
    await sleep(10);
  }
  return f();
}

const state = fs.mkdtempSync(path.join(os.tmpdir(), "dent-spike07-node-"));
const port = await freePort();
// dent-push locally (wrangler dev and a mock of Apple), so the daemon's
// notifications can be followed through, limits included.
const pushPort = await freePort();
const push = spawn("node", [path.join(repo, "push/test/run.mjs"), "--serve", "--port", String(pushPort)], { stdio: "ignore" });
const pushUrl = `http://127.0.0.1:${pushPort}`;
const daemon = spawn(
  path.join(repo, "target/debug/spike-link"),
  ["daemon", "--name", "studio", "--state", state, "--lan", `0.0.0.0:${port}`, "--no-mdns",
   "--agent-style", "markdown", "--agent-ms", "5", "--push-url", pushUrl],
  { stdio: "ignore" },
);
// The daemon's session holder outlives it by design; stop it too. Its
// folder is named from the state path, as spike 06 derives it.
const cleanup = () => {
  daemon.kill("SIGKILL");
  push.kill("SIGTERM");
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

try {
  await until(() => false, 300);
  const keys = core.generate_keys();
  let notes = [];
  const mirror = new Mirror();
  const onNotes = (ns) => {
    for (const n of ns) {
      notes.push({ at: Date.now(), n });
      if (n.type === "message") mirror.apply(n.update);
    }
  };
  const addr = `127.0.0.1:${port}`;
  let m = new Machine(core, { name: "node", keys, known: null, addrs: [addr], onNotes });
  m.start();
  const status = () => [...notes].reverse().find((x) => x.n.type === "status")?.n.status;
  await until(() => status() === "needs_code", 5000);
  check("the wasm core finds an unpaired machine", status() === "needs_code", status());

  fs.rmSync(path.join(state, "pairing-code"), { force: true });
  daemon.kill("SIGHUP");
  await until(() => (fs.existsSync(path.join(state, "pairing-code")) && fs.readFileSync(path.join(state, "pairing-code"), "utf8").length === 6), 2000);
  const code = fs.readFileSync(path.join(state, "pairing-code"), "utf8");
  let t = Date.now();
  m.call((l, now) => l.pair(code, now));
  await until(() => status() === "online", 5000);
  const known = m.link.known_key();
  check("pairs with the code, in wasm", status() === "online" && !!known, `online ${Date.now() - t} ms after the code`);

  const s = "s-web";
  t = Date.now();
  m.call((l) => {
    l.follow(s);
    l.prompt(s, "Why does the sync test fail after a reconnect?");
  });
  await until(() => mirror.get(s, 1)?.state === "done", 15000);
  const first = notes.find((x) => x.n.type === "message" && x.n.update.role === "agent" && x.n.update.blocks.length);
  const blocks = mirror.get(s, 1)?.blocks ?? [];
  check(
    "a prompt streams back as blocks, in wasm",
    mirror.get(s, 1)?.state === "done" && blocks.some((b) => b.kind.type === "code") && blocks.some((b) => b.kind.type === "heading"),
    `first text ${first ? first.at - t : "-"} ms, ${blocks.length} blocks`,
  );

  // 100,000 events: logged by the daemon, then caught up by a fresh core.
  const big = "s-100k";
  m.call((l) => {
    l.follow(big);
    l.bulk(big, 100000);
  });
  await until(() => JSON.parse(m.link.stats()).events >= 100000 + 30, 60000);
  m.stop();

  notes = [];
  const fresh = new Mirror();
  const c = new Machine(core, {
    name: "node", keys, known, addrs: [addr],
    onNotes: (ns) => ns.forEach((n) => n.type === "message" && fresh.apply(n.update)),
  });
  t = performance.now();
  c.call((l) => l.follow(big));
  c.start();
  await until(() => JSON.parse(c.link.stats()).events >= 100000 && (fresh.get(big, 0)?.blocks.length ?? 0) >= 10000, 60000);
  const ms = performance.now() - t;
  const st = JSON.parse(c.link.stats());
  check(
    "a 100,000-event session catches up in wasm",
    st.events === 100000 && st.gaps === 0,
    `${(ms / 1000).toFixed(2)} s, ${Math.round(st.events / (ms / 1000))} events/s, ${c.drainMs.toFixed(0)} ms taking notes`,
  );
  c.stop();

  // Phone notifications: the daemon seals one per finished reply and
  // dent-push forwards it. Over the limit (3 a minute here) the machine
  // says so in its health, and so does a dent-push that's down.
  let up = false;
  for (let i = 0; i < 200 && !up; i++) {
    up = await fetch(`${pushUrl}/health`).then((r) => r.ok, () => false);
    if (!up) await sleep(100);
  }
  const reg = await fetch(`${pushUrl}/register`, {
    method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ token: "sim-node", env: "sandbox" }),
  }).then((r) => r.json());
  let health = null;
  const p = new Machine(core, {
    name: "node", keys, known, addrs: [addr],
    onNotes: (ns) => ns.forEach((n) => {
      if (n.type === "message") mirror.apply(n.update);
      if (n.type === "health") health = n.health;
    }),
  });
  const ps = "s-push";
  p.call((l) => { l.follow(ps); l.set_push(reg.handle, hex.to(crypto.randomBytes(32))); });
  p.start();
  await until(() => health !== null, 5000);
  for (let i = 0; i < 4; i++) {
    p.call((l) => l.prompt(ps, `reply ${i}`));
    await until(() => mirror.get(ps, i * 2 + 1)?.state === "done", 15000);
  }
  await until(() => health?.push?.startsWith("limited:"), 8000);
  const untilMs = Number(health?.push?.slice(8));
  check(
    "a phone over its limit shows on the machine, with until when",
    !!up && health?.push?.startsWith("limited:") && untilMs > Date.now(),
    `${health?.push ?? "nothing"} (in ${Math.round((untilMs - Date.now()) / 1000)} s)`,
  );
  push.kill("SIGTERM");
  await sleep(1500);
  p.call((l) => l.prompt(ps, "after dent-push went away"));
  await until(() => mirror.get(ps, 9)?.state === "done", 15000);
  await until(() => health?.push === "unavailable", 8000);
  check("dent-push down shows as unavailable", health?.push === "unavailable", health?.push ?? "nothing");
  p.stop();

  // Parsing cost inside wasm, for comparison with native.
  const text = "## Heading\n\nSome **bold** text with `code` and a [link](https://x).\n\n- one\n- two\n\n```rust\nfn x() {}\n```\n\n".repeat(2000);
  t = performance.now();
  const parsed = JSON.parse(core.parse_markdown(text));
  check("parse 250 KB of Markdown in wasm", parsed.length > 0, `${(performance.now() - t).toFixed(0)} ms, ${parsed.length} blocks`);
} catch (e) {
  check("node harness", false, String(e?.stack ?? e));
} finally {
  const out = process.argv.indexOf("--out");
  if (out > 0) fs.writeFileSync(process.argv[out + 1], JSON.stringify(rows, null, 2));
  console.log(`\n${rows.filter((r) => r.ok).length} of ${rows.length} passed`);
  process.exit(rows.every((r) => r.ok) ? 0 : 1);
}
