// Runs the iPhone app in the simulator against a real daemon on this Mac,
// announced over Bonjour, with dent-push running locally.
//
//   node spikes/core-ffi/run-sim.mjs [--appearance dark] [--text-size accessibility-large]
//        [--record out.mov] [--restart] [--shots DIR] [--bench]
//
// The UI test finds the daemon, pairs with its code, sends a prompt and
// watches the reply; --restart kills and restarts the daemon mid-reply;
// --bench times a 100,000-event catch-up on the phone instead.

import { execFileSync, spawn } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, "../..");
const ios = path.join(here, "ios");
const args = process.argv.slice(2);
const opt = (k) => (args.includes(k) ? args[args.indexOf(k) + 1] : null);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sh = (cmd, a, o = {}) => execFileSync(cmd, a, { encoding: "utf8", ...o });

const freePort = () =>
  new Promise((res) => {
    const s = net.createServer().listen(0, "127.0.0.1", () => {
      const p = s.address().port;
      s.close(() => res(p));
    });
  });

// The simulator, booted and dressed for stills.
const device = opt("--device") ?? "iPhone 17";
const list = JSON.parse(sh("xcrun", ["simctl", "list", "devices", "available", "-j"]));
const udid = Object.values(list.devices).flat().find((d) => d.name === device)?.udid;
if (!udid) throw new Error(`no simulator named ${device}`);
try { sh("xcrun", ["simctl", "boot", udid]); } catch {}
sh("xcrun", ["simctl", "bootstatus", udid, "-b"]);
sh("xcrun", ["simctl", "status_bar", udid, "override", "--time", "9:41", "--batteryState", "charged", "--batteryLevel", "100", "--cellularBars", "4", "--wifiBars", "3"]);
sh("xcrun", ["simctl", "ui", udid, "appearance", opt("--appearance") ?? "light"]);
sh("xcrun", ["simctl", "ui", udid, "content_size", opt("--text-size") ?? "large"]);

// dent-push, then the daemon announcing itself.
const pushPort = await freePort();
const push = spawn("node", [path.join(repo, "push/test/run.mjs"), "--serve", "--port", String(pushPort)], { stdio: "ignore" });
const pushUrl = `http://127.0.0.1:${pushPort}`;
const state = fs.mkdtempSync(path.join(os.tmpdir(), "dent-spike07-sim-"));
const port = await freePort();
const daemonArgs = ["daemon", "--name", "studio", "--state", state, "--lan", `0.0.0.0:${port}`,
  "--agent-style", "markdown", "--agent-ms", opt("--agent-ms") ?? "45", "--push-url", pushUrl];
let daemon = spawn(path.join(repo, "target/debug/spike-link"), daemonArgs, { stdio: "ignore" });

let recorder = null;
const cleanup = () => {
  recorder?.kill("SIGINT");
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

const waitFor = async (f, ms) => {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    if (await f()) return true;
    await sleep(50);
  }
  return false;
};
await waitFor(() => new Promise((r) => net.connect(port, "127.0.0.1").on("connect", function () { this.destroy(); r(true); }).on("error", () => r(false))), 5000);
await waitFor(() => fetch(`${pushUrl}/health`).then((r) => r.ok, () => false), 30000);

daemon.kill("SIGHUP");
await waitFor(() => (fs.existsSync(path.join(state, "pairing-code")) && fs.readFileSync(path.join(state, "pairing-code"), "utf8").length === 6), 3000);
const code = fs.readFileSync(path.join(state, "pairing-code"), "utf8");

if (args.includes("--bench")) {
  const out = path.join(os.tmpdir(), `dent-bench-${process.pid}.json`);
  const app = spawn("xcrun", ["simctl", "launch", "--console-pty", "--terminate-running-process", udid, "dev.dent.spike07",
    "-fresh", "-bench", "-addr", `127.0.0.1:${port}`, "-code", code], { env: { ...process.env, SIMCTL_CHILD_DENT_OUT: out } });
  app.stdout.on("data", (d) => process.stdout.write(d));
  await waitFor(() => fs.existsSync(out), 120000);
  app.kill("SIGINT");
  console.log(fs.readFileSync(out, "utf8"));
  process.exit(0);
}

const shots = opt("--shots") ?? fs.mkdtempSync(path.join(os.tmpdir(), "dent-shots-"));
fs.mkdirSync(shots, { recursive: true });
for (const f of fs.readdirSync(shots)) if (f.endsWith(".mark")) fs.rmSync(path.join(shots, f));

// Install and launch once outside the test so the recording starts on the
// app, not on a half-booted home screen.
const dd = path.join(ios, "build/dd");
if (opt("--record")) {
  recorder = spawn("xcrun", ["simctl", "io", udid, "recordVideo", "--codec", "h264", "--force", opt("--record")], { stdio: "ignore" });
  await sleep(1500);
}

const test = spawn("xcodebuild", ["test-without-building", "-project", path.join(ios, "Dent.xcodeproj"), "-scheme", "Dent",
  "-destination", `id=${udid}`, "-derivedDataPath", dd, "-only-testing:DentUITests/DentUITests/testPairAndChat"], {
  env: {
    ...process.env,
    TEST_RUNNER_DENT_CODE: code,
    TEST_RUNNER_DENT_SHOTS: shots,
    TEST_RUNNER_DENT_PUSHURL: pushUrl,
  },
  stdio: ["ignore", "pipe", "pipe"],
});
const exited = new Promise((r) => test.on("exit", r));
let log = "";
test.stdout.on("data", (d) => (log += d));
test.stderr.on("data", (d) => (log += d));

if (args.includes("--freeze-at-add")) {
  // The machine stops answering just as it's added, so the code is typed
  // while the phone is still connecting; then it wakes.
  await waitFor(() => fs.existsSync(path.join(shots, "found.mark")), 120000);
  daemon.kill("SIGSTOP");
  await sleep(5000);
  daemon.kill("SIGCONT");
  console.log("froze the daemon for 5 s as it was added");
}

if (args.includes("--restart")) {
  // Mid-reply, the machine restarts: kill it and start it again on the same
  // state and port. The session holder keeps the agent going meanwhile.
  let gone = false;
  exited.then(() => (gone = true));
  await waitFor(() => gone || fs.existsSync(path.join(shots, "sent.mark")), 120000);
  await sleep(Number(opt("--restart-after") ?? 2600));
  daemon.kill("SIGKILL");
  await sleep(1200);
  daemon = spawn(path.join(repo, "target/debug/spike-link"), daemonArgs, { stdio: "ignore" });
  console.log("restarted the daemon mid-reply");
}

const status = await exited;
if (recorder) {
  await sleep(500);
  recorder.kill("SIGINT");
  await new Promise((r) => recorder.on("exit", r));
  recorder = null;
}
const result = log.match(/\*\* TEST (?:EXECUTE )?(SUCCEEDED|FAILED) \*\*/)?.[1] ?? "unknown";
console.log(`UI test: ${result}; shots in ${shots}`);
if (status !== 0) console.log(log.split("\n").filter((l) => /error|failed|XCTAssert/i.test(l)).slice(0, 20).join("\n"));
process.exit(status ?? 1);
