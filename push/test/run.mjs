// dent-push under wrangler dev, against the mock APNs: handles, sealing,
// limits, Apple's answers and bad input.
//
//   npm test                      the worker's own checks
//   node test/run.mjs --serve     keep it running (for the daemon and the
//                                 simulator), forwarding to the simulator

import { spawn } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { start as mockApns } from "./mock-apns.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, "..");
const serve = process.argv.includes("--serve");
const portArg = process.argv.indexOf("--port");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const freePort = () =>
  new Promise((res) => {
    const s = net.createServer().listen(0, "127.0.0.1", () => {
      const p = s.address().port;
      s.close(() => res(p));
    });
  });

// Throwaway keys for this run: two sealing key versions, and an Apple-style
// P-256 key whose public half the mock checks signatures with.
const seal1 = crypto.randomBytes(32);
const seal2 = crypto.randomBytes(32);
const { privateKey, publicKey } = crypto.generateKeyPairSync("ec", { namedCurve: "prime256v1" });
const topic = "dev.dent.spike07";
const apns = await mockApns({ publicKey, keyId: "TESTKEY123", teamId: "TEAM123456", topic, simulator: serve ? topic : null });

// wrangler reads secrets from .dev.vars beside the config it's given.
const devVars = [path.join(root, ".dev.vars"), path.join(root, "test/.dev.vars")];
const vars = [
    `SEAL_KEYS='${JSON.stringify({ 1: seal1.toString("base64"), 2: seal2.toString("base64") })}'`,
    `SEAL_CURRENT=2`,
    `APNS_KEY="${privateKey.export({ type: "pkcs8", format: "pem" }).replace(/\n/g, "\\n")}"`,
    `APNS_KEY_ID=TESTKEY123`,
    `APNS_TEAM_ID=TEAM123456`,
    `APNS_TOPIC=${topic}`,
    `APNS_ORIGIN_SANDBOX=${apns.origin}`,
    `APNS_ORIGIN_PRODUCTION=${apns.origin}`,
    `BURST=${serve ? 3 : 5}`,
    `DAILY=${serve ? 50 : 7}`,
    `ALLOW_FAKE_TOKENS=1`,
    `TEST_CLOCK=1`,
  ].join("\n");
for (const f of devVars) fs.writeFileSync(f, vars);

const port = portArg > 0 ? Number(process.argv[portArg + 1]) : await freePort();
const wrangler = spawn(path.join(root, "node_modules/.bin/wrangler"), ["dev", "--port", String(port), "--ip", "127.0.0.1", "-c", serve ? "wrangler.jsonc" : "test/wrangler.test.jsonc"], {
  cwd: root,
  stdio: ["ignore", "pipe", "pipe"],
  env: { ...process.env, WRANGLER_SEND_METRICS: "false" },
});
let log = "";
wrangler.stdout.on("data", (d) => (log += d));
wrangler.stderr.on("data", (d) => (log += d));
const stop = () => {
  wrangler.kill("SIGTERM");
  apns.server.close();
  for (const f of devVars) fs.rmSync(f, { force: true });
};
process.on("exit", stop);
process.on("SIGINT", () => process.exit(130));
process.on("SIGTERM", () => process.exit(143));

const base = `http://127.0.0.1:${port}`;
for (let i = 0; i < 300; i++) {
  try {
    if ((await fetch(`${base}/health`)).ok) break;
  } catch {}
  await sleep(100);
}

if (serve) {
  console.log(`dent-push ${base} (mock APNs ${apns.origin}, forwarding to the simulator)`);
  await new Promise(() => {});
}

const rows = [];
const check = (name, ok, detail) => {
  console.log(`${ok ? "PASS" : "FAIL"} ${name} (${detail})`);
  rows.push({ name, ok, detail });
};
const post = async (p, body, headers = {}) => {
  const r = await fetch(`${base}${p}`, { method: "POST", headers: { "content-type": "application/json", ...headers }, body: typeof body === "string" ? body : JSON.stringify(body) });
  return { status: r.status, body: await r.json().catch(() => null) };
};
const token = (c) => c.repeat(64);
const payload = Buffer.from("sealed notice bytes").toString("base64");

try {
  // Handles.
  let r = await post("/register", { token: token("a"), env: "sandbox" });
  const h = r.body?.handle;
  check("register returns a sealed handle", r.status === 200 && h?.startsWith("h1.") && !h.includes(token("a")), h ? `${h.length} chars` : JSON.stringify(r));
  const raw = Buffer.from(h.slice(3), "base64url");
  check("the handle is sealed with the current key version", raw[0] === 2, `version ${raw[0]}`);

  r = await post("/register", { token: "not hex", env: "sandbox" });
  check("a malformed token is refused", r.status === 400, `${r.status}`);

  const tampered = h.slice(0, -4) + (h.endsWith("AAAA") ? "BBBB" : "AAAA");
  r = await post("/send", { handle: tampered, kind: "finished", payload });
  check("a tampered handle is refused", r.status === 400 && r.body.error === "bad handle", `${r.status} ${r.body?.error}`);

  // A handle from before a key rotation still works.
  const iv = crypto.randomBytes(12);
  const c = crypto.createCipheriv("aes-256-gcm", seal1, iv);
  const ct = Buffer.concat([c.update(JSON.stringify({ t: token("b"), e: "sandbox" })), c.final(), c.getAuthTag()]);
  const old = `h1.${Buffer.concat([Buffer.from([1]), iv, ct]).toString("base64url")}`;
  r = await post("/send", { handle: old, kind: "finished", payload });
  check("a handle sealed with the previous key still sends", r.status === 200, `${r.status} ${JSON.stringify(r.body)}`);

  // What reaches Apple.
  let t0 = Date.now();
  r = await post("/send", { handle: h, kind: "approval", payload, collapse: "session-1" }, { "x-test-now": String(t0) });
  const p = apns.pushes.at(-1);
  check(
    "a notice reaches Apple sealed, with the right headers",
    r.status === 200 &&
      p.token === token("a") &&
      p.headers["apns-topic"] === topic &&
      p.headers["apns-push-type"] === "alert" &&
      p.headers["apns-priority"] === "10" &&
      p.headers["apns-collapse-id"] === "session-1" &&
      p.body.e === payload &&
      p.body.aps["mutable-content"] === 1 &&
      p.body.aps["interruption-level"] === "time-sensitive",
    `${p.headers["apns-push-type"]}, ${p.body.aps["interruption-level"]}, fallback "${p.body.aps.alert.body}"`,
  );
  r = await post("/send", { handle: h, kind: "refresh", payload: "" }, { "x-test-now": String(t0 + 1000) });
  const q = apns.pushes.at(-1);
  check("a silent refresh goes as a background push", r.status === 200 && q.headers["apns-push-type"] === "background" && q.headers["apns-priority"] === "5" && !q.body.aps.alert, q.headers["apns-push-type"]);
  check("the provider token is signed once and reused", apns.pushes.slice(-3).every((x) => x.jwt === p.jwt), `${new Set(apns.pushes.map((x) => x.jwt)).size} token(s) for ${apns.pushes.length} pushes`);

  // Limits: 5 a minute, 7 a day, per phone.
  r = await post("/register", { token: token("c"), env: "sandbox" });
  const lim = r.body.handle;
  t0 = Date.UTC(2026, 9, 12, 12, 0, 0);
  const codes = [];
  let first429 = null;
  for (let i = 0; i < 6; i++) {
    const x = await post("/send", { handle: lim, kind: "finished", payload }, { "x-test-now": String(t0 + i) });
    codes.push(x.status);
    if (x.status === 429 && !first429) first429 = x.body;
  }
  check("a burst over the per-minute limit is held back, saying until when", codes.join() === "200,200,200,200,200,429" && first429.why === "burst" && first429.until > t0, `${codes.join(" ")}; until +${first429 && first429.until - t0} ms`);
  const more = [];
  for (let i = 0; i < 3; i++) {
    const x = await post("/send", { handle: lim, kind: "finished", payload }, { "x-test-now": String(t0 + 60_000 + i * 15_000) });
    more.push(x);
  }
  check(
    "the daily cap holds until midnight UTC",
    more[0].status === 200 && more[1].status === 200 && more[2].status === 429 && more[2].body.why === "daily" && more[2].body.until === Date.UTC(2026, 9, 13),
    more.map((x) => x.status).join(" ") + (more[2].body?.until ? `; until ${new Date(more[2].body.until).toISOString()}` : ""),
  );
  r = await post("/send", { handle: lim, kind: "finished", payload }, { "x-test-now": String(Date.UTC(2026, 9, 13, 0, 1)) });
  check("the next day it sends again", r.status === 200, `${r.status}`);

  // Apple says the phone is gone.
  r = await post("/register", { token: "dead" + token("d").slice(4), env: "sandbox" });
  r = await post("/send", { handle: r.body.handle, kind: "finished", payload });
  check("an unregistered phone is reported back as dead", r.status === 410 && r.body.error === "dead", `${r.status} ${r.body?.reason}`);

  // Bad input.
  const big = await post("/send", { handle: h, kind: "finished", payload: "A".repeat(3001) });
  const kind = await post("/send", { handle: h, kind: "shout", payload });
  const bad = await post("/send", "{not json");
  const get = await fetch(`${base}/send`);
  const nope = await fetch(`${base}/nope`, { method: "POST" });
  const huge = await post("/send", "x".repeat(10000));
  check(
    "oversized, unknown and malformed requests are refused",
    big.status === 413 && kind.status === 400 && bad.status === 400 && get.status === 405 && nope.status === 404 && huge.status === 400,
    `payload ${big.status}, kind ${kind.status}, json ${bad.status}, GET ${get.status}, path ${nope.status}, body ${huge.status}`,
  );
  check("fake simulator tokens only with the development flag", (await post("/register", { token: "sim-x", env: "sandbox" })).status === 200, "ALLOW_FAKE_TOKENS=1 here; unset in production");

  // Per address: the test config allows 40 a minute.
  let limitedAt = null;
  for (let i = 0; i < 60; i++) {
    const x = await post("/register", { token: token("e"), env: "sandbox" });
    if (x.status === 429) {
      limitedAt = { i, why: x.body.why };
      break;
    }
  }
  check("one address can't flood it", limitedAt?.why === "address", limitedAt ? `limited after ${limitedAt.i} more requests` : "never limited");
} catch (e) {
  check("dent-push harness", false, String(e?.stack ?? e));
  console.log(log.slice(-3000));
}

const out = process.argv.indexOf("--out");
if (out > 0) fs.writeFileSync(process.argv[out + 1], JSON.stringify(rows, null, 2));
console.log(`\n${rows.filter((r) => r.ok).length} of ${rows.length} passed`);
process.exit(rows.every((r) => r.ok) ? 0 : 1);
