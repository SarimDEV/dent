//! Survey network: your machines as numbered stations of a triangulation
//! survey, joined by Delaunay legs. One leg at a time is measured, tick by
//! tick, and labelled with its latency. A scale bar and a north arrow finish
//! the sheet. Each station's position is seeded from its machine's name, so
//! the map stays put from frame to frame and run to run.

use crate::geom::{delaunay, edges};
use crate::math::{Rng, clamp, out_cubic};
use crate::scene::{Align, Font, Paint, Pt, fit};
use crate::{Ambient, Ctx, DrawCtx, Scene, Spec};

pub const SPEC: Spec = Spec {
    id: "survey",
    name: "Survey network",
    from: "Triangulation surveys: numbered stations joined by measured legs, a scale bar and a north arrow.",
    shows: "Your machines as stations. Each leg is labelled with its latency: measured where dent knows it, estimated (~) where it doesn't.",
    make: |seed| Box::new(Survey::new(seed)),
};

/// Seconds each leg is measured for before the next.
const LEG: f32 = 2.6;
/// Machines drawn as stations at most.
const MAX_MACHINES: usize = 12;
/// Extra stations from sessions you start, at most.
const MAX_EXTRA: usize = 3;
/// Candidate spots tried for each new station.
const TRIES: usize = 16;
/// Stations keep at least this far apart (fraction of the screen).
const MIN_GAP: f32 = 0.2;
/// Station fade-in (s).
const FADE: f32 = 0.4;
/// Tick spacing along the measured leg (pt).
const TICK: f32 = 8.;
/// Points per estimated millisecond, matching the scale bar.
const PT_PER_MS: f32 = 6.;

struct Station {
    name: String,
    fx: f32,
    fy: f32,
    t0: f32,
    extra: bool,
    latency: Option<f32>,
}

struct Survey {
    rng: Rng,
    st: Vec<Station>,
    here: String,
    leg: usize,
    leg_t: f32,
    t: f32,
    size: (f32, f32),
    dirty: bool,
    started: bool,
    // Cached when stations or size change.
    pts: Vec<Pt>,
    edges: Vec<(usize, usize)>,
}

/// A stable seed from a name (FNV-1a).
fn name_seed(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
    })
}

impl Survey {
    fn new(seed: u64) -> Self {
        let mut rng = Rng::new(seed ^ 0x5u64);
        let leg = rng.index(20);
        Survey {
            rng,
            st: Vec::new(),
            here: String::new(),
            leg,
            leg_t: 0.,
            t: 0.,
            size: (0., 0.),
            dirty: true,
            started: false,
            pts: Vec::new(),
            edges: Vec::new(),
        }
    }

    fn nearest(&self, fx: f32, fy: f32) -> f32 {
        self.st
            .iter()
            .map(|q| (q.fx - fx).hypot(q.fy - fy))
            .fold(f32::MAX, f32::min)
    }

    /// Bring the machine stations in line with `signals.machines`, keeping
    /// the place of every machine already on the map.
    fn sync(&mut self, cx: &Ctx) {
        let ms = &cx.signals.machines[..cx.signals.machines.len().min(MAX_MACHINES)];
        let same = {
            let mut cur = self.st.iter().filter(|s| !s.extra);
            ms.iter()
                .enumerate()
                .filter(|(i, m)| !ms[..*i].iter().any(|o| o.name == m.name))
                .all(|(_, m)| cur.next().is_some_and(|s| s.name == m.name))
                && cur.next().is_none()
        };
        if !same {
            let born = if self.started { cx.t } else { -10. };
            let old = std::mem::take(&mut self.st);
            let (mut keep, extras): (Vec<Station>, Vec<Station>) =
                old.into_iter().partition(|s| !s.extra);
            for m in ms {
                if self.st.iter().any(|s| s.name == m.name) {
                    continue;
                }
                if let Some(i) = keep.iter().position(|s| s.name == m.name) {
                    self.st.push(keep.swap_remove(i));
                    continue;
                }
                let mut r = Rng::new(name_seed(&m.name));
                let (mut fx, mut fy) = (0.5, 0.5);
                for _ in 0..60 {
                    fx = 0.08 + r.f() * 0.84;
                    fy = 0.14 + r.f() * 0.74;
                    if self.nearest(fx, fy) >= MIN_GAP
                        && extras
                            .iter()
                            .all(|q| (q.fx - fx).hypot(q.fy - fy) >= MIN_GAP)
                    {
                        break;
                    }
                }
                self.st.push(Station {
                    name: m.name.clone(),
                    fx,
                    fy,
                    t0: born,
                    extra: false,
                    latency: None,
                });
            }
            self.st.extend(extras);
            self.dirty = true;
        }
        self.started = true;
        for s in self.st.iter_mut().filter(|s| !s.extra) {
            s.latency = ms
                .iter()
                .find(|m| m.name == s.name)
                .and_then(|m| m.latency_ms)
                .filter(|v| v.is_finite());
        }
        self.here = cx.signals.machine.clone().unwrap_or_default();
    }

    fn add_extra(&mut self) {
        let extras: Vec<usize> = (0..self.st.len()).filter(|&i| self.st[i].extra).collect();
        if extras.len() >= MAX_EXTRA {
            self.st.remove(extras[0]);
        }
        let (mut bx, mut by, mut bd) = (0.5, 0.5, -1.);
        for _ in 0..TRIES {
            let (fx, fy) = (0.1 + self.rng.f() * 0.8, 0.15 + self.rng.f() * 0.72);
            let d = self.nearest(fx, fy);
            if d > bd {
                (bx, by, bd) = (fx, fy, d);
            }
        }
        self.st.push(Station {
            name: "new".into(),
            fx: bx,
            fy: by,
            t0: self.t,
            extra: true,
            latency: None,
        });
        self.leg_t = 0.;
        self.dirty = true;
    }

    /// The leg being measured, as station indices.
    fn current_leg(&self) -> Option<(usize, usize)> {
        (!self.edges.is_empty()).then(|| self.edges[self.leg % self.edges.len()])
    }

    /// The leg's latency label: measured when it runs from this machine to
    /// one whose latency is known, otherwise estimated from its length.
    fn leg_label(&self, a: usize, b: usize, d: f32) -> String {
        let (sa, sb) = (&self.st[a], &self.st[b]);
        let measured = if !sa.extra && sa.name == self.here {
            sb.latency
        } else if !sb.extra && sb.name == self.here {
            sa.latency
        } else {
            None
        };
        match measured {
            Some(ms) => format!("{} ms", ms.round() as i64),
            None => format!("~{} ms", (d / PT_PER_MS).round() as i64 + 3),
        }
    }
}

impl Ambient for Survey {
    fn update(&mut self, cx: &Ctx) {
        self.t = cx.t;
        self.sync(cx);
        self.leg_t += cx.dt;
        if self.leg_t > LEG {
            self.leg_t = 0.;
            self.leg = self.leg.wrapping_add(1);
        }
        if cx.input.submit {
            self.add_extra();
        }
        let size = (cx.size.w, cx.size.h);
        if self.dirty || size != self.size {
            self.size = size;
            self.dirty = false;
            self.pts.clear();
            self.pts
                .extend(self.st.iter().map(|s| [s.fx * size.0, s.fy * size.1]));
            self.edges = edges(&delaunay(&self.pts));
            if self.edges.is_empty() && self.pts.len() == 2 {
                self.edges.push((0, 1));
            }
        }
    }

    fn draw(&self, cx: &DrawCtx, out: &mut Scene) {
        let (w, h, p) = (cx.size.w, cx.size.h, cx.pal);
        if self.pts.len() != self.st.len() {
            return;
        }
        let fade = |s: &Station| clamp((cx.t - s.t0) / FADE, 0., 1.);
        // The network: each leg as faint as its fainter end.
        for &(a, b) in &self.edges {
            let k = fade(&self.st[a]).min(fade(&self.st[b]));
            out.line(vec![self.pts[a], self.pts[b]], 0.8, p.soft.alpha(k));
        }
        // The leg being measured.
        if let Some((ia, ib)) = self.current_leg() {
            let (a, b) = (self.pts[ia], self.pts[ib]);
            let d = (b[0] - a[0]).hypot(b[1] - a[1]).max(1.);
            let (ux, uy) = ((b[0] - a[0]) / d, (b[1] - a[1]) / d);
            let k = out_cubic(self.leg_t.min(1.));
            out.line(
                vec![a, [a[0] + ux * d * k, a[1] + uy * d * k]],
                1.6,
                p.accent,
            );
            let mut ticks = Vec::new();
            let mut s = TICK;
            while s < d * k {
                let (x, y) = (a[0] + ux * s, a[1] + uy * s);
                ticks.extend([[x - uy * 3., y + ux * 3.], [x + uy * 3., y - ux * 3.]]);
                s += TICK;
            }
            out.segments(ticks, 1., p.accent);
            let la = clamp((self.leg_t - 1.) / 0.3, 0., 1.);
            if la > 0. {
                let at = [(a[0] + b[0]) / 2., (a[1] + b[1]) / 2. + 3.];
                out.label(
                    at,
                    self.leg_label(ia, ib, d),
                    p.accent.alpha(la),
                    p.bg.alpha(la),
                    Align::Center,
                );
            }
        }
        // Stations.
        for (i, st) in self.st.iter().enumerate() {
            let [x, y] = self.pts[i];
            let k = fade(st);
            if k <= 0. {
                continue;
            }
            let ink = if st.extra { p.accent } else { p.pencil }.alpha(k);
            let tri = vec![[x, y - 7.], [x + 6., y + 4.], [x - 6., y + 4.]];
            out.fill(tri.clone(), Paint::Solid(p.bg.alpha(k)));
            out.outline(tri, 1.2, ink);
            out.rect(x - 1., y - 1., 2., 2., Paint::Solid(ink));
            out.text(
                [x, y - 12.],
                format!("STN {}", i + 1),
                Font::Mono,
                10.,
                p.soft.alpha(k),
                Align::Center,
            );
            out.text(
                [x, y + 19.],
                fit(&st.name, Font::Book, 13., 140.),
                Font::Book,
                13.,
                ink,
                Align::Center,
            );
        }
        // Scale bar, in estimated milliseconds.
        if w > 260. {
            let (sx, sy) = (20., h - 18.);
            let mut bar = vec![[sx, sy], [sx + 150., sy]];
            for k in [0., 50., 100., 150.] {
                bar.extend([[sx + k + 0.5, sy - 4.], [sx + k + 0.5, sy]]);
                out.text(
                    [sx + k, sy - 8.],
                    format!("{}", (k / PT_PER_MS).round()),
                    Font::Mono,
                    10.,
                    p.soft,
                    Align::Center,
                );
            }
            out.segments(bar, 1., p.pencil);
            out.rect(sx, sy - 2., 50., 2., Paint::Solid(p.pencil));
            out.rect(sx + 100., sy - 2., 50., 2., Paint::Solid(p.pencil));
            out.text(
                [sx + 158., sy + 3.],
                "ms",
                Font::Mono,
                10.,
                p.soft,
                Align::Left,
            );
        }
        // North arrow.
        let (nx, ny) = (w - 24., 26.);
        out.line(vec![[nx, ny + 28.], [nx, ny + 6.]], 1., p.pencil);
        out.fill(
            vec![[nx, ny + 2.], [nx + 4., ny + 10.], [nx - 4., ny + 10.]],
            Paint::Solid(p.pencil),
        );
        out.text(
            [nx, ny - 2.],
            "N",
            Font::Display,
            16.,
            p.pencil,
            Align::Center,
        );
    }
}
