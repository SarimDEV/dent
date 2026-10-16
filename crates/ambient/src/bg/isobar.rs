//! Isobars: a synoptic weather map. Gaussian highs and lows drift slowly
//! over a little noise; isobars run every 4 hPa with every second one bold
//! and labelled, the centres carry their computed pressure, and a cold
//! front trails from the main low. Usage deepens that low, and each session
//! you start drops a temporary low that fills back in.

use crate::geom::contour;
use crate::math::{Rng, clamp, clock, ease, noise3};
use crate::scene::{Align, Font, Paint, Pt};
use crate::{Ambient, Ctx, DrawCtx, Scene, Spec};

pub const SPEC: Spec = Spec {
    id: "isobar",
    name: "Isobars",
    from: "Synoptic weather maps: isobars every 4 hPa, labelled every 8, highs and lows marked with their pressure, a cold front with its teeth.",
    shows: "Usage as weather: the main low deepens as you use up your limit, so the front feels nearer. Each new session drops a low that fills back in over a few seconds.",
    make: |seed| Box::new(Isobar::new(seed)),
};

/// Field grid pitch (pt).
const CELL: f32 = 12.;
/// Cells computed at most; beyond this the pitch grows.
const MAX_CELLS: usize = 80_000;
/// Contour spacing in field units (one step is 4 hPa).
const STEP: f32 = 0.25;
/// Contoured range in field units (deep lows reach well below -1.5).
const LO: f32 = -2.5;
const HI: f32 = 1.5;
const LEVELS: usize = ((HI - LO) / STEP) as usize + 1;
/// hPa at field value 0, and hPa per field unit.
const P0: f32 = 1012.;
const P_PER: f32 = 16.;
/// Noise amplitude over the centres.
const NOISE: f32 = 0.12;
/// The main low: depth at no usage, and extra depth at a full limit.
const LOW_BASE: f32 = 0.7;
const LOW_USAGE: f32 = 0.8;
/// Temporary lows: depth, radius (share of the long side), deepening time
/// (s), filling time constant (s), lifetime (s) and cap.
const TMP_A: f32 = 1.;
const TMP_R: f32 = 0.16;
const TMP_RISE: f32 = 1.5;
const TMP_FILL: f32 = 4.;
const TMP_LIFE: f32 = 14.;
const TMP_MAX: usize = 3;
/// A low pushed out by the cap fades this fast (per second).
const RETIRE: f32 = 3.;
/// Front teeth spacing (pt).
const TOOTH: f32 = 26.;
/// Contour labels: keep this far apart (pt) and from the centres.
const LABEL_GAP_X: f32 = 42.;
const LABEL_GAP_Y: f32 = 22.;
/// A label follows its line if the line stays within this distance (pt).
const LABEL_FOLLOW: f32 = 30.;

#[derive(Clone, Copy)]
struct Centre {
    fx: f32,
    fy: f32,
    a: f32,
    r: f32,
    p: f32,
}

#[derive(Clone, Copy)]
struct Tmp {
    fx: f32,
    fy: f32,
    born: f32,
    /// 1 until pushed out by the cap, then fades to 0.
    keep: f32,
    retiring: bool,
}

/// A centre as placed this frame.
#[derive(Clone, Copy)]
struct Mark {
    x: f32,
    y: f32,
    a: f32,
    rr: f32,
    hi: bool,
    /// How strongly to draw its letter (0..1).
    show: f32,
    /// Pressure there, field units.
    v: f32,
}

#[derive(Clone, Copy, Default)]
struct Label {
    at: Option<Pt>,
    alpha: f32,
    /// Where it should be, if anywhere.
    want: Option<Pt>,
}

struct Isobar {
    seed: f32,
    rng: Rng,
    centres: [Centre; 4],
    tmp: Vec<Tmp>,
    used: f32,
    vals: Vec<f32>,
    coarse: Vec<f32>,
    fx: Vec<f32>,
    fy: Vec<f32>,
    marks: Vec<Mark>,
    /// Contour segments per level, in points.
    segs: Vec<Vec<Pt>>,
    labels: [Label; LEVELS],
    front: Vec<Pt>,
    teeth: Vec<[Pt; 3]>,
    header: Option<String>,
}

impl Isobar {
    fn new(seed: u64) -> Self {
        let c = |fx, fy, a, r, p| Centre { fx, fy, a, r, p };
        Isobar {
            seed: (seed % 20) as f32,
            rng: Rng::new(seed ^ 0x150B),
            centres: [
                c(0.25, 0.35, 1., 0.32, 0.),
                c(0.74, 0.72, 1., 0.28, 2.),
                c(0.62, 0.24, -1., 0.26, 4.),
                c(0.16, 0.82, -0.9, 0.3, 1.),
            ],
            tmp: Vec::new(),
            used: 0.4,
            vals: Vec::new(),
            coarse: Vec::new(),
            fx: Vec::new(),
            fy: Vec::new(),
            marks: Vec::new(),
            segs: (0..LEVELS).map(|_| Vec::new()).collect(),
            labels: [Label::default(); LEVELS],
            front: Vec::new(),
            teeth: Vec::new(),
            header: None,
        }
    }

    fn pos(fx: f32, fy: f32, p: f32, t: f32) -> (f32, f32) {
        (
            fx + 0.07 * (t * 0.05 + p).sin(),
            fy + 0.05 * (t * 0.04 + p * 1.3).cos(),
        )
    }

    fn spawn(&mut self, t: f32) {
        // Try a few spots and keep the one furthest from the other temporary lows.
        let mut best = (0.5, 0.5, -1.);
        for _ in 0..6 {
            let (fx, fy) = (0.3 + self.rng.f() * 0.4, 0.3 + self.rng.f() * 0.4);
            let d = self
                .tmp
                .iter()
                .filter(|c| !c.retiring)
                .map(|c| (c.fx - fx).powi(2) + (c.fy - fy).powi(2))
                .fold(f32::MAX, f32::min);
            if d > best.2 {
                best = (fx, fy, d);
            }
        }
        if self.tmp.iter().filter(|c| !c.retiring).count() >= TMP_MAX
            && let Some(old) = self.tmp.iter_mut().find(|c| !c.retiring)
        {
            old.retiring = true;
        }
        self.tmp.push(Tmp {
            fx: best.0,
            fy: best.1,
            born: t,
            keep: 1.,
            retiring: false,
        });
        // Hard cap: drop the faintest retiring low.
        if self.tmp.len() > TMP_MAX * 2
            && let Some(i) = (0..self.tmp.len())
                .filter(|&i| self.tmp[i].retiring)
                .min_by(|&a, &b| self.tmp[a].keep.total_cmp(&self.tmp[b].keep))
        {
            self.tmp.remove(i);
        }
    }
}

fn pressure(marks: &[Mark], x: f32, y: f32, t: f32, seed: f32) -> f32 {
    let mut v = NOISE * noise3(x * 0.004, y * 0.004, t * 0.02 + seed);
    for c in marks {
        let d2 = (x - c.x).powi(2) + (y - c.y).powi(2);
        if d2 < c.rr * 12. {
            v += c.a * (-d2 / c.rr).exp();
        }
    }
    v
}

/// The point on segment `a`–`b` nearest `p`.
fn nearest(a: Pt, b: Pt, p: Pt) -> Pt {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    let k = if len2 < 1e-9 {
        0.
    } else {
        clamp(((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2, 0., 1.)
    };
    [a[0] + dx * k, a[1] + dy * k]
}

fn hpa(v: f32) -> String {
    format!("{}", (P0 + (v * P_PER).round()) as i32)
}

fn level(i: usize) -> f32 {
    LO + i as f32 * STEP
}

/// Bold isobars fall on every 8 hPa, including 1012.
fn bold(i: usize) -> bool {
    (level(i) / STEP).round() as i32 % 2 == 0
}

impl Ambient for Isobar {
    fn update(&mut self, cx: &Ctx) {
        let (w, h, t, dt) = (cx.size.w, cx.size.h, cx.t, cx.dt);
        let usage = cx.signals.usage;
        if let Some(u) = usage {
            self.used += (clamp(u.used, 0., 1.) - self.used) * ease(1., dt);
        }
        if cx.input.submit || cx.signals.started > 0 {
            self.spawn(t);
        }
        for c in &mut self.tmp {
            if c.retiring {
                c.keep *= (-RETIRE * dt).exp();
            }
        }
        self.tmp.retain(|c| t - c.born < TMP_LIFE && c.keep > 0.01);

        // Centres as placed now.
        let big = w.max(h);
        self.marks.clear();
        for (i, c) in self.centres.iter().enumerate() {
            let (mut fx, mut a) = (c.fx, c.a);
            if i == 2 {
                a = -(LOW_BASE + LOW_USAGE * self.used);
                fx -= 0.08 * self.used;
            }
            let (fx, fy) = Self::pos(fx, c.fy, c.p, t);
            let rr = (c.r * big).powi(2);
            self.marks.push(Mark {
                x: fx * w,
                y: fy * h,
                a,
                rr,
                hi: a > 0.,
                show: 1.,
                v: 0.,
            });
        }
        for c in &self.tmp {
            let age = t - c.born;
            let life = (age / TMP_RISE).min(1.) * (-age / TMP_FILL).exp() * c.keep;
            let (fx, fy) = Self::pos(c.fx, c.fy, 3., t);
            let show = clamp((life - 0.2) / 0.5, 0., 1.);
            let rr = (TMP_R * big).powi(2);
            self.marks.push(Mark {
                x: fx * w,
                y: fy * h,
                a: -TMP_A * life,
                rr,
                hi: false,
                show,
                v: 0.,
            });
        }

        // The field.
        let mut cell = CELL;
        let (mut cols, mut rows);
        loop {
            cols = (w / cell).ceil() as usize + 1;
            rows = (h / cell).ceil() as usize + 1;
            if cols * rows <= MAX_CELLS {
                break;
            }
            cell *= 1.25;
        }
        self.vals.resize(cols * rows, 0.);
        // The noise is broad and faint, so it is sampled every second cell
        // and blended; each centre is a Gaussian, so it separates into a
        // column factor times a row factor.
        let (nc, nr) = (cols / 2 + 2, rows / 2 + 2);
        let z = t * 0.02 + self.seed;
        self.coarse.clear();
        self.coarse.extend((0..nc * nr).map(|i| {
            let (x, y) = ((i % nc) as f32 * 2. * cell, (i / nc) as f32 * 2. * cell);
            NOISE * noise3(x * 0.004, y * 0.004, z)
        }));
        let m = self.marks.len();
        self.fx.clear();
        self.fy.clear();
        for c in &self.marks {
            let g = |d: f32| (-d * d / c.rr).exp();
            self.fx
                .extend((0..cols).map(|gx| c.a * g(gx as f32 * cell - c.x)));
            self.fy
                .extend((0..rows).map(|gy| g(gy as f32 * cell - c.y)));
        }
        for gy in 0..rows {
            let (ny, ky) = (gy / 2, (gy % 2) as f32 * 0.5);
            let (c0, c1) = (
                &self.coarse[ny * nc..][..nc],
                &self.coarse[(ny + 1) * nc..][..nc],
            );
            let row = &mut self.vals[gy * cols..][..cols];
            for (gx, v) in row.iter_mut().enumerate() {
                let (nx, kx) = (gx / 2, (gx % 2) as f32 * 0.5);
                let top = c0[nx] + (c0[nx + 1] - c0[nx]) * kx;
                let bot = c1[nx] + (c1[nx + 1] - c1[nx]) * kx;
                *v = top + (bot - top) * ky;
            }
            for k in 0..m {
                let f = self.fy[k * rows + gy];
                for (v, &g) in row.iter_mut().zip(&self.fx[k * cols..][..cols]) {
                    *v += g * f;
                }
            }
        }
        let (lo, hi) = self
            .vals
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
        for i in 0..self.marks.len() {
            let (x, y) = (self.marks[i].x, self.marks[i].y);
            self.marks[i].v = pressure(&self.marks, x, y, t, self.seed);
        }

        for (i, segs) in self.segs.iter_mut().enumerate() {
            segs.clear();
            if !(lo..=hi).contains(&level(i)) {
                continue;
            }
            contour(&self.vals, cols, rows, level(i), segs);
            for p in segs.iter_mut() {
                *p = [p[0] * cell, p[1] * cell];
            }
        }

        // Labels on bold isobars: follow the line, otherwise fade out and
        // reappear somewhere clear of other labels and the centres.
        let mut taken: Vec<Pt> = self
            .marks
            .iter()
            .filter(|m| m.show > 0.)
            .map(|m| [m.x, m.y + 6.])
            .collect();
        let clear = |p: Pt, taken: &[Pt]| {
            taken
                .iter()
                .all(|q| (p[0] - q[0]).abs() > LABEL_GAP_X || (p[1] - q[1]).abs() > LABEL_GAP_Y)
        };
        let inside = |p: &Pt| p[0] > 24. && p[0] < w - 40. && p[1] > 34. && p[1] < h - 10.;
        let k = ease(4., dt);
        for i in (0..LEVELS).filter(|&i| bold(i)) {
            let segs = &self.segs[i];
            let mids = segs
                .as_chunks::<2>()
                .0
                .iter()
                .map(|s| [(s[0][0] + s[1][0]) / 2., (s[0][1] + s[1][1]) / 2.]);
            let label = &mut self.labels[i];
            label.want = None;
            if let Some(at) = label.at {
                // The nearest point on the line, so the label glides with it.
                let near = segs
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|s| nearest(s[0], s[1], at))
                    .filter(inside)
                    .map(|m| (m, (m[0] - at[0]).powi(2) + (m[1] - at[1]).powi(2)))
                    .min_by(|a, b| a.1.total_cmp(&b.1));
                if let Some((m, d2)) = near
                    && d2 < LABEL_FOLLOW * LABEL_FOLLOW
                    && clear(m, &taken)
                {
                    label.want = Some(m);
                }
            } else {
                // Somewhere new: the clear midpoint furthest from everything taken.
                let mut best: Option<(Pt, f32)> = None;
                for m in mids.filter(inside).step_by(3) {
                    let d = taken
                        .iter()
                        .map(|q| (m[0] - q[0]).powi(2) + (m[1] - q[1]).powi(2))
                        .fold(f32::MAX, f32::min);
                    if clear(m, &taken) && best.is_none_or(|b| d > b.1) {
                        best = Some((m, d));
                    }
                }
                if let Some((m, _)) = best {
                    label.at = Some(m);
                    label.want = Some(m);
                }
            }
            match label.want {
                Some(m) => {
                    label.at = Some(m);
                    label.alpha += (1. - label.alpha) * k;
                    taken.push(m);
                }
                None => {
                    label.alpha -= label.alpha * k * 1.5;
                    if label.alpha < 0.02 {
                        *label = Label::default();
                    } else if let Some(at) = label.at {
                        taken.push(at);
                    }
                }
            }
        }

        // The cold front from the main low, with its teeth.
        let lo = self.marks[2];
        let reach = big * 0.5 * (0.8 + 0.4 * self.used);
        self.front.clear();
        self.front.extend((0..=100).map(|i| {
            let k = i as f32 / 100.;
            let ang = 2.2 + k * 0.7;
            [lo.x + k * reach * ang.cos(), lo.y + k * reach * ang.sin()]
        }));
        self.teeth.clear();
        let mut acc = 0.;
        for s in self.front.windows(2) {
            let ([x0, y0], [x1, y1]) = (s[0], s[1]);
            let d = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt();
            acc += d;
            if acc > TOOTH && d > 1e-4 {
                acc = 0.;
                let (ux, uy) = ((x1 - x0) / d, (y1 - y0) / d);
                self.teeth.push([
                    [x1 - ux * 5., y1 - uy * 5.],
                    [x1 + ux * 5., y1 + uy * 5.],
                    [x1 - uy * 8., y1 + ux * 8.],
                ]);
            }
        }

        self.header = match (usage, cx.signals.minute) {
            (Some(u), Some(m)) => Some(format!(
                "USAGE {:.0}% · RESETS {}",
                clamp(u.used, 0., 1.) * 100.,
                clock(m + u.resets_in.max(0.) / 60.)
            )),
            (Some(u), None) => Some(format!("USAGE {:.0}%", clamp(u.used, 0., 1.) * 100.)),
            _ => None,
        };
    }

    fn draw(&self, cx: &DrawCtx, out: &mut Scene) {
        let p = cx.pal;
        for (i, segs) in self.segs.iter().enumerate() {
            let (color, width) = if bold(i) {
                (p.pencil, 1.3)
            } else {
                (p.soft, 0.8)
            };
            out.segments(segs.clone(), width, color);
        }
        for (i, l) in self.labels.iter().enumerate() {
            if let Some(at) = l.at
                && l.alpha > 0.01
            {
                let a = l.alpha;
                out.label(
                    [at[0], at[1] + 3.],
                    hpa(level(i)),
                    p.soft.alpha(a),
                    p.bg.alpha(a),
                    Align::Center,
                );
            }
        }

        out.line(self.front.clone(), 2., p.accent);
        for tooth in &self.teeth {
            out.fill(tooth.to_vec(), Paint::Solid(p.accent));
        }

        for m in &self.marks {
            if m.show <= 0.01 {
                continue;
            }
            let letter = if m.hi { "H" } else { "L" };
            out.text(
                [m.x, m.y + 8.],
                letter,
                Font::Display,
                32.,
                p.pencil.alpha(m.show),
                Align::Center,
            );
            out.text(
                [m.x, m.y + 22.],
                hpa(m.v),
                Font::Mono,
                10.,
                p.soft.alpha(m.show),
                Align::Center,
            );
        }

        if let Some(head) = &self.header {
            out.label([16., 20.], head.clone(), p.soft, p.bg, Align::Left);
        }
    }
}
