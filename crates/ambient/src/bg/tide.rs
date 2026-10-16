//! Tide chart: a semidiurnal tide curve rolling past the hour you're in.
//! The past is hatched densely under the curve and the future lightly; high
//! and low water are marked; a dashed line shows how much of your usage
//! window you've spent, and the header says when it resets. The curve is
//! decorative; the clock and the usage figures are real.

use std::collections::VecDeque;

use crate::geom::spread;
use crate::math::{clamp, clock, ease};
use crate::scene::{Align, Font, Paint, Pt, fit, text_width};
use crate::{Ambient, Ctx, DrawCtx, Scene, Spec};

pub const SPEC: Spec = Spec {
    id: "tide",
    name: "Tide chart",
    from: "Tide curves: a semidiurnal tide with a 12.42 h period and a daily inequality, high and low water marked, the chart rolling past the hour you're in.",
    shows: "The hours around now as a tide, past hatched dense and future light, with how much of your usage window you've spent and when it resets. A mark is left for each session you start.",
    make: |seed| Box::new(Tide::new(seed)),
};

/// Plot margins (pt): left, right, top, bottom.
const L: f32 = 56.;
const R: f32 = 20.;
const TOP: f32 = 48.;
const BOT: f32 = 34.;
/// Hours shown, and how many of them are before now.
const SPAN: f32 = 24.;
const PAST: f32 = 8.;
/// Tide period (h) and the daily inequality's period (h).
const PERIOD: f32 = 12.42;
const DAY: f32 = 24.;
/// Hatch pitch (pt) for the past and the future.
const GAP_PAST: f32 = 5.;
const GAP_FUTURE: f32 = 10.;
/// Session marks kept.
const MARKS: usize = 12;
/// Step (pt) when walking a hatch line; crossings are interpolated.
const WALK: f32 = 2.;
/// Curve sampling step (h).
const STEP: f32 = 0.1;
/// How fast the chart catches up with the real clock (per second), and the
/// largest gap (h) it glides across rather than jumping.
const CATCH_UP: f32 = 2.;
const SNAP: f32 = 1.;

struct Extreme {
    dot: Pt,
    label: Pt,
    text: String,
}

struct Tide {
    ph: f32,
    /// Hours since some midnight, unwrapped so the chart never jumps at 00:00.
    now: f32,
    seeded: bool,
    marks: VecDeque<f32>,
    used: f32,
    /// A real usage reading exists; without one, no figures are shown.
    has_usage: bool,
    header: String,
    // Cached each update.
    geo: Geo,
    col: Vec<f32>,
    curve: Vec<Pt>,
    hatch: [Vec<Pt>; 2],
    extremes: Vec<Extreme>,
}

#[derive(Clone, Copy, Default)]
struct Geo {
    pw: f32,
    ph: f32,
    h0: f32,
}

impl Geo {
    fn x(&self, hr: f32) -> f32 {
        L + (hr - self.h0) / SPAN * self.pw
    }
    fn y(&self, v: f32) -> f32 {
        TOP + (1. - v) * self.ph
    }
}

impl Tide {
    fn new(seed: u64) -> Self {
        Tide {
            ph: (seed % 7) as f32 * 0.8,
            now: 9.5,
            seeded: false,
            marks: VecDeque::new(),
            used: 0.,
            has_usage: false,
            header: String::new(),
            geo: Geo::default(),
            col: Vec::new(),
            curve: Vec::new(),
            hatch: [Vec::new(), Vec::new()],
            extremes: Vec::new(),
        }
    }

    /// Water level 0..1 at hour `hr`.
    fn lev(&self, hr: f32) -> f32 {
        use std::f32::consts::TAU;
        0.5 + 0.34 * (TAU * (hr - self.ph) / PERIOD).cos() + 0.09 * (TAU * (hr - 3.) / DAY).cos()
    }

    /// The curve's y at screen x, from the per-column table.
    fn curve_y(&self, x: f32) -> f32 {
        let i = clamp(x - L, 0., (self.col.len() - 1) as f32);
        let i0 = i.floor() as usize;
        let i1 = (i0 + 1).min(self.col.len() - 1);
        let k = i - i0 as f32;
        self.col[i0] + (self.col[i1] - self.col[i0]) * k
    }

    /// Diagonal hatching under the curve between screen x `xa` and `xz`:
    /// each 45° line is walked a point at a time and kept where it lies
    /// below the curve, with crossings interpolated.
    fn hatch_into(&self, xa: f32, xz: f32, gap: f32, out: &mut Vec<Pt>) {
        out.clear();
        let g = self.geo;
        let bottom = TOP + g.ph;
        let shift = (-g.h0 / SPAN * g.pw) % gap;
        let mut k = -g.ph + shift;
        while k < g.pw + g.ph {
            let x0 = L + k;
            let (s_lo, s_hi) = ((xa - x0).max(0.), (xz - x0).min(g.ph));
            if s_lo < s_hi {
                // Inside where f >= 0: the line is at or below the curve.
                let f = |s: f32| (bottom - s) - self.curve_y(x0 + s);
                let mut s = s_lo;
                let mut fp = f(s);
                let mut start = (fp >= 0.).then_some(s);
                while s < s_hi {
                    let sn = (s + WALK).min(s_hi);
                    let fnx = f(sn);
                    if (fp >= 0.) != (fnx >= 0.) {
                        let c = s + (sn - s) * (fp / (fp - fnx));
                        match start.take() {
                            Some(a) => out.extend([[x0 + a, bottom - a], [x0 + c, bottom - c]]),
                            None => start = Some(c),
                        }
                    }
                    s = sn;
                    fp = fnx;
                }
                if let Some(a) = start
                    && s_hi > a
                {
                    out.extend([[x0 + a, bottom - a], [x0 + s_hi, bottom - s_hi]]);
                }
            }
            k += gap;
        }
    }
}

impl Ambient for Tide {
    fn update(&mut self, cx: &Ctx) {
        // The real clock, glided so the chart scrolls sub-pixel even when
        // the client only reports whole minutes.
        if let Some(m) = cx.signals.minute.filter(|m| m.is_finite()) {
            let mut target = m.rem_euclid(24. * 60.) / 60.;
            if !self.seeded {
                self.now = target;
                self.seeded = true;
            }
            target += ((self.now - target) / 24.).round() * 24.;
            if (target - self.now).abs() > SNAP {
                self.now = target;
            } else {
                self.now += cx.dt / 3600.;
                self.now += (target - self.now) * ease(CATCH_UP, cx.dt);
            }
        } else {
            self.now += cx.dt / 3600.;
        }
        if cx.input.submit {
            self.marks.push_back(self.now);
            if self.marks.len() > MARKS {
                self.marks.pop_front();
            }
        }
        let now = self.now;
        self.marks.retain(|&m| m > now - PAST - 1. && m < now + 1.);
        let reading = cx.signals.usage;
        self.has_usage = reading.is_some();
        let usage = reading.unwrap_or_default();
        self.used = clamp(
            if usage.used.is_finite() {
                usage.used
            } else {
                0.
            },
            0.,
            1.,
        );
        let resets = cx.signals.minute.unwrap_or(self.now * 60.) + usage.resets_in.max(0.) / 60.;
        self.header = if self.has_usage {
            format!(
                "USAGE {:.0}% · RESETS {}",
                self.used * 100.,
                clock(if resets.is_finite() { resets } else { 0. })
            )
        } else {
            String::new()
        };

        // Geometry, the curve and its hatching.
        let (w, h) = (cx.size.w, cx.size.h);
        let g = Geo {
            pw: (w - L - R).max(40.),
            ph: (h - TOP - BOT).max(30.),
            h0: self.now - PAST,
        };
        self.geo = g;
        let cols = g.pw.ceil() as usize + 2;
        self.col.clear();
        for i in 0..cols {
            let hr = g.h0 + i as f32 / g.pw * SPAN;
            self.col.push(g.y(self.lev(hr)));
        }
        self.curve.clear();
        let n = (SPAN / STEP).round() as usize;
        for i in 0..=n {
            let hr = g.h0 + i as f32 * STEP;
            self.curve.push([g.x(hr), g.y(self.lev(hr))]);
        }
        let xn = g.x(self.now);
        let mut past = std::mem::take(&mut self.hatch[0]);
        let mut future = std::mem::take(&mut self.hatch[1]);
        self.hatch_into(L, xn, GAP_PAST, &mut past);
        self.hatch_into(xn, L + g.pw, GAP_FUTURE, &mut future);
        self.hatch = [past, future];

        // High and low water, labels kept apart within each row.
        self.extremes.clear();
        let (mut prev, mut dir) = (self.lev(g.h0), 0f32);
        let mut hr = g.h0 + 0.05;
        while hr <= g.h0 + SPAN {
            let v = self.lev(hr);
            let d = (v - prev).signum() * ((v - prev).abs() > 1e-7) as i32 as f32;
            if dir != 0. && d != 0. && d != dir {
                let hi = dir > 0.;
                let (x, y) = (g.x(hr), g.y(prev));
                let ly = if hi {
                    (TOP + 10.).max(y - 10.)
                } else {
                    (TOP + g.ph - 4.).min(y + 16.)
                };
                let text = format!("{} {}", if hi { "HW" } else { "LW" }, clock(hr * 60.));
                self.extremes.push(Extreme {
                    dot: [x, y],
                    label: [x, ly],
                    text,
                });
            }
            if d != 0. {
                dir = d;
            }
            prev = v;
            hr += 0.05;
        }
        for hi in ["HW", "LW"] {
            let idx: Vec<usize> = (0..self.extremes.len())
                .filter(|&i| self.extremes[i].text.starts_with(hi))
                .collect();
            let mut xs: Vec<f32> = idx.iter().map(|&i| self.extremes[i].label[0]).collect();
            let gap = text_width("HW 00:00", Font::Mono, 10.) + 10.;
            spread(&mut xs, gap, L + 30., (L + g.pw - 30.).max(L + 30.));
            for (&i, x) in idx.iter().zip(xs) {
                self.extremes[i].label[0] = x;
            }
        }
    }

    fn draw(&self, cx: &DrawCtx, out: &mut Scene) {
        let (w, p, g) = (cx.size.w, cx.pal, self.geo);
        if self.col.is_empty() {
            return;
        }
        // Ruling and axes.
        let mut rule = Vec::new();
        for i in 0..=4 {
            let y = g.y(i as f32 * 0.25).round() + 0.5;
            rule.extend([[L, y], [L + g.pw, y]]);
        }
        let mut hr = g.h0.ceil();
        while hr <= g.h0 + SPAN {
            let x = g.x(hr).round() + 0.5;
            let long = (hr as i64).rem_euclid(3) == 0;
            rule.extend([
                [x, TOP + g.ph],
                [x, TOP + g.ph + if long { 6. } else { 3. }],
            ]);
            hr += 1.;
        }
        out.segments(rule, 1., p.rule);
        for i in 0..=2 {
            let v = i as f32 * 0.5;
            out.text(
                [L - 8., g.y(v) + 3.],
                format!("{:.1} m", v * 2.),
                Font::Mono,
                10.,
                p.soft,
                Align::Right,
            );
        }
        let mut hr = (g.h0 / 3.).ceil() * 3.;
        while hr <= g.h0 + SPAN {
            let lbl = format!("{:02}", (hr as i64).rem_euclid(24));
            out.text(
                [g.x(hr), TOP + g.ph + 18.],
                lbl,
                Font::Mono,
                10.,
                p.soft,
                Align::Center,
            );
            hr += 3.;
        }
        out.text(
            [L, 24.],
            fit(&self.header, Font::Mono, 10., w - L - 10.),
            Font::Mono,
            10.,
            p.soft,
            Align::Left,
        );

        out.clip(L, 0., g.pw, cx.size.h);
        out.segments(self.hatch[0].clone(), 0.8, p.soft);
        out.segments(self.hatch[1].clone(), 0.8, p.rule2);
        out.line(self.curve.clone(), 1.5, p.pencil);
        for e in &self.extremes {
            out.rect(e.dot[0] - 2., e.dot[1] - 2., 4., 4., Paint::Solid(p.pencil));
            out.text(
                e.label,
                e.text.clone(),
                Font::Mono,
                10.,
                p.soft,
                Align::Center,
            );
        }
        // How much of the usage window is spent, when there's a reading.
        if self.has_usage {
            let uy = g.y(self.used);
            out.dashed([L, uy], [L + g.pw, uy], 4., 4., 1., p.soft.alpha(0.55));
            out.label(
                [L + g.pw - 6., uy - 5.],
                format!("used {:.0}%", self.used * 100.),
                p.soft,
                p.bg,
                Align::Right,
            );
        }
        for &m in &self.marks {
            let (x, y) = (g.x(m), TOP + g.ph);
            out.fill(
                vec![[x, y - 8.], [x + 4., y], [x - 4., y]],
                Paint::Solid(p.accent),
            );
        }
        out.unclip();

        // Now.
        let nx = g.x(self.now);
        let nv = self.lev(self.now);
        let rising = self.lev(self.now + 0.05) > nv;
        out.line(vec![[nx, TOP], [nx, TOP + g.ph]], 1.5, p.accent);
        out.circle([nx, g.y(nv)], 3.5, Some(p.accent), None);
        let text = format!(
            "now {} · {:.2} m · {}",
            clock(self.now * 60.),
            nv * 2.,
            if rising { "rising" } else { "falling" }
        );
        let tw = text_width(&text, Font::Mono, 10.);
        let x = clamp(nx + 8., 4., (w - tw - 7.).max(4.));
        out.label([x, TOP + 12.], text, p.accent, p.bg, Align::Left);
    }
}
