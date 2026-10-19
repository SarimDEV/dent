//! How a large picture moves when zoomed: the maths, apart from drawing.
//!
//! What makes zooming feel right, as maps and photo viewers do it:
//! - Zoom lives on a log scale. Every motion moves `ln(zoom)`, so 1× to 2×
//!   takes as long and feels as fast as 4× to 8×.
//! - The point under the pointer (or between the fingers) stays under it
//!   on every frame, eased or not: the pan is worked out from the zoom
//!   about that point, never eased on its own.
//! - Fingers are followed exactly: a pinch or a trackpad's ⌘-scroll moves
//!   the picture by what they did, with nothing smoothed in between. Past
//!   its limits it gives, more the further it goes (the rubber band), and
//!   settles back once they let go.
//! - Steps (a key, a mouse wheel's notch, a double-click) move on a
//!   critically damped spring: they gather speed, then settle without
//!   overshooting, in about the same time however far they go. A step
//!   taken while one is on its way adds to where it's going, not to where
//!   it is, and keeps the speed it had, so quick presses run into one
//!   smooth move with no jolt.
//! - The spring is stepped by its exact solution, so it moves the same at
//!   60 fps and at 120.
//! - A drag let go while moving carries on and slows to a stop, as a
//!   scroll view does, and stops at the picture's edge.

/// The zoom spring's stiffness, as its angular frequency: 90% of the way
/// in 140 ms, and within 1% by 300 ms even going 1× to 8×, about as long
/// as a map's wheel zoom takes.
const SPRING: f32 = 28.;
/// How quickly an edge, or a zoom past its limits, settles back.
const SETTLE_S: f32 = 0.075;
/// After the last move of the fingers, how long before what went past
/// the limits settles back: a trackpad's own glide keeps it held.
const LET_GO_S: f32 = 0.08;
/// The rubber band's stiffness, as UIKit's scroll views use.
const RUBBER: f32 = 0.55;
/// How far past its limits a zoom can stretch, in `ln(zoom)`.
const ZOOM_GIVE: f32 = 0.35;
/// How much a glide keeps of its speed each millisecond (a scroll view's
/// normal deceleration).
const GLIDE_KEEP: f32 = 0.998;
/// Slower than this, in points a millisecond, a glide stops.
const GLIDE_STOP: f32 = 0.015;
/// The fastest a glide starts, in points a millisecond.
const GLIDE_MOST: f32 = 6.;

/// Where a picture fits in the window, in points: its box, the window's
/// size, and the most it zooms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub vw: f32,
    pub vh: f32,
    pub most: f32,
}

impl Frame {
    pub fn middle(&self) -> (f32, f32) {
        (self.x + self.w / 2., self.y + self.h / 2.)
    }

    /// The pans that show no backdrop past an edge the picture could
    /// cover, a side at a time; along a side it doesn't fill, only the
    /// middle.
    fn bounds(&self, zoom: f32) -> [(f32, f32); 2] {
        let (mx, my) = self.middle();
        let axis = |size: f32, mid: f32, view: f32| {
            if size <= view {
                (0., 0.)
            } else {
                (view - size / 2. - mid, size / 2. - mid)
            }
        };
        [
            axis(self.w * zoom, mx, self.vw),
            axis(self.h * zoom, my, self.vh),
        ]
    }

    /// `pan` brought within bounds.
    pub fn clamped(&self, zoom: f32, pan: (f32, f32)) -> (f32, f32) {
        let [bx, by] = self.bounds(zoom);
        (pan.0.clamp(bx.0, bx.1), pan.1.clamp(by.0, by.1))
    }
}

/// How much of a move past a limit shows, already `over` past it, with
/// `give` the distance it can stretch.
fn resistance(over: f32, give: f32) -> f32 {
    let k = RUBBER * over / give + 1.;
    RUBBER / (k * k)
}

/// An eased zoom on its way.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Target {
    /// Where it's going, as `ln(zoom)`.
    to: f32,
    /// The point in the window that stays put.
    about: (f32, f32),
}

#[derive(Clone, Debug, PartialEq)]
pub struct PanZoom {
    /// 1 fits; past the limits only while stretched.
    pub zoom: f32,
    /// Its middle's offset from the fitted middle, in points.
    pub pan: (f32, f32),
    target: Option<Target>,
    /// How fast `ln(zoom)` is moving on the spring, a second.
    speed: f32,
    /// A drag's glide, in points a millisecond.
    glide: Option<(f32, f32)>,
    /// A drag holds it.
    held: bool,
    /// Seconds since fingers last moved it.
    idle: f32,
    /// Where the fingers last zoomed about, to settle back about.
    about: (f32, f32),
}

impl Default for PanZoom {
    fn default() -> Self {
        Self {
            zoom: 1.,
            pan: (0., 0.),
            target: None,
            speed: 0.,
            glide: None,
            held: false,
            idle: LET_GO_S,
            about: (0., 0.),
        }
    }
}

impl PanZoom {
    pub fn zoomed_in(&self) -> bool {
        self.zoom > 1.001
    }

    /// Where an eased zoom is going, or where it is.
    pub fn heading(&self) -> f32 {
        self.target.map_or(self.zoom, |t| t.to.exp())
    }

    /// Set the zoom to `to`, keeping the point at `about` where it is.
    fn zoom_about(&mut self, f: &Frame, to: f32, about: (f32, f32)) {
        let (mx, my) = f.middle();
        let (cx, cy) = (mx + self.pan.0, my + self.pan.1);
        let r = to / self.zoom.max(1e-4);
        self.pan = (
            about.0 - (about.0 - cx) * r - mx,
            about.1 - (about.1 - cy) * r - my,
        );
        self.zoom = to;
    }

    /// Fingers moved it: whatever was easing or gliding stops where it is.
    fn touched(&mut self) {
        self.target = None;
        self.speed = 0.;
        self.glide = None;
        self.idle = 0.;
    }

    /// Zoom by `by` in `ln(zoom)` about `about`, as fingers do: at once,
    /// giving past the limits.
    pub fn zoom_by(&mut self, f: &Frame, by: f32, about: (f32, f32)) {
        self.touched();
        self.about = about;
        let (lz, hi) = (self.zoom.ln(), f.most.ln());
        let over = (lz - hi).max(-lz).max(0.);
        let outward = (by > 0. && lz + by > hi) || (by < 0. && lz + by < 0.);
        let by = if outward {
            by * resistance(over, ZOOM_GIVE)
        } else {
            by
        };
        self.zoom_about(f, (lz + by).exp(), about);
    }

    /// Move it by `d`, as fingers do: at once, giving past its edges.
    pub fn pan_by(&mut self, f: &Frame, d: (f32, f32)) {
        self.touched();
        let [bx, by] = f.bounds(self.zoom);
        let axis = |p: f32, d: f32, (lo, hi): (f32, f32), give: f32| {
            let over = (lo - p).max(p - hi).max(0.);
            let outward = (d > 0. && p + d > hi) || (d < 0. && p + d < lo);
            p + if outward {
                d * resistance(over, give)
            } else {
                d
            }
        };
        self.pan = (
            axis(self.pan.0, d.0, bx, f.vw),
            axis(self.pan.1, d.1, by, f.vh),
        );
    }

    /// Ease to `ln(zoom)` + `by` from where it's going (a key, a wheel's
    /// notch), about `about`.
    pub fn step(&mut self, f: &Frame, by: f32, about: (f32, f32)) {
        let from = self.target.map_or(self.zoom.ln(), |t| t.to);
        self.ease_to(f, (from + by).exp(), about);
    }

    /// Ease to `zoom` about `about`.
    pub fn ease_to(&mut self, f: &Frame, zoom: f32, about: (f32, f32)) {
        self.glide = None;
        self.target = Some(Target {
            to: zoom.clamp(1., f.most).ln(),
            about,
        });
    }

    /// A drag took hold of it.
    pub fn hold(&mut self) {
        self.touched();
        self.held = true;
    }

    /// The drag let go, moving at `speed` points a millisecond.
    pub fn let_go(&mut self, speed: (f32, f32)) {
        self.held = false;
        let s = speed.0.hypot(speed.1);
        if s > GLIDE_STOP {
            let k = GLIDE_MOST.min(s) / s;
            self.glide = Some((speed.0 * k, speed.1 * k));
        }
    }

    /// The fingers left the trackpad: what went past its limits settles
    /// back now.
    pub fn lifted(&mut self) {
        self.idle = self.idle.max(LET_GO_S);
    }

    /// Back to fitting, still.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Move on by `dt` seconds. True while it's still moving, so another
    /// frame is wanted.
    pub fn tick(&mut self, f: &Frame, dt: f32, still: bool) -> bool {
        let dt = dt.clamp(0., 0.05);
        let share = |s: f32| if still { 1. } else { 1. - (-dt / s).exp() };
        let mut moving = false;
        if let Some(t) = self.target {
            // How far from where it's going, and how fast, after `dt` on a
            // critically damped spring: x(t) = (x + (v + ωx)t)e^(−ωt).
            let (x, v) = (self.zoom.ln() - t.to, self.speed);
            let (x, v) = if still {
                (0., 0.)
            } else {
                let e = (-SPRING * dt).exp();
                let c = v + SPRING * x;
                ((x + c * dt) * e, (v - SPRING * c * dt) * e)
            };
            let done = x.abs() < 1e-3 && v.abs() < 0.05;
            let next = if done { t.to } else { t.to + x };
            self.speed = if done { 0. } else { v };
            if done {
                self.target = None;
            }
            self.zoom_about(f, next.exp(), t.about);
            moving = true;
        }
        if let Some((vx, vy)) = self.glide {
            let ms = dt * 1000.;
            let (px, py) = (self.pan.0 + vx * ms, self.pan.1 + vy * ms);
            let (cx, cy) = f.clamped(self.zoom, (px, py));
            // At an edge it stops there.
            let keep = GLIDE_KEEP.powf(ms);
            let v = (
                if cx == px { vx * keep } else { 0. },
                if cy == py { vy * keep } else { 0. },
            );
            self.pan = (cx, cy);
            // With motion reduced, a drag stops where it's let go.
            self.glide = (!still && v.0.hypot(v.1) > GLIDE_STOP).then_some(v);
            moving = true;
        }
        let resting = !self.held && self.idle >= LET_GO_S;
        if resting && self.target.is_none() {
            let within = self.zoom.clamp(1., f.most);
            if (within.ln() - self.zoom.ln()).abs() > 1e-4 {
                self.target = Some(Target {
                    to: within.ln(),
                    about: self.about,
                });
                moving = true;
            }
        }
        // Past an edge, it settles back in, unless fingers are on it.
        if (resting || self.target.is_some()) && self.glide.is_none() {
            let c = f.clamped(self.zoom, self.pan);
            let d = (c.0 - self.pan.0, c.1 - self.pan.1);
            if d.0.hypot(d.1) > 0.25 {
                let s = share(SETTLE_S);
                self.pan = (self.pan.0 + d.0 * s, self.pan.1 + d.1 * s);
                moving = true;
            } else {
                self.pan = c;
            }
        }
        // Stretched and waiting for the fingers to go: keep the frames
        // coming, so it settles back on time.
        if !resting && !self.held {
            let c = f.clamped(self.zoom, self.pan);
            moving |= c != self.pan || self.zoom < 1. || self.zoom > f.most;
        }
        self.idle += dt;
        moving
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1000 × 500 window, the picture fitted to 800 × 400, up to 16×.
    fn frame() -> Frame {
        Frame {
            x: 100.,
            y: 50.,
            w: 800.,
            h: 400.,
            vw: 1000.,
            vh: 500.,
            most: 16.,
        }
    }

    /// Run `pz` at `fps` for `secs`, giving the zoom after each frame.
    fn run(pz: &mut PanZoom, fps: f32, secs: f32) -> Vec<f32> {
        let f = frame();
        (0..(fps * secs) as usize)
            .map(|_| {
                pz.tick(&f, 1. / fps, false);
                pz.zoom
            })
            .collect()
    }

    /// Where the picture's point under `p` is, as a fraction of its box.
    fn under(pz: &PanZoom, p: (f32, f32)) -> (f32, f32) {
        let f = frame();
        let (mx, my) = f.middle();
        let (w, h) = (f.w * pz.zoom, f.h * pz.zoom);
        let (x, y) = (mx + pz.pan.0 - w / 2., my + pz.pan.1 - h / 2.);
        ((p.0 - x) / w, (p.1 - y) / h)
    }

    #[test]
    fn an_eased_zoom_keeps_its_point_still_every_frame() {
        // Already larger than the window both ways, so nothing holds it
        // centred meanwhile.
        let f = frame();
        let mut pz = PanZoom {
            zoom: 2.,
            ..PanZoom::default()
        };
        let p = (420., 230.);
        let at = under(&pz, p);
        pz.ease_to(&f, 6., p);
        for _ in 0..30 {
            pz.tick(&f, 1. / 60., false);
            let now = under(&pz, p);
            assert!((now.0 - at.0).abs() < 1e-4 && (now.1 - at.1).abs() < 1e-4);
        }
        assert_eq!(pz.zoom, 6.);
    }

    #[test]
    fn it_eases_in_log_steps_fast_then_gently_the_same_at_any_frame_rate() {
        let f = frame();
        let mut a = PanZoom::default();
        a.ease_to(&f, 8., f.middle());
        let at60 = run(&mut a, 60., 0.1);
        let mut b = PanZoom::default();
        b.ease_to(&f, 8., f.middle());
        let at120 = run(&mut b, 120., 0.1);
        // The same place after 0.1 s, whatever the frame rate.
        let (x, y) = (at60.last().unwrap().ln(), at120.last().unwrap().ln());
        assert!((x - y).abs() < 0.01, "{x} {y}");
        // Every frame moves on, gathering speed and then easing off,
        // never past where it's going.
        run(&mut a, 60., 0.2);
        let mut all = at60.clone();
        all.extend(run(&mut a, 60., 0.25));
        let steps: Vec<f32> = all.windows(2).map(|w| w[1].ln() - w[0].ln()).collect();
        assert!(steps.iter().all(|s| *s >= 0.), "{steps:?}");
        assert!(steps[1] > steps[0]);
        assert!(all.iter().all(|z| *z <= 8.));
        // Within 1% by 300 ms, whether it goes 1.5× or 8×; there exactly
        // once its frames stop.
        for to in [1.5, 8.] {
            let mut c = PanZoom::default();
            c.ease_to(&f, to, f.middle());
            let z = *run(&mut c, 60., 0.3).last().unwrap();
            assert!((z / to - 1.).abs() < 0.01, "{to}: {z}");
            run(&mut c, 60., 0.3);
            assert_eq!(c.zoom, to);
            assert!(!c.tick(&f, 1. / 60., false));
        }
        // 1× to 2× takes as long as 4× to 8×: the same share of ln(zoom).
        let mut c = PanZoom::default();
        c.ease_to(&f, 2., f.middle());
        let c1 = run(&mut c, 60., 0.05);
        let mut d = PanZoom {
            zoom: 4.,
            ..PanZoom::default()
        };
        d.ease_to(&f, 8., f.middle());
        let d1 = run(&mut d, 60., 0.05);
        assert!((c1.last().unwrap().ln() - (d1.last().unwrap() / 4.).ln()).abs() < 1e-4);
    }

    #[test]
    fn quick_steps_add_to_where_its_going() {
        let f = frame();
        let mut pz = PanZoom::default();
        pz.step(&f, 1.5f32.ln(), f.middle());
        let before = run(&mut pz, 60., 0.05);
        pz.step(&f, 1.5f32.ln(), f.middle());
        assert!((pz.heading() - 2.25).abs() < 1e-4);
        // It keeps its speed through the second press: no jolt.
        let after = run(&mut pz, 60., 1. / 60.);
        let was = before[before.len() - 1].ln() - before[before.len() - 2].ln();
        let now = after[0].ln() - before[before.len() - 1].ln();
        assert!(now >= was * 0.9, "{was} {now}");
        // Never past its limits.
        for _ in 0..20 {
            pz.step(&f, 1.5f32.ln(), f.middle());
        }
        assert!((pz.heading() - 16.).abs() < 1e-3);
    }

    #[test]
    fn fingers_past_the_limits_stretch_and_settle_back() {
        let f = frame();
        let mut pz = PanZoom::default();
        // A pinch out from fitting gives, less the further it goes.
        pz.zoom_by(&f, -0.2, f.middle());
        let first = -pz.zoom.ln();
        assert!(first > 0. && first < 0.2 * RUBBER + 1e-6, "{first}");
        pz.zoom_by(&f, -0.2, f.middle());
        assert!(-pz.zoom.ln() - first < first);
        // Held, it stays stretched; let go, it settles back to fitting.
        run(&mut pz, 60., 0.05);
        assert!(pz.zoom < 1.);
        pz.lifted();
        run(&mut pz, 60., 0.6);
        assert_eq!((pz.zoom, pz.pan), (1., (0., 0.)));
    }

    #[test]
    fn a_drag_gives_past_an_edge_and_a_fling_glides_to_a_stop_at_one() {
        let f = frame();
        let mut pz = PanZoom {
            zoom: 2.,
            ..PanZoom::default()
        };
        // 1600 wide in 1000: 300 either way. A drag 400 past gives.
        pz.hold();
        pz.pan_by(&f, (700., 0.));
        assert!(pz.pan.0 > 300. && pz.pan.0 < 700., "{:?}", pz.pan);
        pz.let_go((0., 0.));
        run(&mut pz, 60., 0.6);
        assert_eq!(pz.pan.0, 300.);
        // A fling left glides, slowing, and stops at the far edge.
        pz.hold();
        pz.let_go((-3., 0.));
        let mut last = pz.pan.0;
        let mut speeds = Vec::new();
        for _ in 0..240 {
            pz.tick(&f, 1. / 60., false);
            speeds.push(last - pz.pan.0);
            last = pz.pan.0;
        }
        assert_eq!(pz.pan.0, -300.);
        assert!(speeds[1] <= speeds[0]);
        // A gentle fling stops on its own, short of the edge.
        let mut g = PanZoom {
            zoom: 2.,
            ..PanZoom::default()
        };
        g.let_go((0.2, 0.));
        run(&mut g, 60., 3.);
        assert!(g.pan.0 > 50. && g.pan.0 < 300., "{:?}", g.pan);
        assert!(!g.tick(&f, 1. / 60., false));
    }
}
