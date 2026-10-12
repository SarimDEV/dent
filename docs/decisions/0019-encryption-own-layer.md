# 0019. Encryption: our own small layer on standard primitives

Status: accepted (2026-10-11) · Spec: [hub.md](../hub.md)

## Decision

One account key wrapped per device, per-field XChaCha20-Poly1305 records, QR or tap approval from an existing device, printed recovery key. The hub stores ciphertext plus minimal metadata.

## Why

No sync framework to outgrow; the hub can never read code or chats.

## Consequences

Search and anything that reads content runs on devices.
