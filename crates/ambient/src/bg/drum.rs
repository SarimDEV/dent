//! Drum recorder: a seismograph's fixed pens over paper moving left. Each
//! tremor rings as a damped oscillation and big ones are soft-limited by
//! the pen's travel. Rows are your sessions; tool calls shake the row of a
//! working session, and your own keystrokes shake the cyan "you" trace.

use std::collections::VecDeque;

use crate::math::{Rng, clamp, ease, hash, noise3, out_cubic};
use crate::scene::{Align, Font, Paint, Pt, fit};
use crate::{Ambient, Ctx, DrawCtx, Kind, Scene, Spec};

pub const SPEC: Spec = Spec {
    id: "drum",
    name: "Drum recorder",
    from: "Seismograph drums: fixed pens over moving paper. Each tremor rings as a damped oscillation, and big ones are clipped by the pen's travel.",
    shows: "One trace per session; each tool call shakes the trace of a working session, harder for edits and commands. Your own keystrokes register as tremors on the cyan line, and starting a session marks it.",
    make: |seed| Box::new(Drum::new(seed)),
};

/// Paper speed (pt/s).
const SPEED: f32 = 34.;
/// Seconds between time ticks; every second tick carries a clock label.
const TICK: f64 = 5.;
/// The pens sit this far from the right edge (pt).
const PEN_INSET: f32 = 64.;
/// Row pitch (pt), and where the first row's band starts.
const ROW: f32 = 30.;
const TOP: f32 = 40.;
/// Rows kept at most (a 4K-tall screen needs ~70).
const MAX_ROWS: usize = 90;
/// Pen travel (pt): the soft limit on a trace's swing.
const TRAVEL: f32 = 26.;
/// A tremor rings for this much paper (pt), decaying over `DECAY`.
const RING: f32 = 180.;
const DECAY: f32 = 34.;
/// Oscillation (radians per pt of paper).
const FREQ: f32 = 0.38;
/// The ring shape is tabulated at this spacing (pt) and interpolated.
const KSTEP: f32 = 0.25;
/// Trace sample spacing (pt).
const SAMPLE: f32 = 2.;
/// Along quiet paper, only every this-many samples is drawn.
const QUIET_EVERY: usize = 4;
/// Tremors are pruned once this far behind the left edge (pt).
const PRUNE: f32 = 300.;
/// Tremors kept per row at most.
const MAX_TREMORS: usize = 160;
/// Live tremors sit at least this far apart on the paper (pt).
const LIVE_GAP: f32 = 5.;
/// Call tremors closer than this to the previous one on the row merge into it.
const CALL_GAP: f32 = 5.;
/// Session marks sit at least this far apart (pt).
const MARK_GAP: f32 = 70.;
/// Submit tremor amplitude (pt).
const SUBMIT_A: f32 = 24.;
/// Clock offsets that drift further than this (s) re-sync.
const RESYNC: f64 = 90.;

#[derive(Clone, Copy)]
struct Tremor {
    /// Paper position (pt) where it started.
    u: f32,
    a: f32,
    mark: bool,
}

#[derive(Default)]
struct Row {
    name: String,
    working: bool,
    /// Eases to 1 while the session is working, 0 while idle.
    lit: f32,
    ev: VecDeque<Tremor>,
    /// The trace, ready to draw.
    trace: Vec<Pt>,
    /// Pen noise per paper sample, from sample `k0` on.
    noise: VecDeque<f32>,
    k0: i64,
}

struct Drum {
    seed: f32,
    rng: Rng,
    rows: Vec<Row>,
    live: usize,
    last_live: f32,
    calls: u32,
    /// Wall clock seconds minus `t`, once known.
    clock: Option<f64>,
    t: f32,
    off: Vec<f32>,
    /// One unit tremor, `sin(d·FREQ)·exp(−d/DECAY)`, every `KSTEP` of paper.
    kernel: Vec<f32>,
    working: Vec<usize>,
}

impl Drum {
    fn new(seed: u64) -> Self {
        Drum {
            seed: (seed % 31) as f32,
            rng: Rng::new(seed ^ 0xD3D3),
            rows: Vec::new(),
            live: 0,
            last_live: f32::NEG_INFINITY,
            calls: 0,
            clock: None,
            t: 0.,
            off: Vec::new(),
            kernel: (0..=(RING / KSTEP) as usize + 1)
                .map(|i| {
                    let d = i as f32 * KSTEP;
                    (d * FREQ).sin() * (-d / DECAY).exp()
                })
                .collect(),
            working: Vec::new(),
        }
    }

    fn pen_x(w: f32) -> f32 {
        (w - PEN_INSET).max(0.)
    }

    fn base_y(i: usize) -> f32 {
        TOP + i as f32 * ROW + ROW / 2.
    }
}

/// How hard each kind of call shakes the pen (pt).
fn amp(k: Kind) -> f32 {
    match k {
        Kind::Think => 3.,
        Kind::Read => 4.,
        Kind::Search => 5.,
        Kind::Web => 6.,
        Kind::Edit => 10.,
        Kind::Bash => 12.,
    }
}

/// `hh:mm:ss` from seconds since midnight.
fn clock_s(secs: f64) -> String {
    let s = secs.rem_euclid(86_400.).floor() as u32;
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

impl Ambient for Drum {
    fn update(&mut self, cx: &Ctx) {
        let (w, h, t) = (cx.size.w, cx.size.h, cx.t);
        self.t = t;
        // At least two rows, unless the page is too short to show any.
        let n = if h < TOP + ROW {
            0
        } else {
            (((h - 48.) / ROW).floor().max(2.) as usize).min(MAX_ROWS)
        };
        self.live = (n as f32 * 0.6) as usize;
        self.rows.resize_with(n, Row::default);
        let pen_x = Self::pen_x(w);
        let u0 = t * SPEED;
        let pen_u = pen_x + u0;

        if let Some(m) = cx.signals.minute {
            let off = m as f64 * 60. - t as f64;
            if self.clock.is_none_or(|c| (c - off).abs() > RESYNC) {
                self.clock = Some(off);
            }
        }

        // Rows other than yours follow the sessions, cycling if there are fewer.
        let sessions = &cx.signals.sessions;
        self.working.clear();
        let k = ease(3., cx.dt);
        let mut j = 0;
        for (i, row) in self.rows.iter_mut().enumerate() {
            if i == self.live {
                row.working = true;
                row.lit = 1.;
                if row.name != "you" {
                    row.name.clear();
                    row.name.push_str("you");
                }
            } else {
                if let Some(s) = sessions.get(j % sessions.len().max(1)) {
                    if row.name != s.name {
                        row.name.clear();
                        row.name.push_str(&s.name);
                    }
                    row.working = s.working;
                } else {
                    row.working = false;
                }
                j += 1;
                if row.working {
                    self.working.push(i);
                }
            }
            row.lit += ((row.working as u8 as f32) - row.lit) * k;
        }

        // Each tool call shakes a working session's row.
        for &kind in cx.signals.calls.iter().take(40) {
            self.calls = self.calls.wrapping_add(1);
            if self.working.is_empty() {
                continue;
            }
            let c = self.calls as i32;
            let pick = (hash(c, 3, self.seed as i32) * self.working.len() as f32) as usize;
            let row = &mut self.rows[self.working[pick.min(self.working.len() - 1)]];
            let a = amp(kind) * (0.8 + 0.4 * hash(c, 5, 11));
            match row.ev.back_mut() {
                Some(e) if pen_u - e.u < CALL_GAP => e.a = (e.a + a * 0.5).min(TRAVEL * 1.5),
                _ => row.ev.push_back(Tremor {
                    u: pen_u,
                    a,
                    mark: false,
                }),
            }
        }

        // Your keystrokes: at most one tremor every few points of paper.
        let Some(live) = self.rows.get_mut(self.live) else {
            return;
        };
        if cx.input.keys > 0 && pen_u - self.last_live > LIVE_GAP {
            let a = 5. + cx.input.keys.min(4) as f32 * 2. + self.rng.f() * 4.;
            live.ev.push_back(Tremor {
                u: pen_u,
                a,
                mark: false,
            });
            self.last_live = pen_u;
        }
        if cx.input.submit {
            let last_mark = live.ev.iter().rev().find(|e| e.mark).map(|e| e.u);
            let mark = last_mark.is_none_or(|m| pen_u - m > MARK_GAP);
            live.ev.push_back(Tremor {
                u: pen_u,
                a: SUBMIT_A,
                mark,
            });
        }

        // Prune, then build each trace. Samples sit at fixed places on the
        // paper, so they glide with it and the pen noise is worked out once,
        // as paper comes out from under the pen.
        let k_lo = (u0 / SAMPLE).floor() as i64 - 1;
        let k_hi = (pen_u / SAMPLE).ceil() as i64;
        let samples = (k_hi - k_lo + 1).max(0) as usize;
        self.off.resize(samples, 0.);
        for (i, row) in self.rows.iter_mut().enumerate() {
            while row.ev.front().is_some_and(|e| e.u <= u0 - PRUNE) {
                row.ev.pop_front();
            }
            while row.ev.len() > MAX_TREMORS {
                row.ev.pop_front();
            }
            let fi = i as f32 * 7.3;
            if row.noise.is_empty() || row.k0 > k_lo || row.k0 + (row.noise.len() as i64) < k_lo {
                row.noise.clear();
                row.k0 = k_lo;
            }
            while row.k0 < k_lo {
                row.noise.pop_front();
                row.k0 += 1;
            }
            while row.k0 + (row.noise.len() as i64) <= k_hi {
                let u = (row.k0 + row.noise.len() as i64) as f32 * SAMPLE;
                row.noise.push_back(noise3(u * 0.08, fi, self.seed) * 0.9);
            }
            self.off.iter_mut().for_each(|v| *v = 0.);
            for e in &row.ev {
                let first = ((e.u / SAMPLE).ceil() as i64 - k_lo).max(0) as usize;
                let last = (((e.u + RING) / SAMPLE).ceil() as i64 - k_lo).clamp(0, samples as i64)
                    as usize;
                for s in first..last {
                    let dd = (k_lo + s as i64) as f32 * SAMPLE - e.u;
                    if (0. ..RING).contains(&dd) {
                        let f = dd / KSTEP;
                        let (i, k) = (f as usize, f.fract());
                        let ring = self.kernel[i] + (self.kernel[i + 1] - self.kernel[i]) * k;
                        self.off[s] += e.a * ring;
                    }
                }
            }
            let by = Self::base_y(i);
            let n = self.off.len().min(row.noise.len());
            row.trace.clear();
            row.trace.extend(
                self.off
                    .iter()
                    .zip(&row.noise)
                    .enumerate()
                    // Where no tremor moves the pen it only drifts a little,
                    // so a quarter of the points draw it as well and cost a
                    // quarter as much to stroke.
                    .filter(|&(s, (&off, _))| {
                        s % QUIET_EVERY == 0 || s + 1 == n || off.abs() > 0.05
                    })
                    .map(|(s, (&off, &jitter))| {
                        let x = (k_lo + s as i64) as f32 * SAMPLE - u0;
                        [x, by + jitter + TRAVEL * (off / TRAVEL).tanh()]
                    }),
            );
        }
    }

    fn draw(&self, cx: &DrawCtx, out: &mut Scene) {
        let (w, h, p) = (cx.size.w, cx.size.h, cx.pal);
        let pen_x = Self::pen_x(w);
        let u0 = self.t * SPEED;

        // Time ticks, gliding with the paper.
        if let Some(clock) = self.clock {
            let now = clock + self.t as f64;
            let speed = SPEED as f64;
            // Wall time at the left edge, and the first tick after it.
            let left = now - pen_x as f64 / speed;
            let mut k = (left / TICK).ceil() as i64;
            let (mut minor, mut major) = (Vec::new(), Vec::new());
            loop {
                let x = (pen_x as f64 - (now - k as f64 * TICK) * speed) as f32;
                if x >= pen_x {
                    break;
                }
                let xs = x.round() + 0.5;
                if k % 2 == 0 {
                    major.extend([[xs, 22.], [xs, h]]);
                    out.text(
                        [x, 15.],
                        clock_s(k as f64 * TICK),
                        Font::Mono,
                        10.,
                        p.soft,
                        Align::Center,
                    );
                } else {
                    minor.extend([[xs, 22.], [xs, h]]);
                }
                k += 1;
            }
            out.segments(minor, 1., p.rule);
            out.segments(major, 1., p.rule2);
        }

        out.clip(0., 0., pen_x, h);
        for (i, row) in self.rows.iter().enumerate() {
            let you = i == self.live;
            let (color, width) = if you {
                (p.accent, 1.5)
            } else {
                (p.pencil.alpha(0.4 + 0.6 * row.lit), 0.9)
            };
            out.line(row.trace.clone(), width, color);
        }
        out.unclip();

        let label_w = (w - pen_x - 14.).max(0.);
        for (i, row) in self.rows.iter().enumerate() {
            let you = i == self.live;
            let by = Self::base_y(i);
            let (pen, label) = if you {
                (p.accent, p.accent)
            } else {
                let k = 0.45 + 0.55 * row.lit;
                (p.pencil.alpha(k), p.soft.alpha(k))
            };
            out.fill(
                vec![[pen_x, by], [pen_x + 7., by - 4.], [pen_x + 7., by + 4.]],
                Paint::Solid(pen),
            );
            out.text(
                [pen_x + 12., by + 3.],
                fit(&row.name, Font::Mono, 10., label_w),
                Font::Mono,
                10.,
                label,
                Align::Left,
            );
            for e in row.ev.iter().filter(|e| e.mark) {
                let x = e.u - u0;
                if x > 0. && x < pen_x {
                    // Marks grow in as they leave the pen.
                    let k = out_cubic(clamp((pen_x - x) / 12., 0., 1.));
                    let c = p.accent.alpha(k);
                    out.circle([x, by - 26.], 3. * k, None, Some((1., c)));
                    out.text(
                        [x + 6., by - 23.],
                        "session",
                        Font::Mono,
                        10.,
                        c,
                        Align::Left,
                    );
                }
            }
        }
        out.segments(vec![[pen_x + 0.5, 22.], [pen_x + 0.5, h]], 1., p.rule2);
    }
}
