//! Stipple: relief shading made only of single dots, as on engraved survey
//! sheets. A drifting height field is lit from the northwest with Lambert
//! shading; each grid cell gets one jittered dot when its darkness beats a
//! fixed per-cell threshold, so the pattern never shimmers. Low ground
//! becomes a sparse stippled lake in the working colour.

use crate::math::{clamp, hash, noise3};
use crate::scene::Pt;
use crate::{Ambient, Ctx, DrawCtx, Scene, Spec};

pub const SPEC: Spec = Spec {
    id: "stipple",
    name: "Stipple",
    from: "Stippled relief from engraved survey sheets and naturalist plates, lit from the northwest with Lambert shading.",
    shows: "Shading made only of single dots. The terrain drifts faster while your sessions are busy, low ground fills as a stippled lake in the working colour, and starting a session lifts a rise under the composer.",
    make: |seed| Box::new(Stipple::new(seed)),
};

/// Grid pitch (pt): one possible dot per cell.
const ST: f32 = 6.;
/// Dot size (pt).
const DOT: f32 = 1.8;
/// Terrain drift when idle (noise units per second).
const DRIFT: f32 = 0.025;
/// Extra drift at full activity, as a share of `DRIFT`.
const DRIFT_BUSY: f32 = 1.6;
/// How fast activity fades (per second), and what one tool call adds.
const ACT_DECAY: f32 = 0.5;
const ACT_PER_CALL: f32 = 0.035;
/// The submit hill: what one start adds, its cap, and its decay per second.
const HILL_ADD: f32 = 0.8;
const HILL_MAX: f32 = 1.2;
const HILL_DECAY: f32 = 0.35;
/// Hill and pointer bump widths (pt², as 2σ²).
const HILL_W: f32 = 45_000.;
const BUMP_W: f32 = 12_000.;
const BUMP_H: f32 = 0.6;
/// Ground below this height is lake.
const LAKE: f32 = -0.42;
/// Share of lake cells that get a dot.
const LAKE_DENSITY: f32 = 0.2;
/// Light from the northwest (unit vector).
const L: [f32; 3] = [-0.52, -0.52, 0.677];
/// Slope exaggeration for shading.
const RELIEF: f32 = 70.;
/// Dots this close to their threshold draw half-strength, so a dot that
/// the drifting terrain is about to add or remove fades rather than pops.
const EDGE: f32 = 0.04;
/// Cells computed at most (2560 by 1600 needs ~116k); beyond this the pitch grows.
const MAX_CELLS: usize = 150_000;
/// The noise terrain is sampled as snapshots this far apart in drift, built
/// a few rows per frame and blended, since it moves far less than a dot
/// between frames. Larger than the most one frame can drift.
const SNAP_DZ: f32 = 0.008;

struct Stipple {
    seed: i32,
    z: f32,
    act: f32,
    hill: f32,
    /// Grid as last laid out: columns, rows, pitch.
    grid: (usize, usize, f32),
    /// Noise snapshots: blending `a`→`b` while `c` is built.
    snaps: [Vec<f32>; 3],
    snap_z: [f32; 3],
    order: [usize; 3],
    built: usize,
    /// Each cell's fixed threshold and jittered dot position.
    cells: Vec<(f32, Pt)>,
    /// Hill and pointer factors per column and per row.
    fx: Vec<[f32; 2]>,
    fy: Vec<[f32; 2]>,
    h: Vec<f32>,
    /// Full and half-strength dots, then lake dots the same way.
    dots: [Vec<Pt>; 2],
    lake: [Vec<Pt>; 2],
}

impl Stipple {
    fn new(seed: u64) -> Self {
        Stipple {
            seed: (seed % 100_000) as i32,
            z: 0.,
            act: 0.,
            hill: 0.,
            grid: (0, 0, ST),
            snaps: [Vec::new(), Vec::new(), Vec::new()],
            snap_z: [0.; 3],
            order: [0, 1, 2],
            built: 0,
            cells: Vec::new(),
            fx: Vec::new(),
            fy: Vec::new(),
            h: Vec::new(),
            dots: [Vec::new(), Vec::new()],
            lake: [Vec::new(), Vec::new()],
        }
    }
}

impl Stipple {
    /// Noise heights for grid rows `r0..r1` at drift `z` into `buf`.
    fn fill(&self, buf: &mut [f32], z: f32, r0: usize, r1: usize) {
        let (cols, _, st) = self.grid;
        let (z1, z2) = (z + (self.seed % 40) as f32, z + 4.);
        for gy in r0..r1 {
            let y = gy as f32 * st;
            for (gx, cell) in buf[gy * cols..(gy + 1) * cols].iter_mut().enumerate() {
                let x = gx as f32 * st;
                *cell = noise3(x * 0.004, y * 0.004, z1) + 0.3 * noise3(x * 0.011, y * 0.011, z2);
            }
        }
    }

    /// Build `rows` more of the snapshot under construction.
    fn build(&mut self, upto: usize) {
        let ic = self.order[2];
        let mut buf = std::mem::take(&mut self.snaps[ic]);
        self.fill(&mut buf, self.snap_z[ic], self.built, upto);
        self.snaps[ic] = buf;
        self.built = self.built.max(upto);
    }

    /// Keep the snapshots ahead of the drift; returns the blend from `a` to `b`.
    fn advance(&mut self, cols: usize, rows: usize, st: f32) -> f32 {
        if self.grid != (cols, rows, st) || self.snaps[0].len() != cols * rows {
            self.grid = (cols, rows, st);
            self.order = [0, 1, 2];
            for i in 0..3 {
                self.snaps[i].resize(cols * rows, 0.);
                self.snap_z[i] = self.z + i as f32 * SNAP_DZ;
            }
            for i in 0..2 {
                let mut buf = std::mem::take(&mut self.snaps[i]);
                self.fill(&mut buf, self.snap_z[i], 0, rows);
                self.snaps[i] = buf;
            }
            self.built = 0;
            let (s0, s1) = (self.seed, self.seed + 1);
            self.cells.clear();
            self.cells.extend((0..cols * rows).map(|i| {
                let (ix, iy) = ((i % cols) as i32, (i / cols) as i32);
                let x = ix as f32 * st + (hash(ix, iy, s0) - 0.5) * st;
                let y = iy as f32 * st + (hash(iy, ix, s1) - 0.5) * st;
                (hash(ix, iy, 77), [x, y])
            }));
        }
        while self.z >= self.snap_z[self.order[1]] {
            self.build(rows);
            let [a, b, c] = self.order;
            self.order = [b, c, a];
            self.snap_z[a] = self.snap_z[c] + SNAP_DZ;
            self.built = 0;
        }
        let k = clamp((self.z - self.snap_z[self.order[0]]) / SNAP_DZ, 0., 1.);
        // Stay a little ahead so the next snapshot is done before it's needed.
        self.build((((k + 0.15) * rows as f32).ceil() as usize).min(rows));
        k
    }
}

impl Ambient for Stipple {
    fn update(&mut self, cx: &Ctx) {
        let dt = cx.dt;
        let calls = cx.signals.calls.len().min(30) as f32;
        self.act = clamp(
            self.act * (-dt * ACT_DECAY).exp() + calls * ACT_PER_CALL,
            0.,
            1.,
        );
        self.z += dt * DRIFT * (1. + DRIFT_BUSY * self.act);
        let lift = if cx.input.submit || cx.signals.started > 0 {
            HILL_ADD
        } else {
            0.
        };
        self.hill = (self.hill * (-dt * HILL_DECAY).exp() + lift).min(HILL_MAX);

        let (w, h) = (cx.size.w, cx.size.h);
        let mut st = ST;
        let (mut cols, mut rows);
        loop {
            cols = (w / st).ceil() as usize + 2;
            rows = (h / st).ceil() as usize + 2;
            if cols * rows <= MAX_CELLS {
                break;
            }
            st *= 1.25;
        }
        self.h.resize(cols * rows, 0.);
        let k = self.advance(cols, rows, st);
        let [ia, ib, _] = self.order;
        // Both bumps are Gaussians, so they separate into a column and a row factor.
        let ptr = cx.input.pointer.filter(|p| p.k > 0.002);
        let (hill, (hx, hy)) = (self.hill, (w / 2., h / 2.));
        let bump = |d: f32, wide: f32| {
            if d * d < wide * 9. {
                (-d * d / wide).exp()
            } else {
                0.
            }
        };
        self.fx.clear();
        self.fx.extend((0..cols).map(|gx| {
            let x = gx as f32 * st;
            [
                bump(x - hx, HILL_W),
                ptr.map_or(0., |p| bump(x - p.x, BUMP_W)),
            ]
        }));
        self.fy.clear();
        self.fy.extend((0..rows).map(|gy| {
            let y = gy as f32 * st;
            [
                hill * bump(y - hy, HILL_W),
                ptr.map_or(0., |p| BUMP_H * p.k * bump(y - p.y, BUMP_W)),
            ]
        }));
        for gy in 0..rows {
            let span = gy * cols..(gy + 1) * cols;
            let (a, b) = (&self.snaps[ia][span.clone()], &self.snaps[ib][span.clone()]);
            let [hy, py] = self.fy[gy];
            let row = &mut self.h[span];
            for (gx, cell) in row.iter_mut().enumerate() {
                let [hx, px] = self.fx[gx];
                *cell = a[gx] + (b[gx] - a[gx]) * k + hx * hy + px * py;
            }
        }

        for v in self.dots.iter_mut().chain(self.lake.iter_mut()) {
            v.clear();
        }
        let hm = &self.h;
        let grad = RELIEF / (2. * st);
        for gy in 1..rows.saturating_sub(1) {
            for gx in 1..cols - 1 {
                let i = gy * cols + gx;
                let (v, (th, at)) = (hm[i], self.cells[i]);
                if v < LAKE {
                    if th < LAKE_DENSITY {
                        self.lake[(LAKE - v < EDGE) as usize].push(at);
                    }
                    continue;
                }
                let ax = (hm[i + 1] - hm[i - 1]) * grad;
                let ay = (hm[i + cols] - hm[i - cols]) * grad;
                let n = 1. / (ax * ax + ay * ay + 1.).sqrt();
                let lam = (-ax * L[0] - ay * L[1] + L[2]) * n;
                let dark = clamp((L[2] - lam) * 4.2 + 0.2, 0., 1.);
                if dark > th {
                    self.dots[(dark - th < EDGE) as usize].push(at);
                }
            }
        }
    }

    fn draw(&self, cx: &DrawCtx, out: &mut Scene) {
        let p = cx.pal;
        let [full, half] = &self.dots;
        out.dots(DOT, p.pencil, full.clone());
        out.dots(DOT, p.pencil.alpha(0.5), half.clone());
        let [full, half] = &self.lake;
        out.dots(DOT, p.accent, full.clone());
        out.dots(DOT, p.accent.alpha(0.5), half.clone());
    }
}
