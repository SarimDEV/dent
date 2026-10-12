// Captures every concept screen in the simulator.
//
//   node spikes/phone-concepts/shoot.mjs OUT_DIR [--looks paper,book] [--screens home,chat]
//        [--appearance light|dark] [--text-size large|accessibility-extra-extra-extra-large]
//
// Build first: xcodegen, then xcodebuild build into build/dd (see README).

import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const opt = (k, d) => (args.includes(k) ? args[args.indexOf(k) + 1] : d);
const out = path.resolve(args[0] ?? "shots");
const sh = (cmd, a) => execFileSync(cmd, a, { encoding: "utf8" });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const all = ["home", "home-zero", "closed", "many", "empty", "new", "new-typed", "chat", "chat-transcript", "chat-tools", "chat-working",
  "chat-offline", "chat-pr", "changes", "doc", "settings", "settings-limited", "machine", "machine-remove",
  "add", "code", "qr", "paired", "syncing", "welcome", "lock", "notify-actions", "island"];
const screens = opt("--screens", all.join(",")).split(",");
const looks = opt("--looks", "system,paper,serif,book").split(",");
const appearance = opt("--appearance", "light");
const size = opt("--text-size", "large");

const list = JSON.parse(sh("xcrun", ["simctl", "list", "devices", "available", "-j"]));
const udid = Object.values(list.devices).flat().find((d) => d.name === opt("--device", "iPhone 17"))?.udid;
try { sh("xcrun", ["simctl", "boot", udid]); } catch {}
sh("xcrun", ["simctl", "bootstatus", udid, "-b"]);
sh("xcrun", ["simctl", "status_bar", udid, "override", "--time", "9:41", "--batteryState", "charged", "--batteryLevel", "100", "--cellularBars", "4", "--wifiBars", "3"]);
sh("xcrun", ["simctl", "ui", udid, "appearance", appearance]);
sh("xcrun", ["simctl", "ui", udid, "content_size", size]);
sh("xcrun", ["simctl", "install", udid, path.join(here, "build/dd/Build/Products/Debug-iphonesimulator/Concepts.app")]);

fs.mkdirSync(out, { recursive: true });
const tag = [appearance === "dark" ? "dark" : "", size === "large" ? "" : "ax"].filter(Boolean).join("-");
for (const look of looks) {
  for (const screen of screens) {
    sh("xcrun", ["simctl", "launch", "--terminate-running-process", udid, "dev.dent.concepts", "-screen", screen, "-look", look]);
    // Sheets, pushes and the keyboard all animate in; let them settle.
    await sleep(["code", "machine-remove", "add", "qr", "paired", "new", "new-typed", "empty-add"].includes(screen) ? 3200 : 2200);
    const file = path.join(out, `${look}-${screen}${tag ? "-" + tag : ""}.png`);
    sh("xcrun", ["simctl", "io", udid, "screenshot", file]);
    console.log(file);
  }
}
sh("xcrun", ["simctl", "terminate", udid, "dev.dent.concepts"]);
// Leave the simulator as we found it, so tests run at the default size.
sh("xcrun", ["simctl", "ui", udid, "appearance", "light"]);
sh("xcrun", ["simctl", "ui", udid, "content_size", "large"]);
