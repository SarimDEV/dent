//! Tally: tool calls counted by hand in gates of five, one row per hour
//! label, the way a field count is kept. Calls are queued and drawn at a
//! steady hand's pace, so a burst never smears; the page rolls up smoothly
//! as rows fill.

use crate::math::{ease, hash};
use crate::scene::{Align, Font};
use crate::{Ambient, Ctx, DrawCtx, Scene, Spec};

pub const SPEC: Spec = Spec {
    id: "tally",
    name: "Tally",
    from: "Hand tallies in gates of five, one row per hour, the way a field count is kept.",
    shows: "Tool calls across your sessions, counted by hand as they happen, with your typing and the sessions you start. The newest stroke is drawn in the working colour, and the page rolls up smoothly as rows fill.",
    make: |seed| Box::new(Tally::new(seed)),
};

/// Row pitch (pt).
const ROW_H: f32 = 40.;
/// Top of the first row (pt).
const TOP: f32 = 44.;
/// Left edge of the first gate (pt).
const LEFT: f32 = 64.;
/// Width of one gate of five (pt).
const GATE_W: f32 = 48.;
/// Space kept right of the last gate (pt).
const RIGHT: f32 = 60.;
/// The quickest the hand draws a new stroke (seconds between strokes).
const GAP: f32 = 0.34;
/// Marks waiting to be drawn; anything beyond is a flood a tally can't
/// keep up with, and is let go.
const MAX_QUEUE: u32 = 24;
/// Keys typed per mark.
const KEYS_PER_MARK: u32 = 6;
/// How long the newest stroke takes to draw (seconds).
const GROW: f32 = 0.3;
/// How long it stays in the working colour (seconds).
const FRESH: f32 = 1.5;
/// Row scrolling rate (per second).
const SCROLL: f32 = 6.;
/// Stroke width (pt).
const STROKE: f32 = 1.5;

struct Layout {
    per: usize,
    vis: usize,
}

fn layout(w: f32, h: f32) -> Layout {
    let per = ((w - LEFT - RIGHT) / GATE_W).floor().max(1.) as usize;
    let vis = ((h - TOP - 8.) / ROW_H).floor().max(1.) as usize;
    Layout { per, vis }
}

struct Tally {
    seed: i32,
    count: usize,
    queue: u32,
    keys: u32,
    last: f32,
    /// Rows scrolled, smoothly.
    sr: Option<f32>,
    /// Hour of the first row.
    hour0: Option<u32>,
    project: String,
}

impl Tally {
    fn new(seed: u64) -> Self {
        Tally {
            seed: (seed % 997) as i32,
            count: 0,
            queue: 0,
            keys: 0,
            last: f32::NEG_INFINITY,
            sr: None,
            hour0: None,
            project: String::new(),
        }
    }

    fn enqueue(&mut self, n: usize) {
        self.queue = (self.queue as usize + n).min(MAX_QUEUE as usize) as u32;
    }
}

impl Ambient for Tally {
    fn update(&mut self, cx: &Ctx) {
        let s = cx.signals;
        if let Some(p) = &s.project
            && *p != self.project
        {
            self.project.clone_from(p);
        }
        if self.hour0.is_none() {
            self.hour0 = Some(s.minute.map_or(9, |m| (m.rem_euclid(1440.) / 60.) as u32));
        }
        self.enqueue(s.calls.len());
        self.keys += cx.input.keys;
        let marks = self.keys / KEYS_PER_MARK;
        self.keys %= KEYS_PER_MARK;
        self.enqueue(marks as usize + cx.input.submit as usize);
        if self.queue > 0 && cx.t - self.last >= GAP {
            self.queue -= 1;
            self.count += 1;
            self.last = cx.t;
        }
        let l = layout(cx.size.w, cx.size.h);
        let need = self.count.div_ceil(l.per * 5);
        let target = need.saturating_sub(l.vis) as f32;
        self.sr = Some(match self.sr {
            None => target,
            Some(sr) => sr + (target - sr) * ease(SCROLL, cx.dt),
        });
    }

    fn draw(&self, cx: &DrawCtx, out: &mut Scene) {
        let (w, h, p) = (cx.size.w, cx.size.h, cx.pal);
        let l = layout(w, h);
        let per_m = l.per * 5;
        let sr = self.sr.unwrap_or(0.);
        let first = sr.floor().max(0.) as usize;
        let header = if self.project.is_empty() {
            "TOOL CALLS".to_string()
        } else {
            format!("TOOL CALLS · {}", self.project)
        };
        out.text([LEFT, 26.], header, Font::Mono, 10., p.soft, Align::Left);

        if w < LEFT + GATE_W || h < TOP {
            // No room for even one gate.
            return;
        }
        out.clip(0., TOP - 6., w, h - TOP + 6.);
        let row_y = |r: usize| TOP + (r as f32 - sr) * ROW_H;
        let mut rules = Vec::new();
        let mut r = first;
        while row_y(r) < h {
            let y = (row_y(r) + 34.).round() + 0.5;
            rules.extend([[0., y], [w, y]]);
            r += 1;
        }
        out.segments(rules, 1., p.rule);

        let need = self.count.div_ceil(per_m);
        let hour0 = self.hour0.unwrap_or(9);
        let mut r = first;
        while r < need && row_y(r) < h {
            let hr = (hour0 as usize + r) % 24;
            out.text(
                [LEFT - 14., row_y(r) + 18.],
                format!("{hr:02}h"),
                Font::Mono,
                10.,
                p.soft,
                Align::Right,
            );
            r += 1;
        }

        let age = cx.t - self.last;
        let mut old = Vec::new();
        for i in first * per_m..self.count {
            let (g, k) = (i / 5, i % 5);
            let (row, col) = (g / l.per, g % l.per);
            let (gx, gy) = (LEFT + col as f32 * GATE_W, row_y(row));
            if gy > h {
                break;
            }
            let hs = |s: i32| hash(i as i32, s, self.seed) - 0.5;
            let (a, b) = if k < 4 {
                let x0 = gx + k as f32 * 7. + hs(1) * 2.4;
                ([x0, gy + 1.], [x0 + hs(2) * 3., gy + 25.])
            } else {
                ([gx - 4., gy + 20. + hs(3) * 3.], [gx + 28., gy + 5.])
            };
            if i + 1 == self.count {
                let grow = (age / GROW).clamp(0., 1.);
                let end = [a[0] + (b[0] - a[0]) * grow, a[1] + (b[1] - a[1]) * grow];
                let color = if age < FRESH { p.accent } else { p.pencil };
                if grow > 0. {
                    out.line(vec![a, end], STROKE, color);
                }
            } else {
                old.extend([a, b]);
            }
        }
        out.segments(old, STROKE, p.pencil);

        let lg = self.count.saturating_sub(1) / 5;
        let col = if self.count == 0 { 0 } else { lg % l.per + 1 };
        let x = LEFT + col as f32 * GATE_W + 2.;
        out.text(
            [x, row_y(lg / l.per) + 18.],
            format!("= {}", self.count),
            Font::Mono,
            11.,
            p.soft,
            Align::Left,
        );
        out.unclip();
    }
}
