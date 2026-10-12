# 0035. dent-push: notifications as their own small service on Cloudflare Workers

Status: accepted (2026-10-12) · Supersedes [0021](0021-push-apns-from-hub.md) · Spec: [hub.md](../hub.md#push-notifications)

## Decision

Phone notifications come from **dent-push**, a small stateless service on Cloudflare Workers, separate from the hub. It ships with the iPhone app, whether or not the hub is back.

- **Registering:** the phone sends its APNs token to dent-push and gets back a push handle: the token sealed with dent-push's own key (AES-GCM, with a key version). dent-push stores nothing.
- **Pairing:** the phone gives each paired daemon its handle and a notification key, over the existing encrypted link. Daemons never see the raw token.
- **Sending:** the daemon encrypts a short message with the notification key and posts it to dent-push with the handle. dent-push unseals the handle, checks rate limits for it, signs an APNs token (ES256, cached under an hour) and forwards the push. It sees a token and the timing, never the content.
- **Showing:** a notification service extension on the phone decrypts the message. If it can't, it shows "A session on <machine> needs you". Allow and Deny act from the notification and reach the daemon over the direct link.
- **Kinds:**
  - Alerts: needs approval (time-sensitive), finished, failed, limit nearly reached.
  - Live Activity updates.
  - Silent refreshes.
  - Android later goes through Firebase, in the same worker.
- **Limits:** per handle (bursts and a daily cap) and per sending address. A dead token is reported back so the daemon drops it.
- **Code:** TypeScript under `push/`, deployed with wrangler, MIT. Nothing to self-host: only dent can sign pushes for the App Store app. Someone who builds the app themselves can deploy the same worker with their own Apple key.

## Why

Apple accepts pushes only when they're signed with the app's own key, and iOS can't keep a connection open in the background. So whoever publishes the app must run something that sends pushes. Every comparable app works this way. That something doesn't need accounts, sync or a relay. Splitting it out lets the iPhone app have approvals and alerts while the hub stays on hold. Workers fits a stateless service like this: no servers to run, and the cost is near zero at dent's scale (see Consequences).

[0020](0020-hub-single-binary.md) keeps the hub one Rust binary so the hosted and self-hosted versions share a codebase. That reason doesn't apply here, because dent-push is hosted only.

## Consequences

- **Cost:** Workers' free plan allows 100,000 requests a day; the $5 a month plan includes 10 million a month. APNs is free.
  - At around 50 notifications per active person per day, the free plan covers about 2,000 people.
  - The $5 plan covers about 6,000, and each further million requests costs well under a dollar.
  - The per-handle limits cap what any one person or abuser can spend.
- **Price:** free, with fair-use limits per phone (decided 2026-10-12). The paid tier stays for the relay and hub, which carry real bandwidth costs.
- **Running out is always visible, never silent:**
  - A phone over its limit: dent-push answers the daemon with "limited until <time>". The apps show "Phone notifications paused until 9:00, daily limit reached" in Settings › Notifications and once in the session. Everything still shows in the app.
  - dent-push down or over its Cloudflare plan: daemons get an error or no answer, retry with backoff, and the apps show "Phone notifications are unavailable right now".
  - For whoever runs it: Cloudflare usage alerts at 80% of the plan, and the worker counts sends per day, so moving to the $5 plan happens before anyone notices.
- **Needs** an Apple developer account and a Cloudflare account. Keys live in Workers secrets.
- **Built and tested locally** in [spike 07](../spikes/07-client-core.md): [push/](../../push), 15 checks under wrangler against a stand-in for Apple. The daemon's notices travel end to end to the iOS simulator.
- **The per-phone limits cost a Durable Object request per send,** as well as the Worker request. On the free plan both allow 100,000 a day, so the estimate above stands. On the $5 plan, Durable Object requests beyond 1 million a month are $0.15 per million.
- **Unverified:** whether Workers' outbound fetch reaches APNs, which requires HTTP/2. A Workers-compatible APNs client exists, which suggests yes. Checking needs a deployed Worker and an Apple developer account; a tiny VM is the fallback.
- **A leaked handle** lets someone send rate-limited, undecryptable pushes to that one phone until it re-registers. Removing a device in dent deletes its handle on every daemon.
- Desktop clients keep using native OS notifications directly.
