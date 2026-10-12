// The web client spike: one machine, its pairing, and a chat, drawn from the
// core's notes. All protocol, pairing, reconnecting and Markdown logic is in
// the wasm core; this file is DOM.

import init, * as core from "./pkg/dent_core_web.js";
import { Machine, Mirror, hex } from "./host.js";

await init();

const $ = (id) => document.getElementById(id);
const params = new URLSearchParams(location.search);
if (params.get("theme")) document.documentElement.dataset.theme = params.get("theme");

// Per-browser conveniences only; a private window starts over.
const store = {
  get(k) { try { return localStorage.getItem(k); } catch { return null; } },
  set(k, v) { try { localStorage.setItem(k, v); } catch {} },
  clear() { try { localStorage.clear(); } catch {} },
};
if (params.has("fresh")) store.clear();

let keys = store.get("dent.keys");
if (!keys) {
  keys = hex.to(core.generate_keys());
  store.set("dent.keys", keys);
}
$("me").textContent = `this browser · ${core.device_id(hex.from(keys))}`;

let saved = JSON.parse(store.get("dent.machine") || "null");
let machine = null;
let status = "idle";
let detail = "";
let lostAt = 0;
let rtt = null;
let name = saved?.name ?? "";
let push = null;
const mirror = new Mirror();
const session = saved?.session ?? `web-${hex.to(crypto.getRandomValues(new Uint8Array(4)))}`;
const pending = new Map();
const nodes = new Map();

function show(id, on) {
  $(id).hidden = !on;
}

// ---- Status: a calm line, the square a fill ----

function render() {
  const known = !!saved;
  show("machine", !!machine && (known || status === "needs_code" || status === "pairing"));
  show("connect", !machine || (!known && !["needs_code", "pairing", "connecting"].includes(status)));
  show("code", !known && (status === "needs_code" || status === "pairing"));
  show("chat", known && status !== "removed");
  show("composer", known && status !== "removed");
  $("name").textContent = name || $("addr").value;
  const reconnecting = status === "offline" && Date.now() - lostAt < 10000;
  const [cls, text] = {
    online: ["done", rtt === null ? "Online" : `Online · ${rtt < 1 ? "under 1" : rtt} ms`],
    connecting: ["working", saved ? "Reconnecting…" : "Connecting…"],
    pairing: ["working", "Pairing…"],
    offline: reconnecting ? ["working", "Reconnecting…"] : ["", "Offline · retrying"],
    needs_code: ["needs", "Needs the code shown on it"],
    removed: ["needs", "Removed this browser"],
    incompatible: ["needs", "Needs a dent update"],
    idle: ["", ""],
  }[status] ?? ["", ""];
  $("sq").className = `sq ${cls}`;
  $("sub").textContent = text;
  $("sub").title = detail;
  const limited = push?.startsWith("limited:") ? Number(push.slice(8)) : 0;
  $("notice").hidden = !push;
  if (push === "unavailable") $("notice").textContent = "Phone notifications are unavailable right now.";
  else if (limited) {
    const until = new Date(limited).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
    $("notice").textContent = `Phone notifications paused until ${until}: today's limit is reached. Everything still shows here.`;
  }
  $("prompt").placeholder = `Ask ${name || "it"}…`;
  $("code-hint").textContent = `Enter the 6-digit code shown on ${name || "the machine"}.`;
}

// ---- Blocks to DOM ----

function spans(el, list) {
  let cell = null;
  let cellNo = -1;
  for (const s of list) {
    let target = el;
    if (el.classList.contains("row")) {
      if (s.cell !== cellNo) {
        cell = document.createElement("div");
        el.append(cell);
        cellNo = s.cell;
      }
      target = cell;
    }
    let node;
    if (s.link) {
      node = document.createElement("a");
      node.href = s.link;
      node.target = "_blank";
      node.rel = "noreferrer";
      node.className = "s-l";
    } else if (s.code || s.math) {
      node = document.createElement("code");
      node.className = "s-c";
    } else {
      node = document.createElement("span");
    }
    if (s.bold) node.classList.add("s-b");
    if (s.italic) node.classList.add("s-i");
    if (s.strike) node.classList.add("s-s");
    node.textContent = s.text;
    target.append(node);
  }
}

function block(b) {
  const k = b.kind.type;
  let el;
  if (k === "code") {
    el = document.createElement("pre");
    el.className = "b code";
    el.textContent = b.text.replace(/\n$/, "");
    if (b.kind.lang) {
      const l = document.createElement("span");
      l.className = "lang";
      l.textContent = b.kind.lang;
      el.append(l);
    }
  } else if (k === "rule") {
    el = document.createElement("hr");
    el.className = "b rule";
  } else if (k === "math") {
    el = document.createElement("div");
    el.className = "b math";
    el.textContent = b.text;
  } else if (k === "item") {
    el = document.createElement("div");
    el.className = "b item";
    el.style.paddingLeft = `${(b.depth - 1) * 20}px`;
    const mk = document.createElement("span");
    mk.className = "mk";
    mk.textContent = b.kind.marker;
    const body = document.createElement("span");
    spans(body, b.spans);
    el.append(mk, body);
  } else if (k === "row") {
    el = document.createElement("div");
    el.className = `b row${b.kind.header ? " head" : ""}`;
    spans(el, b.spans);
  } else {
    el = document.createElement(k === "heading" ? "h3" : "p");
    el.className = k === "heading" ? (b.kind.level === 1 ? "b h1" : "b h") : "b";
    spans(el, b.spans);
  }
  if (b.quote > 0) el.classList.add("q");
  return el;
}

function message(u) {
  const key = `${u.session}/${u.index}`;
  const m = mirror.apply(u);
  let el = nodes.get(key);
  if (!el) {
    el = document.createElement("div");
    el.className = `msg ${u.role}`;
    el.dataset.key = key;
    el.blocks = 0;
    nodes.set(key, el);
    $("chat").append(el);
  }
  // Keep what stays, replace the rest.
  el.querySelectorAll(".cursor, .end").forEach((n) => n.remove());
  while (el.blocks > u.keep) {
    el.lastElementChild.remove();
    el.blocks--;
  }
  for (const b of u.blocks) {
    el.append(block(b));
    el.blocks++;
  }
  if (u.role === "agent") {
    if (u.state === "streaming") {
      const c = document.createElement("span");
      c.className = "cursor";
      (el.lastElementChild ?? el).append(c);
    } else {
      const end = document.createElement("div");
      end.className = "end";
      end.innerHTML = `<span class="sq ${u.state === "done" ? "done" : "needs"}"></span>`;
      end.append(u.state === "done" ? "Done" : "Interrupted · the machine stopped mid-reply");
      el.append(end);
    }
  }
  return m;
}

// ---- Notes from the core ----

function onNotes(notes) {
  const chat = $("chat");
  const atEnd = chat.scrollHeight - chat.scrollTop - chat.clientHeight < 48;
  for (const n of notes) {
    switch (n.type) {
      case "status":
        if (n.status === "offline" && status !== "offline") lostAt = Date.now();
        status = n.status;
        detail = n.detail;
        if (status === "online") $("code-error").hidden = true;
        break;
      case "welcome":
        name = n.name;
        break;
      case "paired":
        name = n.name || name;
        saved = { addr: $("addr").value, key: n.key, name, session };
        store.set("dent.machine", JSON.stringify(saved));
        machine.call((l) => l.follow(session));
        break;
      case "pair_failed": {
        const e = n.error;
        $("code-error").hidden = false;
        $("code-error").textContent = {
          wrong: `That code didn't match. Check the code on ${name} and try again.`,
          wrong_new_code: `That code didn't match three times, so ${name} now shows a new one.`,
          closed: `No code is showing on ${name}. Open Settings › Devices there to show one.`,
          locked: `Too many wrong codes. Try again in ${e.secs} s.`,
        }[e.type] ?? `Couldn't pair: ${e.why ?? e.type}`;
        $("code-input").value = "";
        break;
      }
      case "queued": {
        const el = document.createElement("div");
        el.className = "msg user pending";
        el.textContent = n.text;
        const meta = document.createElement("div");
        meta.className = "meta";
        meta.textContent = status === "online" ? "sending…" : "waiting for the machine";
        el.append(meta);
        $("chat").append(el);
        pending.set(n.id, el);
        break;
      }
      case "accepted":
        pending.get(n.id)?.remove();
        pending.delete(n.id);
        break;
      case "message":
        message(n.update);
        break;
      case "rtt":
        rtt = n.ms;
        break;
      case "health":
        push = n.health.push ?? null;
        break;
    }
  }
  // Pending prompts stay last until the daemon has them.
  for (const el of pending.values()) chat.append(el);
  if (atEnd) chat.scrollTop = chat.scrollHeight;
  render();
}

function connect(addr, known) {
  machine?.stop();
  machine = new Machine(core, { name: "Browser", keys: hex.from(keys), known, addrs: [addr], onNotes });
  if (known) machine.call((l) => l.follow(session));
  machine.start();
  render();
}

$("connect-form").addEventListener("submit", (e) => {
  e.preventDefault();
  const addr = $("addr").value.trim();
  if (addr) connect(addr, null);
});

$("code-form").addEventListener("submit", (e) => {
  e.preventDefault();
  const code = $("code-input").value.replace(/\D/g, "");
  if (code.length === 6) machine.call((l, now) => l.pair(code, now));
});

$("composer").addEventListener("submit", (e) => {
  e.preventDefault();
  const text = $("prompt").value.trim();
  if (!text || !machine) return;
  machine.call((l) => l.prompt(session, text));
  $("prompt").value = "";
});

$("prompt").addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey) {
    e.preventDefault();
    $("composer").requestSubmit();
  }
});

// Reconnecting turns to offline after 10 s without a new status.
setInterval(render, 1000);

$("addr").value = saved?.addr ?? params.get("addr") ?? "";
if (saved) connect(saved.addr, saved.key);
render();
window.dent = { get status() { return status; }, mirror };
