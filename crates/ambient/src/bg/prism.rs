//! Prism: a dispersing prism hung on a thread. The prompt is the beam and
//! the kinds of tool call it splits into are the spectrum. Each colour is
//! traced by Snell's law into the glass, reflected internally (up to four
//! bounces) until it can refract out, and the bands are as wide as each
//! kind's rolling share of recent calls.

use crate::geom::{clip_band, ray_segment, spread};
use crate::math::{clamp, ease};
use crate::scene::{Align, Font, Paint, Pt, text_width};
use crate::signals::KINDS;
use crate::{Ambient, Ctx, DrawCtx, Scene, Spec};

pub const SPEC: Spec = Spec {
    id: "prism",
    name: "Prism",
    from: "A dispersing prism hung on a thread. Light refracts by Snell's law at each face and violet bends more than red, so the spectrum fans toward the base (dispersion exaggerated so you can see it).",
    shows: "Your prompt as the beam and the work it splits into as the spectrum. Each band is a kind of tool call, as wide as its share of the last minute or so of calls across your sessions. Point to the left of the prism to aim the beam; aim low and the colours peel away into the glass, violet first.",
    make: |_| Box::new(Prism::new()),
};

/// Shares before any calls are seen, in `KINDS` order.
const PRIOR: [f32; 6] = [0.12, 0.3, 0.14, 0.22, 0.16, 0.06];
/// Weight of the prior, in calls, so a few calls only nudge the mix.
const PRIOR_CALLS: f32 = 12.;
/// Time constant of the rolling call counts (s).
const SHARE_TAU: f32 = 60.;
/// How fast drawn shares follow the counts (per second).
const SHARE_RATE: f32 = 0.8;
/// No band ever gets narrower than this share.
const MIN_SHARE: f32 = 0.04;
/// Refractive index at the red and violet ends (exaggerated dispersion).
const N_LO: f32 = 1.46;
const N_HI: f32 = 1.74;
/// Sub-rays traced per band.
const SUB: usize = 6;
/// Face hits followed per ray: the exit plus up to four internal reflections.
const MAX_HITS: usize = 5;
/// Incidence limits (degrees from the entry face's normal).
const INC_MIN: f32 = -30.;
const INC_MAX: f32 = 85.;
/// The pointer aims fully once it's this far left of the entry point (pt).
const AIM_RAMP: f32 = 80.;
/// Seconds the pulse takes to travel down the beam, then out the spectrum.
const PULSE_IN: f32 = 0.4;
const PULSE_OUT: f32 = 1.8;
/// Pulse speed along the spectrum (pt/s) and half its length (pt).
const PULSE_SPEED: f32 = 900.;
const PULSE_HALF: f32 = 70.;
/// Glow decay (per second) and glow per key.
const GLOW_DECAY: f32 = 2.;
const GLOW_PER_KEY: f32 = 0.05;
/// Colour order from the top of the fan: orange (lowest index) to violet.
const ORDER: [usize; 6] = [5, 4, 3, 2, 1, 0];
const SAMPLES: usize = 6 * SUB + 1;

/// One traced sub-ray: where it leaves the glass, which way, the light
/// path inside, and a signature of the faces it hit.
#[derive(Clone, Copy)]
struct Ray {
    ex: Pt,
    out: Pt,
    path: [Pt; MAX_HITS + 1],
    len: usize,
    /// Faces hit, base 4 after a leading 1. Below 16 means it went straight out.
    sig: u32,
}

#[derive(Clone, Copy)]
struct Strip {
    /// Band, 0..6 in `ORDER`.
    k: usize,
    /// First and last sample index.
    a: usize,
    b: usize,
}

struct Prism {
    counts: [f32; 6],
    share: [f32; 6],
    glow: f32,
    pulse: f32,
    // Geometry for the current frame.
    w: f32,
    h: f32,
    r: f32,
    cx: f32,
    tri: [Pt; 3],
    entry: Pt,
    d: Pt,
    src: Pt,
    reach: f32,
    rays: Vec<Option<Ray>>,
    strips: Vec<Strip>,
    /// Widest strip per kind: (width, mid exit, mean direction).
    best: [Option<(usize, Pt, Pt)>; 6],
    right: Vec<f32>,
    right_kind: Vec<usize>,
    bottom: Vec<f32>,
    bottom_kind: Vec<usize>,
    lost: usize,
}

fn refract(i: Pt, n: Pt, eta: f32) -> Option<Pt> {
    let c = -(i[0] * n[0] + i[1] * n[1]);
    let k = 1. - eta * eta * (1. - c * c);
    if k < 0. {
        return None;
    }
    let f = eta * c - k.sqrt();
    Some([eta * i[0] + f * n[0], eta * i[1] + f * n[1]])
}

/// `v` limited to `lo..=hi`, preferring `lo` when the range is empty.
fn keep_in(v: f32, lo: f32, hi: f32) -> f32 {
    v.min(hi).max(lo)
}

impl Prism {
    fn new() -> Self {
        Prism {
            counts: [0.; 6],
            share: PRIOR,
            glow: 0.,
            pulse: -9.,
            w: 1.,
            h: 1.,
            r: 34.,
            cx: 0.,
            tri: [[0.; 2]; 3],
            entry: [0.; 2],
            d: [1., 0.],
            src: [0.; 2],
            reach: 0.,
            rays: Vec::with_capacity(SAMPLES),
            strips: Vec::with_capacity(SAMPLES),
            best: [None; 6],
            right: Vec::new(),
            right_kind: Vec::new(),
            bottom: Vec::new(),
            bottom_kind: Vec::new(),
            lost: 0,
        }
    }

    /// Entry refraction, then up to four internal reflections until the ray
    /// can refract out.
    fn trace(&self, n: f32) -> Option<Ray> {
        let [a, b, c] = self.tri;
        let faces = [
            (a, b, [0.866, -0.5]),
            (b, c, [0., 1.]),
            (c, a, [-0.866, -0.5]),
        ];
        let nn = [0.866f32, 0.5];
        let mut t = refract(self.d, [-nn[0], -nn[1]], 1. / n)?;
        let mut o = self.entry;
        let mut from = 2;
        let mut ray = Ray {
            ex: o,
            out: [0.; 2],
            path: [o; MAX_HITS + 1],
            len: 1,
            sig: 1,
        };
        for _ in 0..MAX_HITS {
            let mut best: Option<(f32, Pt, usize)> = None;
            for (fi, &(p, q, out)) in faces.iter().enumerate() {
                if fi == from {
                    continue;
                }
                if let Some(tt) = ray_segment(o, t, p, q)
                    && best.is_none_or(|b| tt < b.0)
                {
                    best = Some((tt, out, fi));
                }
            }
            let (tt, out, fi) = best?;
            let ex = [o[0] + t[0] * tt, o[1] + t[1] * tt];
            ray.path[ray.len] = ex;
            ray.len += 1;
            ray.sig = ray.sig * 4 + fi as u32;
            if let Some(dir) = refract(t, [-out[0], -out[1]], n) {
                ray.ex = ex;
                ray.out = dir;
                return Some(ray);
            }
            let dn = t[0] * out[0] + t[1] * out[1];
            t = [t[0] - 2. * dn * out[0], t[1] - 2. * dn * out[1]];
            o = ex;
            from = fi;
        }
        None
    }

    fn update_shares(&mut self, cx: &Ctx) {
        let decay = (-cx.dt / SHARE_TAU).exp();
        for c in &mut self.counts {
            *c *= decay;
        }
        for &k in &cx.signals.calls {
            self.counts[k as usize] += 1.;
        }
        let total: f32 = self.counts.iter().sum::<f32>() + PRIOR_CALLS;
        let target: [f32; 6] = std::array::from_fn(|i| {
            ((self.counts[i] + PRIOR_CALLS * PRIOR[i]) / total).max(MIN_SHARE)
        });
        let sum: f32 = target.iter().sum();
        let k = ease(SHARE_RATE, cx.dt);
        for (s, t) in self.share.iter_mut().zip(target) {
            *s += (t / sum - *s) * k;
        }
    }

    fn polygon(&self, st: &Strip, far: f32) -> Vec<Pt> {
        let rs = || self.rays[st.a..=st.b].iter().flatten();
        let mut poly: Vec<Pt> = rs().map(|r| r.ex).collect();
        let tail: Vec<Pt> = rs()
            .map(|r| [r.ex[0] + r.out[0] * far, r.ex[1] + r.out[1] * far])
            .collect();
        poly.extend(tail.into_iter().rev());
        poly
    }

    fn mid(&self, st: &Strip) -> (Pt, Pt) {
        let (r0, r1) = (self.rays[st.a].unwrap(), self.rays[st.b].unwrap());
        let m = [(r0.ex[0] + r1.ex[0]) / 2., (r0.ex[1] + r1.ex[1]) / 2.];
        let mo = [(r0.out[0] + r1.out[0]) / 2., (r0.out[1] + r1.out[1]) / 2.];
        let ml = mo[0].hypot(mo[1]).max(1e-6);
        (m, [mo[0] / ml, mo[1] / ml])
    }
}

impl Ambient for Prism {
    fn update(&mut self, cx: &Ctx) {
        let (w, h, t) = (cx.size.w, cx.size.h, cx.t);
        self.glow =
            (self.glow * (-cx.dt * GLOW_DECAY).exp() + cx.input.keys as f32 * GLOW_PER_KEY).min(1.);
        if cx.input.submit || cx.signals.started > 0 {
            self.pulse = t;
        }
        self.update_shares(cx);

        // The prism and the beam.
        let mn = w.min(h);
        let pcx = w * 0.45;
        let pcy = h * if h > 600. { 0.25 } else { 0.3 };
        let r = (mn * if mn < 400. { 0.24 } else { 0.17 }).max(34.);
        let a = [pcx, pcy - r];
        let b = [pcx + 0.866 * r, pcy + 0.5 * r];
        let c = [pcx - 0.866 * r, pcy + 0.5 * r];
        let entry = [(a[0] + c[0]) / 2., (a[1] + c[1]) / 2.];
        let idle = 62. + 7. * (t * 0.23).sin() + 3. * (t * 0.61).sin();
        let mut inc = idle;
        if let Some(p) = cx.input.pointer {
            let k = p.k * clamp((entry[0] - p.x) / AIM_RAMP, 0., 1.);
            let ip = clamp(
                30. - (entry[1] - p.y).atan2(entry[0] - p.x).to_degrees(),
                INC_MIN,
                INC_MAX,
            );
            inc = idle + (ip - idle) * k;
        }
        let da = (30. - inc).to_radians();
        let d = [da.cos(), da.sin()];
        let reach = (entry[0] + 40.) / d[0].max(0.3);
        (
            self.w, self.h, self.r, self.cx, self.tri, self.entry, self.d, self.reach,
        ) = (w, h, r, pcx, [a, b, c], entry, d, reach);
        self.src = [entry[0] - d[0] * reach, entry[1] - d[1] * reach];

        // Trace each sub-ray.
        let mut cum = [0.; 7];
        for (k, &ci) in ORDER.iter().enumerate() {
            cum[k + 1] = cum[k] + self.share[ci];
        }
        let mut rays = std::mem::take(&mut self.rays);
        rays.clear();
        for k in 0..6 {
            for j in 0..SUB {
                let f = cum[k] + (cum[k + 1] - cum[k]) * j as f32 / SUB as f32;
                rays.push(self.trace(N_LO + (N_HI - N_LO) * f));
            }
        }
        rays.push(self.trace(N_HI));
        self.rays = rays;

        // Neighbouring sub-rays of one colour that share an exit path make a strip.
        self.strips.clear();
        for j in 0..self.rays.len() - 1 {
            let k = j / SUB;
            let (Some(ra), Some(rb)) = (self.rays[j], self.rays[j + 1]) else {
                continue;
            };
            if ra.sig != rb.sig {
                continue;
            }
            match self.strips.last_mut() {
                Some(s)
                    if s.k == k && s.b == j && self.rays[s.a].is_some_and(|r| r.sig == ra.sig) =>
                {
                    s.b = j + 1
                }
                _ => self.strips.push(Strip { k, a: j, b: j + 1 }),
            }
        }

        // Labels at the edge each colour's widest strip runs into.
        self.best = [None; 6];
        let mut direct = [false; 6];
        for st in &self.strips {
            let ci = ORDER[st.k];
            let width = st.b - st.a + 1;
            if self.best[ci].is_none_or(|b| b.0 < width) {
                let (m, mo) = self.mid(st);
                self.best[ci] = Some((width, m, mo));
            }
            if self.rays[st.a].is_some_and(|r| r.sig < 16) {
                direct[ci] = true;
            }
        }
        self.lost = 6
            - (0..6)
                .filter(|&i| self.best[i].is_some() && direct[i])
                .count();
        self.right.clear();
        self.right_kind.clear();
        self.bottom.clear();
        self.bottom_kind.clear();
        for (ci, best) in self.best.iter().enumerate() {
            let Some((_, m, mo)) = *best else { continue };
            let tx = if mo[0] > 0.02 {
                (w - 12. - m[0]) / mo[0]
            } else {
                f32::INFINITY
            };
            let ty = if mo[1] > 0.02 {
                (h - 10. - m[1]) / mo[1]
            } else {
                f32::INFINITY
            };
            if tx < ty && tx > 0. {
                let y = m[1] + mo[1] * tx + 3.;
                if y > 0. && y < h {
                    self.right.push(y);
                    self.right_kind.push(ci);
                }
            } else if ty < f32::INFINITY && ty > 0. {
                let x = m[0] + mo[0] * ty;
                if x > 0. && x < w {
                    self.bottom.push(x);
                    self.bottom_kind.push(ci);
                }
            }
        }
        spread(&mut self.right, 14., 12., h - 6.);
        spread(&mut self.bottom, 64., 40., w - 40.);
    }

    fn draw(&self, cx: &DrawCtx, out: &mut Scene) {
        let (w, h, p) = (self.w, self.h, cx.pal);
        let [a, b, c] = self.tri;
        let (entry, d, src, r) = (self.entry, self.d, self.src, self.r);
        let bright = 0.5 + self.glow * 0.5;
        let pa = cx.t - self.pulse;

        // The thread, the beam and its faint partial reflection.
        out.line(
            vec![[self.cx + 0.5, 0.], [self.cx + 0.5, a[1]]],
            0.8,
            p.soft,
        );
        for (lw, al) in [(7., 0.08), (3., 0.22), (1.2, 1.)] {
            out.line(vec![src, entry], lw, p.pencil.alpha(al * bright));
        }
        let n = [0.866, 0.5];
        let dn = d[0] * n[0] + d[1] * n[1];
        let rf = [d[0] - 2. * dn * n[0], d[1] - 2. * dn * n[1]];
        out.line(
            vec![
                entry,
                [entry[0] + rf[0] * r * 1.4, entry[1] + rf[1] * r * 1.4],
            ],
            1.,
            p.pencil.alpha(0.18 * bright),
        );
        if (0. ..PULSE_IN).contains(&pa) {
            let k = pa / PULSE_IN;
            out.circle(
                [
                    src[0] + (entry[0] - src[0]) * k,
                    src[1] + (entry[1] - src[1]) * k,
                ],
                3.,
                Some(p.accent),
                None,
            );
        }
        out.fill(vec![a, b, c], Paint::Solid(p.bg.alpha(0.6)));

        // Light inside the glass, including any internal reflections.
        for (j, ray) in self.rays.iter().enumerate() {
            if let Some(ray) = ray {
                let ci = ORDER[(j / SUB).min(5)];
                out.line(
                    ray.path[..ray.len].to_vec(),
                    1.,
                    p.spec[ci].alpha(0.22 * bright),
                );
            }
        }

        // The spectrum.
        let far = (w + h) * 2.;
        let diag = w.hypot(h);
        for st in &self.strips {
            let col = p.spec[ORDER[st.k]];
            let (m, mo) = self.mid(st);
            let len = ((w - m[0]) / mo[0].max(0.3))
                .max((h - m[1]) / mo[1].max(0.3))
                .min(diag)
                .max(80.);
            let poly = self.polygon(st, far);
            let to = [m[0] + mo[0] * len, m[1] + mo[1] * len];
            out.fill_ramp(
                &poly,
                m,
                to,
                &[
                    (0., col.alpha(0.12)),
                    (0.35, col.alpha(0.62)),
                    (0.7, col.alpha(0.85)),
                    (1., col.alpha(0.92)),
                ],
            );
            if pa > PULSE_IN && pa < PULSE_IN + PULSE_OUT {
                let dd = (pa - PULSE_IN) * PULSE_SPEED;
                let band = clip_band(&poly, m, mo, dd - PULSE_HALF, dd + PULSE_HALF);
                let from = [
                    m[0] + mo[0] * (dd - PULSE_HALF),
                    m[1] + mo[1] * (dd - PULSE_HALF),
                ];
                let to = [
                    m[0] + mo[0] * (dd + PULSE_HALF),
                    m[1] + mo[1] * (dd + PULSE_HALF),
                ];
                let peak = 0.7 * (1. - (pa - PULSE_IN) / PULSE_OUT);
                out.fill_ramp(
                    &band,
                    from,
                    to,
                    &[
                        (0., p.raised.alpha(0.)),
                        (0.5, p.raised.alpha(peak)),
                        (1., p.raised.alpha(0.)),
                    ],
                );
            }
        }

        let text = |ci: usize| format!("{} {}%", KINDS[ci], (self.share[ci] * 100.).round() as i32);
        for (&y, &ci) in self.right.iter().zip(&self.right_kind) {
            out.label([w - 12., y], text(ci), p.pencil, p.bg, Align::Right);
        }
        for (&x, &ci) in self.bottom.iter().zip(&self.bottom_kind) {
            out.label([x, h - 8.], text(ci), p.pencil, p.bg, Align::Center);
        }

        // The glass, its hanger and the entry point.
        out.outline(vec![a, b, c], 1.3, p.pencil);
        out.line(
            vec![[a[0] - 9., a[1] + 15.], [a[0], a[1] + 2.]],
            1.4,
            p.accent,
        );
        out.circle(entry, 2.2, Some(p.pencil), None);
        let back = self.reach.min(140.);
        let lp = [entry[0] - d[0] * back, entry[1] - d[1] * back];
        out.text(
            [
                keep_in(lp[0], 6., w - 60.),
                keep_in(lp[1] - 10., 12., h - 4.),
            ],
            "PROMPT",
            Font::Mono,
            10.,
            p.soft,
            Align::Left,
        );
        if self.lost > 0 {
            let s = if self.lost > 1 { "s" } else { "" };
            let note = format!("{} colour{s} reflected inside the glass", self.lost);
            let x = keep_in(b[0] + 14., 6., w - text_width(&note, Font::Mono, 10.) - 6.);
            out.label([x, a[1] + r * 0.35], note, p.soft, p.bg, Align::Left);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Input, Pointer, Signals, Size};

    fn run(pointer: Option<Pointer>) -> Prism {
        let mut p = Prism::new();
        let sig = Signals::default();
        let input = Input {
            pointer,
            ..Default::default()
        };
        let cx = Ctx {
            t: 1.,
            dt: 0.016,
            size: Size { w: 1440., h: 900. },
            input: &input,
            signals: &sig,
        };
        p.update(&cx);
        p
    }

    #[test]
    fn spectrum_fans_out_and_labels_every_colour() {
        let p = run(None);
        assert!(!p.strips.is_empty());
        assert_eq!(
            p.right.len() + p.bottom.len(),
            6 - p.best.iter().filter(|b| b.is_none()).count()
        );
        assert!(p.right.len() + p.bottom.len() >= 5);
    }

    #[test]
    fn aiming_low_reflects_colours_inside() {
        let idle = run(None);
        let p = run(Some(Pointer {
            x: 100.,
            y: 20.,
            k: 1.,
        }));
        assert!(p.lost > idle.lost, "lost {} vs idle {}", p.lost, idle.lost);
    }
}
