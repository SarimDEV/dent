//! Bearings: a compass card with the open project and recent sessions
//! marked as landmarks on the ring. The needle swings on a lightly damped
//! spring toward the pointer, or drifts on its own, and settles.

use std::collections::VecDeque;

use crate::geom::spread;
use crate::math::{ease, noise3, wrap_deg};
use crate::scene::{Align, Font, Paint, Pt, fit, text_width};
use crate::{Ambient, Ctx, DrawCtx, Scene, Spec};

pub const SPEC: Spec = Spec {
    id: "bearing",
    name: "Bearings",
    from: "Compass cards and sighted bearings, written down to the degree. The needle swings on a lightly damped spring and settles.",
    shows: "A bearing that follows your pointer around the composer, with the open project and your recent sessions marked as landmarks on the ring, each at its own fixed bearing. A ping spreads from the centre when a session starts.",
    make: |seed| Box::new(Bearing::new(seed)),
};

/// Landmarks shown at once.
const MARKS: usize = 4;
/// Landmarks kept while old ones fade out.
const MARKS_KEEP: usize = 8;
/// Landmark fade rate (per second) and label glide rate (per second).
const FADE: f32 = 2.5;
const GLIDE: f32 = 4.;
/// Spring stiffness (1/s²) and damping (1/s); sub-step length (s).
const STIFF: f32 = 22.;
const DAMP: f32 = 7.;
const SUBSTEP: f32 = 0.008;
/// Pings kept at once, and how long one lasts (s).
const PINGS: usize = 4;
const PING_LIFE: f32 = 2.;
/// Longest landmark name (characters' worth of mono at 10 pt).
const NAME_MAX: f32 = 84.;

struct Mark {
    name: String,
    /// True bearing, whole degrees.
    deg: f32,
    /// Where its label sits, eased toward the spread position.
    label: f32,
    goal: f32,
    alpha: f32,
    live: bool,
}

struct Bearing {
    seed: f32,
    ang: f32,
    vel: f32,
    pings: VecDeque<f32>,
    marks: Vec<Mark>,
    r: f32,
    spread_buf: Vec<f32>,
}

/// A stable bearing for a name (FNV-1a), whole degrees.
fn bearing_of(name: &str) -> f32 {
    let mut h: u32 = 0x811C_9DC5;
    for b in name.bytes() {
        h = (h ^ b as u32).wrapping_mul(0x0100_0193);
    }
    (h % 360) as f32
}

fn keep_in(v: f32, lo: f32, hi: f32) -> f32 {
    v.min(hi).max(lo)
}

impl Bearing {
    fn new(seed: u64) -> Self {
        Bearing {
            seed: (seed % 9) as f32,
            ang: 30.,
            vel: 0.,
            pings: VecDeque::new(),
            marks: Vec::new(),
            r: 70.,
            spread_buf: Vec::new(),
        }
    }

    fn update_marks(&mut self, cx: &Ctx) {
        let s = cx.signals;
        let mut names: [&str; MARKS] = [""; MARKS];
        let mut n = 0;
        for name in s
            .project
            .iter()
            .map(|p| p.as_str())
            .chain(s.sessions.iter().rev().map(|x| x.name.as_str()))
        {
            if n == MARKS {
                break;
            }
            if !name.is_empty() && !names[..n].contains(&name) {
                names[n] = name;
                n += 1;
            }
        }
        let names = &names[..n];
        for m in &mut self.marks {
            m.live = names.contains(&m.name.as_str());
        }
        for &name in names {
            if !self.marks.iter().any(|m| m.name == name) {
                let deg = bearing_of(name);
                self.marks.push(Mark {
                    name: name.to_string(),
                    deg,
                    label: deg,
                    goal: deg,
                    alpha: 0.,
                    live: true,
                });
            }
        }
        let k = ease(FADE, cx.dt);
        for m in &mut self.marks {
            m.alpha += ((if m.live { 1. } else { 0. }) - m.alpha) * k;
        }
        self.marks.retain(|m| m.live || m.alpha > 0.01);
        while self.marks.len() > MARKS_KEEP {
            if let Some(i) = self.marks.iter().position(|m| !m.live) {
                self.marks.remove(i);
            } else {
                break;
            }
        }

        // Spread the labels round the ring, cutting it open at its widest gap.
        let ring = (self.r * 1.07).max(1.);
        let widest = self
            .marks
            .iter()
            .map(|m| text_width(&label_text(m), Font::Mono, 10.))
            .fold(0f32, f32::max);
        let gap = ((widest + 8.) / ring)
            .to_degrees()
            .min(360. / self.marks.len().max(1) as f32);
        let mut order: Vec<usize> = (0..self.marks.len()).collect();
        order.sort_by(|&a, &b| self.marks[a].deg.total_cmp(&self.marks[b].deg));
        let mut cut = 0.;
        let mut biggest = -1.;
        for (i, &oi) in order.iter().enumerate() {
            let prev = self.marks[order[(i + order.len() - 1) % order.len()]].deg;
            let g = (self.marks[oi].deg - prev).rem_euclid(360.);
            let g = if order.len() == 1 { 360. } else { g };
            if g > biggest {
                biggest = g;
                cut = self.marks[oi].deg;
            }
        }
        self.spread_buf.clear();
        self.spread_buf
            .extend(self.marks.iter().map(|m| (m.deg - cut).rem_euclid(360.)));
        let before: f32 = self.spread_buf.iter().sum();
        spread(&mut self.spread_buf, gap, f32::MIN, f32::MAX);
        let shift =
            (self.spread_buf.iter().sum::<f32>() - before) / self.spread_buf.len().max(1) as f32;
        let g = ease(GLIDE, cx.dt);
        for (m, &v) in self.marks.iter_mut().zip(&self.spread_buf) {
            m.goal = v - shift + cut;
            m.label += wrap_deg(m.goal - m.label) * g;
        }
    }
}

fn label_text(m: &Mark) -> String {
    format!(
        "{} {:03}°",
        fit(&m.name, Font::Mono, 10., NAME_MAX),
        m.deg as i32
    )
}

impl Ambient for Bearing {
    fn update(&mut self, cx: &Ctx) {
        let (w, h, t, dt) = (cx.size.w, cx.size.h, cx.t, cx.dt);
        self.r = (w.min(h) * 0.42).max(70.);
        let idle = 40. + 70. * noise3(t * 0.08, 1., self.seed);
        let mut target = idle;
        if let Some(p) = cx.input.pointer {
            let aim = (p.x - w / 2.).atan2(-(p.y - h / 2.)).to_degrees();
            target = idle + wrap_deg(aim - idle) * p.k;
        }
        // Sub-stepped so the spring is stable at any frame rate.
        let n = (dt / SUBSTEP).ceil().max(1.) as usize;
        let sdt = dt / n as f32;
        for _ in 0..n {
            self.vel += wrap_deg(target - self.ang) * sdt * STIFF;
            self.vel *= (-sdt * DAMP).exp();
            self.ang += self.vel * sdt;
        }
        self.ang = self.ang.rem_euclid(360.);
        if cx.input.submit || cx.signals.started > 0 {
            self.pings.push_back(t);
            if self.pings.len() > PINGS {
                self.pings.pop_front();
            }
        }
        while self.pings.front().is_some_and(|&p| t - p >= PING_LIFE) {
            self.pings.pop_front();
        }
        self.update_marks(cx);
    }

    fn draw(&self, cx: &DrawCtx, out: &mut Scene) {
        let (w, h, p, t) = (cx.size.w, cx.size.h, cx.pal, cx.t);
        let (cxp, cyp, r) = (w / 2., h / 2., self.r);
        let at = |deg: f32, rr: f32| -> Pt {
            let a = deg.to_radians();
            [cxp + a.sin() * rr, cyp - a.cos() * rr]
        };

        // The card.
        out.circle([cxp, cyp], r, None, Some((1., p.rule2)));
        out.circle([cxp, cyp], r * 0.9, None, Some((1., p.rule2)));
        let rd = r * 0.55;
        let circ = std::f32::consts::TAU * rd;
        let mut dash = Vec::new();
        let mut s = 0.;
        while s < circ {
            let e = (s + 3.).min(circ);
            dash.extend([at(s / circ * 360., rd), at(e / circ * 360., rd)]);
            s += 8.;
        }
        out.segments(dash, 1., p.rule);
        let mut ticks = Vec::with_capacity(360);
        for d in (0..360).step_by(2) {
            let r0 = if d % 10 != 0 { r * 0.965 } else { r * 0.93 };
            ticks.extend([at(d as f32, r0), at(d as f32, r)]);
        }
        out.segments(ticks, 1., p.soft);
        for d in (30..360).step_by(30).filter(|d| d % 90 != 0) {
            let [x, y] = at(d as f32, r * 0.84);
            out.text(
                [x, y + 3.],
                format!("{d:03}"),
                Font::Mono,
                10.,
                p.soft,
                Align::Center,
            );
        }
        for (name, d) in [("N", 0.), ("E", 90.), ("S", 180.), ("W", 270.)] {
            let [x, y] = at(d, r * 0.78);
            out.text(
                [x, y + 7.],
                name,
                Font::Display,
                20.,
                p.pencil,
                Align::Center,
            );
        }

        // Landmarks.
        for m in &self.marks {
            let col = p.soft.alpha(m.alpha);
            out.dashed(at(m.deg, r * 0.58), at(m.deg, r * 0.9), 2., 4., 1., col);
            let [lx, ly] = at(m.label, r * 1.07);
            out.text(
                [lx, ly + 3.],
                label_text(m),
                Font::Mono,
                10.,
                col,
                Align::Center,
            );
        }

        // The needle.
        let a = self.ang;
        let (tip, tail) = (at(a, r), at(a, r * 0.2));
        out.line(vec![tail, tip], 1.6, p.accent);
        out.fill(
            vec![tip, at(a - 1.6, r * 0.95), at(a + 1.6, r * 0.95)],
            Paint::Solid(p.accent),
        );
        let [rx, ry] = at(a, r * 1.14);
        let brg = format!("BRG {:03}°", (a.round() as i32).rem_euclid(360));
        out.label(
            [keep_in(rx, 34., w - 34.), keep_in(ry + 3., 12., h - 4.)],
            brg,
            p.accent,
            p.bg,
            Align::Center,
        );
        out.segments(
            vec![
                [cxp - 6., cyp],
                [cxp + 6., cyp],
                [cxp, cyp - 6.],
                [cxp, cyp + 6.],
            ],
            1.,
            p.pencil,
        );

        for &ping in &self.pings {
            let age = (t - ping) / PING_LIFE;
            if (0. ..1.).contains(&age) {
                out.circle(
                    [cxp, cyp],
                    r * age,
                    None,
                    Some((1., p.accent.alpha(1. - age))),
                );
            }
        }
    }
}
