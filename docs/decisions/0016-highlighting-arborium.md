# 0016. Highlighting: arborium tree-sitter grammars, tokens in the block tree

Status: accepted (2026-10-11) · Spec: [durability.md](../durability.md)

## Decision

Syntax highlighting with arborium's tree-sitter grammars; tokens are carried in the block tree.

## Why

iPhone and web need no highlighter of their own.

## Consequences

Fallback is plain monospace.

For now the desktop highlights at render time (arborium 2.18.2, twenty grammars compiled in, each block cached); the tokens move into the block tree when the client core parses Markdown for the phone.
