// dent-push: phone notifications for dent (decision 0035).
//
// A phone registers its APNs token and gets back a push handle: the token
// sealed with our key. It gives the handle, and a key of its own, to each
// machine it pairs with. A machine seals each notice with the phone's key
// and posts it here with the handle; we unseal the handle, check the
// phone's limits and forward to Apple. We never see the text, and we store
// nothing but a counter per phone.

import { DurableObject } from "cloudflare:workers";

export interface Env {
  /** {"1": base64 AES-256 key, ...}; old versions keep opening old handles. */
  SEAL_KEYS: string;
  SEAL_CURRENT: string;
  /** The .p8 key from Apple (PKCS#8 PEM). */
  APNS_KEY: string;
  APNS_KEY_ID: string;
  APNS_TEAM_ID: string;
  APNS_TOPIC: string;
  /** Overrides for tests: where "Apple" is. */
  APNS_ORIGIN_SANDBOX?: string;
  APNS_ORIGIN_PRODUCTION?: string;
  /** Per phone: notices per minute, and per day (UTC). */
  BURST: string;
  DAILY: string;
  /** Development only: accept "sim-…" tokens, and an X-Test-Now clock. */
  ALLOW_FAKE_TOKENS?: string;
  TEST_CLOCK?: string;
  LIMITER: DurableObjectNamespace<Limiter>;
  PER_IP?: RateLimit;
}

/** What a sealed notice can be, and how Apple should treat it. */
const KINDS: Record<string, { fallback: string; level?: string; type: "alert" | "background"; priority: number }> = {
  approval: { fallback: "A session needs your approval", level: "time-sensitive", type: "alert", priority: 10 },
  finished: { fallback: "A session finished", level: "active", type: "alert", priority: 10 },
  failed: { fallback: "A session stopped with an error", level: "active", type: "alert", priority: 10 },
  limit: { fallback: "A usage limit is nearly reached", level: "active", type: "alert", priority: 10 },
  refresh: { fallback: "", type: "background", priority: 5 },
};

/** APNs allows 4 KB per push; the sealed notice must leave room for aps. */
const MAX_PAYLOAD = 3000;
const MAX_BODY = 8192;

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

// ---- base64 ----

const b64 = (b: Uint8Array) => btoa(String.fromCharCode(...b));
const unb64 = (s: string) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));
const b64url = (b: Uint8Array) => b64(b).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
const unb64url = (s: string) => unb64(s.replace(/-/g, "+").replace(/_/g, "/") + "===".slice((s.length + 3) % 4));

// ---- Handles: AES-GCM over the token, with the key's version in front ----

interface Sealed {
  t: string;
  e: "sandbox" | "production";
}

const keyCache = new Map<string, CryptoKey>();

async function sealKey(env: Env, version: string): Promise<CryptoKey | null> {
  const raw = (JSON.parse(env.SEAL_KEYS) as Record<string, string>)[version];
  if (!raw) return null;
  const cacheKey = `${version}:${raw}`;
  let k = keyCache.get(cacheKey);
  if (!k) {
    k = await crypto.subtle.importKey("raw", unb64(raw), "AES-GCM", false, ["encrypt", "decrypt"]);
    keyCache.set(cacheKey, k);
  }
  return k;
}

async function seal(env: Env, s: Sealed): Promise<string> {
  const v = Number(env.SEAL_CURRENT);
  const key = await sealKey(env, env.SEAL_CURRENT);
  if (!key || !(v > 0 && v < 256)) throw new Error("no sealing key");
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const ct = new Uint8Array(await crypto.subtle.encrypt({ name: "AES-GCM", iv }, key, new TextEncoder().encode(JSON.stringify(s))));
  const out = new Uint8Array(1 + 12 + ct.length);
  out[0] = v;
  out.set(iv, 1);
  out.set(ct, 13);
  return `h1.${b64url(out)}`;
}

async function unseal(env: Env, handle: string): Promise<Sealed | null> {
  if (typeof handle !== "string" || !handle.startsWith("h1.") || handle.length > 1024) return null;
  try {
    const raw = unb64url(handle.slice(3));
    if (raw.length < 1 + 12 + 16) return null;
    const key = await sealKey(env, String(raw[0]));
    if (!key) return null;
    const plain = await crypto.subtle.decrypt({ name: "AES-GCM", iv: raw.slice(1, 13) }, key, raw.slice(13));
    return JSON.parse(new TextDecoder().decode(plain)) as Sealed;
  } catch {
    return null;
  }
}

// ---- The APNs provider token: ES256, reused for 50 minutes ----

let jwt: { token: string; at: number; kid: string } | null = null;

async function providerToken(env: Env, now: number): Promise<string> {
  if (jwt && jwt.kid === env.APNS_KEY_ID && now - jwt.at < 50 * 60_000) return jwt.token;
  const der = unb64(env.APNS_KEY.replace(/-----[^-]+-----/g, "").replace(/\s+/g, ""));
  const key = await crypto.subtle.importKey("pkcs8", der, { name: "ECDSA", namedCurve: "P-256" }, false, ["sign"]);
  const enc = (o: unknown) => b64url(new TextEncoder().encode(JSON.stringify(o)));
  const head = `${enc({ alg: "ES256", kid: env.APNS_KEY_ID })}.${enc({ iss: env.APNS_TEAM_ID, iat: Math.floor(now / 1000) })}`;
  const sig = new Uint8Array(await crypto.subtle.sign({ name: "ECDSA", hash: "SHA-256" }, key, new TextEncoder().encode(head)));
  jwt = { token: `${head}.${b64url(sig)}`, at: now, kid: env.APNS_KEY_ID };
  return jwt.token;
}

// ---- Limits per phone: a token bucket per minute and a count per day ----

interface Count {
  day: number;
  count: number;
  tokens: number;
  at: number;
}

export class Limiter extends DurableObject {
  async take(burst: number, daily: number, now: number): Promise<{ ok: boolean; until?: number; why?: string }> {
    const day = Math.floor(now / 86_400_000);
    const s = (await this.ctx.storage.get<Count>("c")) ?? { day, count: 0, tokens: burst, at: now };
    if (s.day !== day) {
      s.day = day;
      s.count = 0;
    }
    s.tokens = Math.min(burst, s.tokens + ((now - s.at) * burst) / 60_000);
    s.at = now;
    if (s.count >= daily) return { ok: false, until: (day + 1) * 86_400_000, why: "daily" };
    if (s.tokens < 1) {
      await this.ctx.storage.put("c", s);
      return { ok: false, until: now + Math.ceil(((1 - s.tokens) * 60_000) / burst), why: "burst" };
    }
    s.tokens -= 1;
    s.count += 1;
    await this.ctx.storage.put("c", s);
    return { ok: true };
  }
}

async function phoneId(token: string): Promise<string> {
  const h = new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(token)));
  return b64url(h);
}

// ---- Routes ----

async function register(req: Request, env: Env): Promise<Response> {
  const body = (await readJson(req)) as { token?: unknown; env?: unknown } | null;
  if (!body) return json(400, { error: "bad json" });
  const token = body.token;
  const e = body.env === "production" ? "production" : body.env === "sandbox" ? "sandbox" : null;
  const real = typeof token === "string" && /^[0-9a-f]{64,200}$/.test(token);
  const fake = typeof token === "string" && env.ALLOW_FAKE_TOKENS === "1" && /^sim-[\w-]{1,64}$/.test(token);
  if (!(real || fake) || !e) return json(400, { error: "bad token" });
  return json(200, { handle: await seal(env, { t: token as string, e }) });
}

async function send(req: Request, env: Env, now: number): Promise<Response> {
  const body = (await readJson(req)) as { handle?: unknown; kind?: unknown; payload?: unknown; collapse?: unknown } | null;
  if (!body) return json(400, { error: "bad json" });
  const kind = typeof body.kind === "string" ? KINDS[body.kind] : undefined;
  if (!kind) return json(400, { error: "bad kind" });
  if (typeof body.payload !== "string" || !/^[A-Za-z0-9+/=]*$/.test(body.payload)) return json(400, { error: "bad payload" });
  if (body.payload.length > MAX_PAYLOAD) return json(413, { error: "payload too large", max: MAX_PAYLOAD });
  const sealed = await unseal(env, body.handle as string);
  if (!sealed) return json(400, { error: "bad handle" });

  const limiter = env.LIMITER.get(env.LIMITER.idFromName(await phoneId(sealed.t)));
  const r = await limiter.take(Number(env.BURST) || 30, Number(env.DAILY) || 500, now);
  if (!r.ok) return json(429, { error: "limited", until: r.until, why: r.why });

  const aps: Record<string, unknown> =
    kind.type === "background"
      ? { "content-available": 1 }
      : { alert: { title: "dent", body: kind.fallback }, "mutable-content": 1, sound: "default", "interruption-level": kind.level };
  const collapse = typeof body.collapse === "string" ? body.collapse.slice(0, 64) : "";
  if (collapse && kind.type === "alert") aps["thread-id"] = collapse;
  const push = JSON.stringify({ aps, e: body.payload });

  const origin =
    sealed.e === "production"
      ? env.APNS_ORIGIN_PRODUCTION ?? "https://api.push.apple.com"
      : env.APNS_ORIGIN_SANDBOX ?? "https://api.sandbox.push.apple.com";
  for (let attempt = 0; attempt < 2; attempt++) {
    const headers: Record<string, string> = {
      authorization: `bearer ${await providerToken(env, now)}`,
      "apns-topic": env.APNS_TOPIC,
      "apns-push-type": kind.type,
      "apns-priority": String(kind.priority),
      "content-type": "application/json",
    };
    if (collapse) headers["apns-collapse-id"] = collapse;
    let res: Response;
    try {
      res = await fetch(`${origin}/3/device/${sealed.t}`, { method: "POST", headers, body: push });
    } catch (e) {
      return json(502, { error: "apns unreachable", detail: String(e) });
    }
    if (res.ok) return json(200, { ok: true, id: res.headers.get("apns-id") });
    const reason = ((await res.json().catch(() => ({}))) as { reason?: string }).reason ?? "";
    if (res.status === 410 || ["BadDeviceToken", "Unregistered", "DeviceTokenNotForTopic"].includes(reason)) {
      return json(410, { error: "dead", reason });
    }
    // A rejected provider token: make a new one and try once more.
    if (res.status === 403 && attempt === 0 && /ProviderToken/.test(reason)) {
      jwt = null;
      continue;
    }
    if (res.status === 429) return json(503, { error: "apns busy", reason });
    return json(502, { error: "apns", status: res.status, reason });
  }
  return json(502, { error: "apns", reason: "provider token rejected twice" });
}

async function readJson(req: Request): Promise<unknown> {
  const len = Number(req.headers.get("content-length") ?? 0);
  if (len > MAX_BODY) return null;
  const text = await req.text();
  if (text.length > MAX_BODY) return null;
  try {
    const v = JSON.parse(text);
    return v && typeof v === "object" ? v : null;
  } catch {
    return null;
  }
}

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    const url = new URL(req.url);
    const routes: Record<string, true> = { "/register": true, "/send": true };
    if (url.pathname === "/health") return json(200, { ok: true });
    if (!routes[url.pathname]) return json(404, { error: "not found" });
    if (req.method !== "POST") return json(405, { error: "POST only" });
    const now = env.TEST_CLOCK === "1" && req.headers.get("x-test-now") ? Number(req.headers.get("x-test-now")) : Date.now();
    if (env.PER_IP) {
      const ip = req.headers.get("cf-connecting-ip") ?? "local";
      const { success } = await env.PER_IP.limit({ key: ip });
      if (!success) return json(429, { error: "limited", until: now + 60_000, why: "address" });
    }
    return url.pathname === "/register" ? register(req, env) : send(req, env, now);
  },
};
