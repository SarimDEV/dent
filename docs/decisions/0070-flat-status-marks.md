# 0070. Status marks are flat shapes, not pixel patterns

Status: accepted (2026-10-22) · Spec: [design-system.md](../design-system.md), [design.md](../design.md) · Narrows: [0006](0006-field-notes-design.md)

## Decision

- Each status keeps a shape of its own, so it reads without its colour, but drawn flat: **needs** a filled square, **done** a filled square with a check cut out of it, **waiting** an outline with a small square inside, **idle** an outline. **Working** still breathes as a small rounded square; with Reduce Motion it holds still as that same small rounded square, its paler fill under the signal, so it never reads as needs' full square.
- No ordered dither anywhere in the app. The unused dissolve (`anim::dissolve`) is gone, and the Resolve motion with it.

## Why

The owner doesn't like dither at all (said 15 Oct while choosing backgrounds), and the 4×4 pixel marks were the last of it. The done mark, a stair of green cells, read as a bar-chart icon in the 21 Oct design audit, and the waiting mark (four dots) as noise. The shapes still differ for colour-blind eyes, which is what the patterns were for.

## Consequences

- The signals' partner colours stay: the working square breathes from its partner to the signal.
- Status squares draw one or two quads (plus the check's glyph) instead of up to sixteen.
