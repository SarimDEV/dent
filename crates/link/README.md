# dent-link

How a device and a daemon talk across the network ([decision 0060](../../docs/decisions/0060-remote-wire.md)):

- **Handshake:** Noise XX between device keys. The initiator checks the responder's key before revealing its own, and each side's protocol range rides in the encrypted handshake payloads.
- **Frames:** one data message at a time is split into chunks of at most 64 KiB, and control messages (ping, goodbye, refusals, pairing) go between chunks, never behind them.
- **Pairing:** a 6-digit code through SPAKE2, bound to the channel by an HMAC of the handshake hash.

No I/O and no runtime: callers move the bytes. The daemon wraps it in a WebSocket; the client core drives it from its host.
