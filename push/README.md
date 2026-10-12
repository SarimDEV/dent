# dent-push

Phone notifications for dent ([decision 0035](../docs/decisions/0035-dent-push.md)). A Cloudflare Worker that forwards notices, sealed end to end, to Apple's push service. It stores nothing except a counter per phone for its limits. MIT.

```sh
npm install
npm test                          # under wrangler dev, against a stand-in for Apple
node test/run.mjs --serve         # keep it running locally, forwarding to the iOS simulator
npm run check                     # TypeScript
```

- `POST /register {token, env}` → `{handle}`: the APNs token sealed with our key.
- `POST /send {handle, kind, payload, collapse?}`: `payload` is the notice sealed with the phone's own key (base64, up to 3,000 characters); `kind` is `approval`, `finished`, `failed`, `limit` or `refresh`. Answers 200, 429 `{until}` when the phone is over its limit, 410 when the phone is gone.

Secrets (`wrangler secret put`, or `.dev.vars` locally, never committed): `SEAL_KEYS`, `SEAL_CURRENT`, `APNS_KEY`, `APNS_KEY_ID`, `APNS_TEAM_ID`. `ALLOW_FAKE_TOKENS` and `TEST_CLOCK` are for local runs only and must stay unset in production.
