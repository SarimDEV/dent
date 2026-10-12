# 0027. Project docs folder at ~/dent/projects/<project>/docs

Status: accepted (2026-10-11) · Spec: [projects.md](../projects.md)

## Decision

Each project has a docs folder at `~/dent/projects/<project>/docs` on every machine hosting it, synced per file, latest edit wins, conflicts kept as sibling copies.

## Why

Agents on any machine share notes across repos.

## Consequences

Session system prompts can point agents at it.
