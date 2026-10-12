// The host for the core in JavaScript: it owns the WebSockets and the
// timer, and the core decides everything else. The same file runs in the
// browser page and in the Node test (Node has WebSocket built in).

export class Machine {
  // `core` is the wasm module's exports; `known` is the machine's key (hex)
  // once paired.
  constructor(core, { name, keys, known, addrs, onNotes }) {
    this.link = new core.WebLink(name, keys, known ?? undefined, addrs);
    this.socks = new Map();
    this.onNotes = onNotes;
    this.timer = null;
    this.frame = null;
    this.drainMs = 0;
  }

  start() {
    this.link.start(Date.now());
    this.pump();
  }

  // Run what the core asked for, then arrange the next tick and frame.
  pump() {
    for (const io of this.link.take_io()) {
      if (io.type === "connect") this.open(io.conn, io.addr);
      else if (io.type === "send") {
        const ws = this.socks.get(io.conn);
        if (ws && ws.readyState === 1) ws.send(io.data);
      } else if (io.type === "close") {
        const ws = this.socks.get(io.conn);
        this.socks.delete(io.conn);
        if (ws) ws.close();
      }
    }
    this.schedule();
  }

  open(conn, addr) {
    const ws = new WebSocket(`ws://${addr}/`);
    ws.binaryType = "arraybuffer";
    this.socks.set(conn, ws);
    ws.onopen = () => {
      this.link.opened(conn, Date.now());
      this.pump();
    };
    ws.onmessage = (e) => {
      this.link.received(conn, new Uint8Array(e.data), Date.now());
      this.pump();
    };
    ws.onclose = (e) => {
      if (!this.socks.has(conn)) return;
      this.socks.delete(conn);
      this.link.closed(conn, e.reason || `closed (${e.code})`, Date.now());
      this.pump();
    };
    ws.onerror = () => {};
  }

  schedule() {
    const next = this.link.next_deadline();
    clearTimeout(this.timer);
    if (next !== undefined) {
      const wait = Math.max(0, Math.min(250, next - Date.now()));
      this.timer = setTimeout(() => {
        this.link.tick(Date.now());
        this.pump();
      }, wait);
    }
    // Notes are taken once per frame, however many frames arrived.
    if (this.frame === null) {
      const raf = globalThis.requestAnimationFrame ?? ((f) => setTimeout(f, 16));
      this.frame = raf(() => {
        this.frame = null;
        const t = performance.now();
        const notes = JSON.parse(this.link.take_notes());
        this.drainMs += performance.now() - t;
        if (notes.length) this.onNotes(notes);
      });
    }
  }

  call(f) {
    f(this.link, Date.now());
    this.pump();
  }

  stop() {
    this.link.stop();
    this.pump();
    clearTimeout(this.timer);
  }
}

// Applies message updates the way any UI does: keep `keep` blocks, then
// append the rest.
export class Mirror {
  constructor() {
    this.messages = new Map();
  }
  apply(u) {
    const k = `${u.session}/${u.index}`;
    let m = this.messages.get(k);
    if (!m) {
      m = { session: u.session, index: u.index, role: u.role, state: u.state, blocks: [] };
      this.messages.set(k, m);
    }
    m.state = u.state;
    m.blocks.length = Math.min(m.blocks.length, u.keep);
    for (const b of u.blocks) m.blocks.push(b);
    return m;
  }
  get(session, index) {
    return this.messages.get(`${session}/${index}`);
  }
}

export const hex = {
  to: (b) => Array.from(b, (x) => x.toString(16).padStart(2, "0")).join(""),
  from: (s) => new Uint8Array(s.match(/../g).map((h) => parseInt(h, 16))),
};
