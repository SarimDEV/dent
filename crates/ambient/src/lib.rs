//! Ambient backgrounds for the new-session screen.
//!
//! A background is a small simulation: [`Ambient::update`] advances it from
//! the time, normalised input and live [`Signals`]; [`Ambient::draw`] paints
//! it into a [`Scene`] with a [`Palette`]. Updates never see the theme, so
//! drawing in another theme is free. A [`Runner`] owns one background and
//! does everything common: input caps, pointer smoothing, time-step limits,
//! filling missing data from a synthetic source, and containing panics.
//!
//! Adding a background: write `src/bg/<id>.rs` exposing a `SPEC`, and list
//! it in [`ALL`]. See docs/ambient.md.

pub mod geom;
pub mod math;
pub mod scene;
pub mod signals;

mod bg;

pub use scene::{Align, Font, Paint, Palette, Prim, Pt, Rgba, Scene};
pub use signals::{File, Kind, LogLine, Machine, Session, Signals, Touch, Usage};

use std::panic::{AssertUnwindSafe, catch_unwind};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    pub w: f32,
    pub h: f32,
}

/// The pointer, smoothed. `k` is how much it counts (0..1): it eases in when
/// the pointer arrives over open background and out when it leaves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pointer {
    pub x: f32,
    pub y: f32,
    pub k: f32,
}

/// Input for one frame, already capped and debounced.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Input {
    /// Keys typed in the composer since the last frame (at most [`MAX_KEYS`]).
    pub keys: u32,
    /// You started a session this frame.
    pub submit: bool,
    pub pointer: Option<Pointer>,
}

pub struct Ctx<'a> {
    /// Seconds since the background started.
    pub t: f32,
    pub dt: f32,
    pub size: Size,
    pub input: &'a Input,
    pub signals: &'a Signals,
}

pub struct DrawCtx<'a> {
    pub t: f32,
    pub size: Size,
    pub pal: &'a Palette,
    pub pointer: Option<Pointer>,
}

pub trait Ambient {
    fn update(&mut self, cx: &Ctx);
    fn draw(&self, cx: &DrawCtx, out: &mut Scene);
}

/// A background as listed in settings.
pub struct Spec {
    pub id: &'static str,
    pub name: &'static str,
    /// The real-world convention it borrows.
    pub from: &'static str,
    /// What it shows.
    pub shows: &'static str,
    pub make: fn(seed: u64) -> Box<dyn Ambient>,
}

/// Every background, in the order settings lists them.
pub static ALL: &[Spec] = &[
    bg::prism::SPEC,
    bg::attention::SPEC,
    bg::spectro::SPEC,
    bg::graph::SPEC,
    bg::ruled::SPEC,
    bg::stipple::SPEC,
    bg::drum::SPEC,
    bg::isobar::SPEC,
    bg::stars::SPEC,
    bg::survey::SPEC,
    bg::tide::SPEC,
    bg::tally::SPEC,
    bg::bearing::SPEC,
];

pub fn find(id: &str) -> Option<&'static Spec> {
    ALL.iter().find(|s| s.id == id)
}

/// Keys counted per frame, however fast you type.
pub const MAX_KEYS: u32 = 6;
/// Sessions you start count at most this often (seconds).
pub const SUBMIT_GAP: f32 = 0.6;
/// The longest step simulated at once (seconds), so waking from sleep or a
/// stalled frame never makes anything lurch.
pub const MAX_DT: f32 = 0.1;

pub struct Runner {
    spec: &'static Spec,
    bg: Box<dyn Ambient>,
    synth: signals::Synth,
    t: f32,
    keys: u32,
    submit: bool,
    last_submit: f32,
    raw_pointer: Option<Pt>,
    smooth: Option<Pt>,
    weight: f32,
    size: Size,
    scene: Scene,
    failed: bool,
}

impl Runner {
    pub fn new(spec: &'static Spec, seed: u64) -> Runner {
        Runner {
            spec,
            bg: (spec.make)(seed),
            synth: signals::Synth::new(seed),
            t: 0.,
            keys: 0,
            submit: false,
            last_submit: f32::NEG_INFINITY,
            raw_pointer: None,
            smooth: None,
            weight: 0.,
            size: Size::default(),
            scene: Scene::default(),
            failed: false,
        }
    }

    pub fn spec(&self) -> &'static Spec {
        self.spec
    }

    /// A background panicked and has stopped drawing.
    pub fn failed(&self) -> bool {
        self.failed
    }

    /// A key was typed in the composer.
    pub fn key(&mut self) {
        self.keys = self.keys.saturating_add(1);
    }

    /// You started a session.
    pub fn submit(&mut self) {
        self.submit = true;
    }

    /// Where the pointer is over open background, or `None` when it's away
    /// or over the UI.
    pub fn pointer(&mut self, p: Option<Pt>) {
        self.raw_pointer = p.filter(|p| p[0].is_finite() && p[1].is_finite());
    }

    /// Advance by `dt` seconds. `live` is what the client knows; anything it
    /// leaves empty comes from the synthetic source.
    pub fn tick(&mut self, dt: f32, size: Size, live: Signals) {
        if self.failed {
            return;
        }
        let dt = if dt.is_finite() {
            dt.clamp(0., MAX_DT)
        } else {
            0.
        };
        self.t += dt;
        self.size = Size {
            w: size.w.max(1.),
            h: size.h.max(1.),
        };
        if let Some(p) = self.raw_pointer {
            let s = self.smooth.get_or_insert(p);
            let k = math::ease(14., dt);
            s[0] += (p[0] - s[0]) * k;
            s[1] += (p[1] - s[1]) * k;
        }
        let target = if self.raw_pointer.is_some() { 1. } else { 0. };
        self.weight += (target - self.weight) * math::ease(3.5, dt);
        let pointer = self.pointer_now();
        let submit = self.submit && self.t - self.last_submit >= SUBMIT_GAP;
        if submit {
            self.last_submit = self.t;
        }
        let input = Input {
            keys: self.keys.min(MAX_KEYS),
            submit,
            pointer,
        };
        self.keys = 0;
        self.submit = false;
        let connected = is_connected(&live);
        let live_usage = live.usage;
        let synth = self.synth.at(self.t);
        let mut signals = live.or(synth);
        if connected {
            // Figures shown as facts are never made up: no reading, no usage.
            signals.usage = live_usage;
        } else {
            // No live state at all (a test, a gallery, a disconnected app):
            // stand-in events keep the picture moving. A connected app that's
            // quiet stays quiet: events are only ever real.
            signals.calls.clone_from(&synth.calls);
            signals.touches.clone_from(&synth.touches);
            signals.started = synth.started;
        }
        let cx = Ctx {
            t: self.t,
            dt,
            size: self.size,
            input: &input,
            signals: &signals,
        };
        let bg = &mut self.bg;
        if catch_unwind(AssertUnwindSafe(|| bg.update(&cx))).is_err() {
            self.failed = true;
        }
    }

    /// The background drawn in `pal`. Empty once it has failed.
    pub fn draw(&mut self, pal: &Palette) -> &Scene {
        self.scene.clear();
        if !self.failed {
            let cx = DrawCtx {
                t: self.t,
                size: self.size,
                pal,
                pointer: self.pointer_now(),
            };
            let (bg, scene) = (&self.bg, &mut self.scene);
            if catch_unwind(AssertUnwindSafe(|| bg.draw(&cx, scene))).is_err() {
                self.failed = true;
                self.scene.clear();
            }
            self.scene.sanitize();
        }
        &self.scene
    }

    fn pointer_now(&self) -> Option<Pointer> {
        match self.smooth {
            Some(s) if self.weight > 0.005 => Some(Pointer {
                x: s[0],
                y: s[1],
                k: self.weight,
            }),
            _ => None,
        }
    }
}

/// The client is feeding live state, so its event lists are the truth.
fn is_connected(live: &Signals) -> bool {
    live.machine.is_some() || !live.sessions.is_empty()
}

#[cfg(test)]
mod tests;
