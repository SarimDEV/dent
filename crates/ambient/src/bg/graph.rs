//! Graph paper: activity plotted like a chart recorder on Field Notes'
//! 3/16 in ruling. The pen sits at the right and glides to each new value;
//! committed points slide left continuously, so nothing ever jumps.

use std::collections::VecDeque;

use crate::math::{clamp, ease, noise3};
use crate::scene::{Align, Font, Paint};
use crate::{Ambient, Ctx, DrawCtx, Scene, Spec};

pub const SPEC: Spec = Spec {
    id: "graph",
    name: "Graph paper",
    from: "Field Notes graph ruling, 3/16 in squares in light brown ink, plotted like a chart recorder.",
    shows: "Tool calls across your sessions, plotted live on the page's own grid. A circled point marks each session you start.",
    make: |seed| Box::new(Graph::new(seed)),
};

/// Grid pitch (pt): 3/16 in.
const G: f32 = 18.;
/// Seconds per plotted point.
const STEP: f32 = 0.5;
/// Points kept; more than any screen shows.
const KEEP: usize = 400;
/// Session marks sit at least this many points apart.
const MARK_GAP: usize = 8;

struct Graph {
    seed: f32,
    pts: VecDeque<f32>,
    /// Index of the next point to commit.
    n: usize,
    next: f32,
    frac: f32,
    act: f32,
    head: f32,
    pending: bool,
    marks: VecDeque<usize>,
}

impl Graph {
    fn new(seed: u64) -> Self {
        let mut g = Graph {
            seed: (seed % 97) as f32,
            pts: VecDeque::new(),
            n: 0,
            next: STEP,
            frac: 0.,
            act: 0.,
            head: 0.,
            pending: false,
            marks: VecDeque::new(),
        };
        for _ in 0..160 {
            let v = g.base(g.n);
            g.pts.push_back(v);
            g.n += 1;
        }
        g.head = g.base(g.n);
        g
    }

    /// A quiet baseline so an idle plot still breathes a little.
    fn base(&self, i: usize) -> f32 {
        let i = i as f32;
        0.08 + 0.1 * (noise3(i * 0.09, 3.1, self.seed) + 1.) / 2.
            + 0.05 * (noise3(i * 0.42, 7.7, self.seed) + 1.) / 2.
    }
}

impl Ambient for Graph {
    fn update(&mut self, cx: &Ctx) {
        let calls = cx.signals.calls.len().min(20) as f32;
        self.act = clamp(
            self.act * (-cx.dt * 1.2).exp() + calls * 0.05 + cx.input.keys as f32 * 0.04,
            0.,
            1.,
        );
        if cx.input.submit || cx.signals.started > 0 {
            self.act = clamp(self.act + 0.45, 0., 1.);
            self.pending = true;
        }
        let target = clamp(self.base(self.n) + self.act.min(0.7), 0., 0.96);
        self.head += (target - self.head) * ease(10., cx.dt);
        while cx.t >= self.next {
            self.next += STEP;
            self.pts.push_back(self.head);
            if self.pending {
                if self.marks.back().is_none_or(|&m| self.n - m >= MARK_GAP) {
                    self.marks.push_back(self.n);
                }
                self.pending = false;
            }
            self.n += 1;
            if self.pts.len() > KEEP {
                self.pts.pop_front();
            }
        }
        while self.marks.front().is_some_and(|&m| m + KEEP < self.n) {
            self.marks.pop_front();
        }
        self.frac = clamp(1. - (self.next - cx.t) / STEP, 0., 1.);
    }

    fn draw(&self, cx: &DrawCtx, out: &mut Scene) {
        let (w, h, p) = (cx.size.w, cx.size.h, cx.pal);
        // The ruling.
        let mut grid = Vec::new();
        let mut x = G;
        while x < w {
            grid.extend([[x.round() + 0.5, 0.], [x.round() + 0.5, h]]);
            x += G;
        }
        let mut y = G;
        while y < h {
            grid.extend([[0., y.round() + 0.5], [w, y.round() + 0.5]]);
            y += G;
        }
        out.segments(grid, 1., p.rule);

        let x0 = G * 2.;
        let y0 = ((h - G * 1.5) / G).floor() * G;
        let ph = (y0 - G * 2.).max(20.);
        let xr = x0 + ((w - x0 - G * 2.) / G).floor().max(2.) * G;
        out.line(
            vec![[x0 + 0.5, G], [x0 + 0.5, y0 + 0.5], [w, y0 + 0.5]],
            1.,
            p.rule2,
        );
        out.text(
            [x0 + 6., G + 12.],
            "TOOL CALLS / MIN",
            Font::Mono,
            10.,
            p.soft,
            Align::Left,
        );

        // Committed point `idx` sits this far left of the pen.
        let px = |idx: usize| xr - ((self.n - idx) as f32 - 1. + self.frac) * G;
        let base = self.n - self.pts.len();
        let vis: Vec<(f32, f32, usize)> = self
            .pts
            .iter()
            .enumerate()
            .map(|(j, v)| (px(base + j), y0 - v * ph, base + j))
            .filter(|&(x, _, _)| x > x0 - G)
            .collect();
        let (hx, hy) = (xr, y0 - self.head * ph);

        out.clip(x0, 0., w - x0, h);
        for &(x, _, idx) in &vis {
            if idx % 10 == 0 {
                let secs = ((self.n - idx) as f32 - 1. + self.frac) * STEP + STEP;
                out.text(
                    [x, y0 + 14.],
                    format!("−{secs:.0}s"),
                    Font::Mono,
                    10.,
                    p.soft,
                    Align::Center,
                );
            }
        }
        let mut line: Vec<[f32; 2]> = vis.iter().map(|&(x, y, _)| [x, y]).collect();
        line.push([hx, hy]);
        out.line(line, 1.25, p.pencil);
        out.dots(
            3.,
            p.pencil,
            vis.iter().map(|&(x, y, _)| [x - 1.5, y - 1.5]).collect(),
        );
        for &(x, y, idx) in &vis {
            if self.marks.contains(&idx) {
                out.circle([x, y], 9., None, Some((1.5, p.accent)));
                out.text(
                    [x + 12., y - 10.],
                    "session",
                    Font::Mono,
                    10.,
                    p.accent,
                    Align::Left,
                );
            }
        }
        out.unclip();
        out.rect(hx - 3., hy - 3., 6., 6., Paint::Solid(p.accent));
    }
}
