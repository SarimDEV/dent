//! Spectrograph: a waterfall of the agent's work, one row per kind of tool
//! call (violet to orange) plus a row for your typing. Columns commit at a
//! steady rate and the strip slides in sub-pixel from the right, so time
//! reads back continuously. Each row is a soft Gaussian band built from
//! gradient rects, never pixels.

use std::collections::VecDeque;

use crate::math::{clamp, noise3};
use crate::scene::{Align, Font, Paint};
use crate::{Ambient, Ctx, DrawCtx, Scene, Spec};

pub const SPEC: Spec = Spec {
    id: "spectro",
    name: "Spectrograph",
    from: "A spectrograph's waterfall: one row per wavelength, time running left, brightness for intensity.",
    shows: "Tool calls across your sessions as a strip you can read back: one row per kind of work, brighter when there's more of it. Your own typing shows on the bottom row.",
    make: |seed| Box::new(Spectro::new(seed)),
};

const LABELS: [&str; ROWS] = ["think", "read", "search", "edit", "bash", "web", "you"];
const ROWS: usize = 7;
const YOU: usize = 6;
/// Seconds per column.
const STEP: f32 = 0.25;
/// Column width (pt).
const CW: f32 = 8.;
/// Columns kept; more than any screen shows.
const KEEP: usize = 420;
/// Columns already on the strip when it starts (a faint, quiet history).
const PREFILL: usize = 240;
/// Space above and below the strip, and for the labels on the right (pt).
const TOP: f32 = 30.;
const BOT: f32 = 26.;
const LAB: f32 = 76.;
/// Level one tool call adds to its row.
const PER_CALL: f32 = 0.5;
/// Row levels decay at this rate (per second), short enough that rows
/// separate, long enough that a single call shows.
const DECAY: f32 = 3.;
/// Level per key typed, the cap on it, and how fast it fades (per second).
const PER_KEY: f32 = 0.2;
const YOU_CAP: f32 = 1.2;
const YOU_DECAY: f32 = 3.;
/// A session started: added to every agent row, shrinking per column.
const BURST: f32 = 0.6;
const BURST_KEEP: f32 = 0.8;
/// Gaussian width across a row (in row heights squared) and gain.
const SIGMA2: f32 = 0.07;
const GAIN: f32 = 1.15;
/// Half-height of a row's flat core (in row heights); fringes fill the rest.
const CORE: f32 = 0.1;
/// Rows shorter than this (pt) go unlabelled.
const LABEL_MIN: f32 = 11.;
/// Cells dimmer than this are skipped.
const FAINT: f32 = 0.004;

struct Spectro {
    seed: f32,
    cols: VecDeque<[f32; ROWS]>,
    /// Index of the next column to commit.
    n: usize,
    next: Option<f32>,
    frac: f32,
    level: [f32; ROWS],
    /// Highest level reached during the column being gathered.
    peak: [f32; ROWS],
    burst: f32,
}

impl Spectro {
    fn new(seed: u64) -> Self {
        let mut s = Spectro {
            seed: (seed % 17) as f32,
            cols: VecDeque::with_capacity(KEEP + 1),
            n: 0,
            next: None,
            frac: 0.,
            level: [0.; ROWS],
            peak: [0.; ROWS],
            burst: 0.,
        };
        for _ in 0..PREFILL {
            let col = std::array::from_fn(|i| if i == YOU { 0. } else { s.floor(i, s.n) });
            s.cols.push_back(col);
            s.n += 1;
        }
        s
    }

    /// A faint drifting floor so a quiet strip still breathes.
    fn floor(&self, i: usize, n: usize) -> f32 {
        let n = n as f32;
        let base = 0.5 + 0.5 * noise3(n * 0.05, i as f32 * 4.7, self.seed);
        let spike = noise3(n * 0.22, i as f32 * 9.1, 5.);
        (base * 0.12 + spike * 0.08 - 0.03).max(0.)
    }
}

impl Ambient for Spectro {
    fn update(&mut self, cx: &Ctx) {
        let next = *self.next.get_or_insert(cx.t + STEP);
        let fall = (-cx.dt * DECAY).exp();
        for l in &mut self.level[..YOU] {
            *l *= fall;
        }
        for &k in &cx.signals.calls {
            let l = &mut self.level[k as usize];
            *l = (*l + PER_CALL).min(2.);
        }
        let you = &mut self.level[YOU];
        *you = (*you * (-cx.dt * YOU_DECAY).exp() + cx.input.keys as f32 * PER_KEY).min(YOU_CAP);
        if cx.input.submit {
            *you = YOU_CAP;
        }
        if cx.signals.started > 0 {
            self.burst = 1.;
        }
        for (p, l) in self.peak.iter_mut().zip(self.level) {
            *p = p.max(l);
        }
        let mut next = next;
        while cx.t >= next {
            next += STEP;
            let col: [f32; ROWS] = std::array::from_fn(|i| {
                let v = if i == YOU {
                    self.peak[i]
                } else {
                    self.floor(i, self.n) + self.peak[i] + self.burst * BURST
                };
                v.min(1.)
            });
            self.cols.push_back(col);
            self.burst *= BURST_KEEP;
            self.peak = self.level;
            self.n += 1;
            if self.cols.len() > KEEP {
                self.cols.pop_front();
            }
        }
        self.next = Some(next);
        self.frac = clamp(1. - (next - cx.t) / STEP, 0., 1.);
    }

    fn draw(&self, cx: &DrawCtx, out: &mut Scene) {
        let (w, h, p) = (cx.size.w, cx.size.h, cx.pal);
        let right = (w - LAB).max(1.);
        let bh = ((h - TOP - BOT) / ROWS as f32).max(1.);
        let colour = |i: usize| if i < 6 { p.spec[i] } else { p.pencil };

        let mut rules = Vec::new();
        for i in 0..=ROWS {
            let y = (TOP + i as f32 * bh).round() + 0.5;
            rules.extend([[0., y], [right, y]]);
        }
        out.segments(rules, 1., p.rule);

        // Time axis: a tick every 2 s, labelled every 10 s.
        let mut k = 0;
        while right - k as f32 * 8. * CW > 10. {
            let x = (right - k as f32 * 8. * CW).round();
            if k % 5 == 0 {
                let t = if k == 0 {
                    "now".to_string()
                } else {
                    format!("−{} s", k * 2)
                };
                out.text(
                    [clamp(x, 14., w), TOP - 10.],
                    t,
                    Font::Mono,
                    10.,
                    p.soft,
                    Align::Center,
                );
                out.rect(x, TOP - 6., 1., 6., Paint::Solid(p.soft));
            } else {
                out.rect(x, TOP - 3., 1., 3., Paint::Solid(p.soft));
            }
            k += 1;
        }

        // The strip. Column j's centre; the newest slides in from the right.
        let n = ((right / CW).floor() as usize + 3).min(self.cols.len());
        let skip = self.cols.len() - n;
        let cxj = |j: usize| right - self.frac * CW - (n - 1 - j) as f32 * CW + CW / 2.;
        let a_core = |v: f32| (v * GAIN).min(1.);
        let edge = (-(CORE * CORE) / SIGMA2).exp();
        out.clip(0., TOP, right, h - TOP - BOT);
        for i in 0..ROWS {
            let c = colour(i);
            let yc = TOP + (i as f32 + 0.5) * bh;
            let (core_h, fringe_h) = (2. * CORE * bh, (0.5 - CORE) * bh);
            let v_at = |j: usize| self.cols[skip + j][i];
            for j in 0..n {
                let v = v_at(j);
                let x = cxj(j);
                if v > FAINT {
                    // Fringes: from the core's edge value fading to nothing.
                    let a = (v * GAIN * edge).min(1.);
                    let (x0, ww) = (x - CW / 2., CW);
                    let top = yc - CORE * bh;
                    out.rect(
                        x0,
                        top - fringe_h,
                        ww,
                        fringe_h,
                        Paint::Linear {
                            from: [x, top - fringe_h],
                            to: [x, top],
                            a: c.alpha(0.),
                            b: c.alpha(a),
                        },
                    );
                    let bot = yc + CORE * bh;
                    out.rect(
                        x0,
                        bot,
                        ww,
                        fringe_h,
                        Paint::Linear {
                            from: [x, bot],
                            to: [x, bot + fringe_h],
                            a: c.alpha(a),
                            b: c.alpha(0.),
                        },
                    );
                }
                // Core: centre to centre, blending into the next column.
                let vn = if j + 1 < n { v_at(j + 1) } else { v };
                if v > FAINT || vn > FAINT {
                    let x1 = if j + 1 < n { cxj(j + 1) } else { x + CW / 2. };
                    let x0 = if j == 0 { x - CW / 2. } else { x };
                    out.rect(
                        x0,
                        yc - CORE * bh,
                        x1 - x0,
                        core_h,
                        Paint::Linear {
                            from: [x, yc],
                            to: [x1, yc],
                            a: c.alpha(a_core(v)),
                            b: c.alpha(a_core(vn)),
                        },
                    );
                }
            }
        }
        out.unclip();

        // The "now" rule, then the row labels with their swatches, when the
        // rows are tall enough to keep them apart.
        out.rect(right.round(), TOP, 1., h - TOP - BOT, Paint::Solid(p.rule2));
        if bh < LABEL_MIN {
            return;
        }
        for (i, kd) in LABELS.iter().enumerate() {
            let y = TOP + i as f32 * bh + bh / 2.;
            out.rect(right + 10., y - 5., 8., 8., Paint::Solid(colour(i)));
            out.text(
                [right + 24., y + 3.],
                *kd,
                Font::Mono,
                10.,
                p.soft,
                Align::Left,
            );
        }
    }
}
