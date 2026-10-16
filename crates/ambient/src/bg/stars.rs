//! Star chart: a celestial chart turning about a pole off the top-left of
//! the screen. A graticule in hours of right ascension, a dashed ecliptic,
//! 900 seeded stars drawn larger the brighter they are, your project and
//! sessions as named stars, your machines as constellations, and a nova for
//! each session started, which keeps its place in the sky as it turns.

use std::collections::VecDeque;

use crate::math::{Rng, clamp, ease};
use crate::scene::{Align, Font, Pt};
use crate::{Ambient, Ctx, DrawCtx, Scene, Spec};

pub const SPEC: Spec = Spec {
    id: "stars",
    name: "Star chart",
    from: "Celestial charts: a graticule in hours of right ascension, stars drawn larger the brighter they are, the sky turning about the pole.",
    shows: "Your project and recent sessions as named stars, and your machines as constellations. A new session appears as a nova and keeps its place in the sky.",
    make: |seed| Box::new(Stars::new(seed)),
};

/// Background stars.
const STARS: usize = 900;
/// How fast the sky turns (radians per second); negative is counter-clockwise.
const TURN: f32 = -0.012;
/// Star radius (pt) by magnitude 1..=5 (index 0 unused).
const MAG_R: [f32; 6] = [0., 2.8, 2.1, 1.5, 1.05, 0.7];
/// Magnitudes up to this one are drawn as circles; fainter ones as dots.
const CIRCLE_MAG: u8 = 3;
/// Named stars at most.
const NAMED: usize = 7;
/// The order free slots are filled in: slot 1 starts on screen and slot 0
/// is about to turn off it, so the project (named first) lands where it's seen.
const FILL: [usize; NAMED] = [1, 2, 3, 4, 5, 6, 0];
/// Novas kept.
const NOVAS: usize = 6;
/// Candidate spots tried for each nova.
const NOVA_TRIES: usize = 12;
/// How long a nova's ring expands and fades (s), and its fade-in (s).
const RING_LIFE: f32 = 1.6;
const NOVA_FADE: f32 = 0.3;
/// Name fade rate (per second).
const NAME_FADE: f32 = 3.;
/// Ecliptic dash and gap (pt).
const DASH: f32 = 4.;

struct Star {
    r: f32,
    c: f32,
    s: f32,
    m: u8,
}

/// A name that fades in when it arrives and out when it leaves.
#[derive(Default)]
struct Slot {
    name: String,
    on: bool,
    a: f32,
}

struct Constellation {
    pts: Vec<(f32, f32)>,
    label: Slot,
}

struct Nova {
    r: f32,
    th: f32,
    t0: f32,
}

#[derive(Clone, Copy, Default)]
struct Frame {
    cx: f32,
    cy: f32,
    r: f32,
    rot: f32,
}

impl Frame {
    fn at(w: f32, h: f32, t: f32) -> Frame {
        Frame {
            cx: -0.15 * w,
            cy: -0.25 * h,
            r: w.hypot(h) * 1.25,
            rot: t * TURN,
        }
    }
    fn xy(&self, r: f32, th: f32) -> Pt {
        let a = th + self.rot;
        [
            self.cx + r * self.r * a.cos(),
            self.cy + r * self.r * a.sin(),
        ]
    }
}

struct Stars {
    rng: Rng,
    /// Sorted by radius, so culling is a range lookup.
    stars: Vec<Star>,
    named: Vec<Slot>,
    cons: [Constellation; 2],
    novas: VecDeque<Nova>,
    t: f32,
    w: f32,
    h: f32,
    f: Frame,
    // Cached each update for a cheap draw.
    big: Vec<(Pt, f32)>,
    dots: [Vec<Pt>; 2],
    ecliptic: Vec<Pt>,
}

impl Stars {
    fn new(seed: u64) -> Self {
        let mut rng = Rng::new(seed ^ 0x57A2);
        let mut stars: Vec<Star> = (0..STARS)
            .map(|_| {
                let r = 0.12 + rng.f().sqrt() * 0.88;
                let th = rng.f() * std::f32::consts::TAU;
                let m = 5 - (rng.f().powi(3) * 5.).floor() as u8;
                Star {
                    r,
                    c: th.cos(),
                    s: th.sin(),
                    m: m.clamp(1, 5),
                }
            })
            .collect();
        stars.sort_by(|a, b| a.r.total_cmp(&b.r));
        let mut con = |r: f32, th: f32| Constellation {
            pts: (0..6)
                .map(|k| {
                    (
                        r + (rng.f() - 0.5) * 0.12,
                        th + (k as f32 - 2.5) * 0.05 + (rng.f() - 0.5) * 0.03,
                    )
                })
                .collect(),
            label: Slot::default(),
        };
        let cons = [con(0.34, 0.55), con(0.55, 1.15)];
        Stars {
            rng,
            stars,
            named: (0..NAMED).map(|_| Slot::default()).collect(),
            cons,
            novas: VecDeque::new(),
            t: 0.,
            w: 1.,
            h: 1.,
            f: Frame::default(),
            big: Vec::new(),
            dots: [Vec::new(), Vec::new()],
            ecliptic: Vec::new(),
        }
    }

    /// Where named star `i` sits in the sky.
    fn named_at(i: usize) -> (f32, f32) {
        (
            0.25 + ((i as f32 * 0.618) % 1.) * 0.45,
            i as f32 * 0.42 + 0.35,
        )
    }

    fn place_nova(&mut self) {
        let (w, h, f) = (self.w, self.h, self.f);
        let mut best = [w * 0.5, h * 0.3];
        let mut best_d = -1.;
        for _ in 0..NOVA_TRIES {
            let p = [
                w * (0.15 + self.rng.f() * 0.7),
                h * (0.15 + self.rng.f() * 0.4),
            ];
            let d = self
                .novas
                .iter()
                .map(|n| {
                    let q = f.xy(n.r, n.th);
                    (q[0] - p[0]).hypot(q[1] - p[1])
                })
                .fold(1e9_f32, f32::min);
            if d > best_d {
                best_d = d;
                best = p;
            }
        }
        let (dx, dy) = (best[0] - f.cx, best[1] - f.cy);
        self.novas.push_back(Nova {
            r: dx.hypot(dy) / f.r,
            th: dy.atan2(dx) - f.rot,
            t0: self.t,
        });
        if self.novas.len() > NOVAS {
            self.novas.pop_front();
        }
    }

    fn sync_names(&mut self, cx: &Ctx) {
        let s = cx.signals;
        let mut want: Vec<&str> = Vec::with_capacity(NAMED);
        let names = s
            .project
            .iter()
            .map(String::as_str)
            .chain(s.sessions.iter().rev().map(|x| x.name.as_str()));
        for n in names {
            if want.len() >= NAMED {
                break;
            }
            if !n.is_empty() && !want.contains(&n) {
                want.push(n);
            }
        }
        for slot in &mut self.named {
            slot.on = !slot.name.is_empty() && want.contains(&slot.name.as_str());
        }
        for n in want {
            if self.named.iter().any(|s| s.name == n) {
                continue;
            }
            if let Some(i) = FILL
                .iter()
                .copied()
                .find(|&i| self.named[i].name.is_empty())
            {
                let slot = &mut self.named[i];
                *slot = Slot {
                    name: n.to_string(),
                    on: true,
                    a: 0.,
                };
            }
        }
        // Constellations: this machine and the next one along.
        let here = s.machine.clone().unwrap_or_default();
        let next = s
            .machines
            .iter()
            .position(|m| m.name == here)
            .map(|i| {
                (1..s.machines.len())
                    .map(|k| &s.machines[(i + k) % s.machines.len()].name)
                    .find(|n| **n != here)
            })
            .unwrap_or_else(|| s.machines.iter().map(|m| &m.name).find(|n| **n != here))
            .cloned()
            .unwrap_or_default();
        for (c, n) in self.cons.iter_mut().zip([here, next]) {
            if c.label.name != n {
                c.label = Slot {
                    name: n,
                    on: true,
                    a: 0.,
                };
            }
        }
        let k = ease(NAME_FADE, cx.dt);
        for slot in self
            .named
            .iter_mut()
            .chain(self.cons.iter_mut().map(|c| &mut c.label))
        {
            let target = if slot.on && !slot.name.is_empty() {
                1.
            } else {
                0.
            };
            slot.a += (target - slot.a) * k;
            if !slot.on && slot.a < 0.01 {
                slot.name.clear();
                slot.a = 0.;
            }
        }
    }

    fn cache(&mut self) {
        let (w, h, f) = (self.w, self.h, self.f);
        // Only stars whose radius can reach the screen are rotated at all.
        let near = [clamp(f.cx, 0., w), clamp(f.cy, 0., h)];
        let rmin = ((near[0] - f.cx).hypot(near[1] - f.cy) - 4.) / f.r;
        let rmax = [[0., 0.], [w, 0.], [0., h], [w, h]]
            .iter()
            .map(|p: &Pt| (p[0] - f.cx).hypot(p[1] - f.cy))
            .fold(0., f32::max)
            / f.r
            + 4. / f.r;
        let lo = self.stars.partition_point(|s| s.r < rmin);
        let hi = self.stars.partition_point(|s| s.r <= rmax);
        let (cr, sr) = (f.rot.cos(), f.rot.sin());
        self.big.clear();
        self.dots[0].clear();
        self.dots[1].clear();
        for st in &self.stars[lo..hi] {
            let rr = st.r * f.r;
            let x = f.cx + rr * (st.c * cr - st.s * sr);
            let y = f.cy + rr * (st.s * cr + st.c * sr);
            if x < -4. || y < -4. || x > w + 4. || y > h + 4. {
                continue;
            }
            let rad = MAG_R[st.m as usize];
            if st.m <= CIRCLE_MAG {
                self.big.push(([x, y], rad));
            } else {
                self.dots[(st.m - 4) as usize].push([x - rad, y - rad]);
            }
        }
        // The dashed ecliptic, only the dashes that land on screen. Its
        // centre and its dash pattern both turn with the sky.
        self.ecliptic.clear();
        let c = f.xy(0.2, 0.42);
        let rad = f.r * 0.62;
        let n = ((std::f32::consts::TAU * rad / (DASH * 2.)).floor() as usize).clamp(8, 6000);
        let step = std::f32::consts::TAU / n as f32;
        for i in 0..n {
            let a0 = i as f32 * step + f.rot;
            let a1 = a0 + step / 2.;
            let p0 = [c[0] + rad * a0.cos(), c[1] + rad * a0.sin()];
            if p0[0] < -8. || p0[1] < -8. || p0[0] > w + 8. || p0[1] > h + 8. {
                continue;
            }
            self.ecliptic
                .extend([p0, [c[0] + rad * a1.cos(), c[1] + rad * a1.sin()]]);
        }
    }
}

impl Ambient for Stars {
    fn update(&mut self, cx: &Ctx) {
        self.t = cx.t;
        self.w = cx.size.w;
        self.h = cx.size.h;
        self.f = Frame::at(self.w, self.h, self.t);
        self.sync_names(cx);
        if cx.input.submit || cx.signals.started > 0 {
            self.place_nova();
        }
        self.cache();
    }

    fn draw(&self, cx: &DrawCtx, out: &mut Scene) {
        let (w, h, p, f) = (cx.size.w, cx.size.h, cx.pal, self.f);
        // Graticule: declination circles and hour lines.
        for k in 1..=10 {
            out.circle([f.cx, f.cy], k as f32 * f.r / 10., None, Some((1., p.rule)));
        }
        let mut hours = Vec::with_capacity(48);
        for j in 0..24 {
            let a = j as f32 * std::f32::consts::PI / 12. + f.rot;
            hours.extend([[f.cx, f.cy], [f.cx + a.cos() * f.r, f.cy + a.sin() * f.r]]);
        }
        out.segments(hours, 1., p.rule);
        out.segments(self.ecliptic.clone(), 1., p.rule2);
        for j in (0..24).step_by(2) {
            let a = j as f32 * std::f32::consts::PI / 12. + f.rot + 0.02;
            let (x, y) = (f.cx + a.cos() * f.r * 0.45, f.cy + a.sin() * f.r * 0.45);
            if x > 0. && x < w && y > 0. && y < h {
                out.text(
                    [x, y],
                    format!("{j}h"),
                    Font::Mono,
                    10.,
                    p.soft,
                    Align::Center,
                );
            }
        }
        // Stars by magnitude.
        for &(c, r) in &self.big {
            out.circle(c, r, Some(p.pencil), None);
        }
        out.dots(MAG_R[4] * 2., p.pencil, self.dots[0].clone());
        out.dots(MAG_R[5] * 2., p.pencil, self.dots[1].clone());
        // Constellations: your machines.
        for c in &self.cons {
            let ps: Vec<Pt> = c.pts.iter().map(|&(r, th)| f.xy(r, th)).collect();
            let (mx, my) = ps.iter().fold((0., 0.), |(a, b), q| (a + q[0], b + q[1]));
            let n = ps.len() as f32;
            for &q in &ps {
                out.circle(q, 2., Some(p.pencil), None);
            }
            out.line(ps, 0.8, p.soft);
            if c.label.a > 0.01 {
                out.text(
                    [mx / n, my / n + 22.],
                    c.label.name.clone(),
                    Font::Book,
                    14.,
                    p.soft.alpha(c.label.a),
                    Align::Center,
                );
            }
        }
        // Named stars: the project and recent sessions.
        for (i, s) in self.named.iter().enumerate() {
            if s.name.is_empty() || s.a < 0.01 {
                continue;
            }
            let (r, th) = Stars::named_at(i);
            let [x, y] = f.xy(r, th);
            if x < -80. || y < -20. || x > w + 20. || y > h + 20. {
                continue;
            }
            out.circle([x, y], 3., Some(p.pencil.alpha(s.a)), None);
            out.circle([x, y], 6., None, Some((0.8, p.pencil.alpha(s.a))));
            out.text(
                [x + 9., y + 3.],
                s.name.clone(),
                Font::Mono,
                10.,
                p.soft.alpha(s.a),
                Align::Left,
            );
        }
        // Novas: sessions started.
        for (i, nv) in self.novas.iter().enumerate() {
            let [x, y] = f.xy(nv.r, nv.th);
            let age = (cx.t - nv.t0).max(0.);
            if age < RING_LIFE {
                out.circle(
                    [x, y],
                    4. + age * 26.,
                    None,
                    Some((1.2, p.accent.alpha(1. - age / RING_LIFE))),
                );
            }
            let col = p.accent.alpha(clamp(age / NOVA_FADE, 0., 1.));
            out.circle([x, y], 3., Some(col), None);
            out.segments(
                vec![[x - 9., y], [x + 9., y], [x, y - 9.], [x, y + 9.]],
                1.2,
                col,
            );
            if i + 1 == self.novas.len() {
                let left = x + 90. > w;
                let (at, align) = if left {
                    ([x - 12., y + 3.], Align::Right)
                } else {
                    ([x + 12., y + 3.], Align::Left)
                };
                out.text(at, "new session", Font::Mono, 10., col, align);
            }
        }
    }
}
