// A stand-in for Apple's push service, for local runs. It checks what a real
// one would (a valid ES256 provider token, the topic, the size) and records
// every push. Tokens starting "dead" are unregistered phones. With
// `simulator` set, each push is handed to the booted iOS simulator with
// `xcrun simctl push`, so the app's notification extension opens it.
//
// It speaks HTTP/1.1; the real service is HTTP/2 only.

import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { execFile } from "node:child_process";

export function start({ publicKey, keyId, teamId, topic, simulator }) {
  const pushes = [];
  const server = http.createServer((req, res) => {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      const send = (status, obj) => {
        res.writeHead(status, { "content-type": "application/json", "apns-id": crypto.randomUUID() });
        res.end(obj ? JSON.stringify(obj) : "");
      };
      const m = req.url.match(/^\/3\/device\/([^/]+)$/);
      if (req.method !== "POST" || !m) return send(404, { reason: "BadPath" });
      const token = m[1];
      const auth = (req.headers.authorization ?? "").replace(/^bearer /, "");
      const [h, p, s] = auth.split(".");
      let claims = null;
      try {
        const head = JSON.parse(Buffer.from(h, "base64url"));
        claims = JSON.parse(Buffer.from(p, "base64url"));
        const ok = crypto.verify("sha256", Buffer.from(`${h}.${p}`), { key: publicKey, dsaEncoding: "ieee-p1363" }, Buffer.from(s, "base64url"));
        if (!ok || head.alg !== "ES256" || head.kid !== keyId || claims.iss !== teamId) throw new Error("bad");
      } catch {
        return send(403, { reason: "InvalidProviderToken" });
      }
      if (req.headers["apns-topic"] !== topic) return send(400, { reason: "TopicDisallowed" });
      if (Buffer.byteLength(body) > 4096) return send(413, { reason: "PayloadTooLarge" });
      const push = { token, headers: req.headers, body: JSON.parse(body), jwt: auth, iat: claims.iat };
      pushes.push(push);
      if (token.startsWith("dead")) return send(410, { reason: "Unregistered", timestamp: Date.now() });
      if (simulator) {
        const file = path.join(os.tmpdir(), `dent-push-${crypto.randomUUID()}.apns`);
        fs.writeFileSync(file, JSON.stringify({ "Simulator Target Bundle": simulator, ...push.body }));
        execFile("xcrun", ["simctl", "push", "booted", simulator, file], () => fs.rmSync(file, { force: true }));
      }
      send(200);
    });
  });
  return new Promise((resolve) =>
    server.listen(0, "127.0.0.1", () => resolve({ server, pushes, origin: `http://127.0.0.1:${server.address().port}` })),
  );
}
