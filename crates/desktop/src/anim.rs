//! The motion vocabulary's building blocks (docs/design-system.md, Motion):
//! curves and the digit ticker.

#![allow(dead_code)] // shared with the gpui-chat spike

use std::time::Instant;

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::ds::{Type, Typed, motion};

/// Linear progress in 0..=1 since `start`, after an optional delay.
pub fn progress(start: Instant, ms: f32) -> f32 {
    progress_after(start, 0., ms)
}

pub fn progress_after(start: Instant, delay_ms: f32, ms: f32) -> f32 {
    ((start.elapsed().as_secs_f32() * 1000. - delay_ms) / ms).clamp(0., 1.)
}

/// A CSS-style cubic Bézier easing, solved for x with Newton's method.
fn bezier(x1: f32, y1: f32, x2: f32, y2: f32, x: f32) -> f32 {
    if x <= 0. {
        return 0.;
    }
    if x >= 1. {
        return 1.;
    }
    let curve = |a: f32, b: f32, t: f32| {
        let u = 1. - t;
        3. * u * u * t * a + 3. * u * t * t * b + t * t * t
    };
    let slope = |a: f32, b: f32, t: f32| {
        let u = 1. - t;
        3. * u * u * a + 6. * u * t * (b - a) + 3. * t * t * (1. - b)
    };
    let mut t = x;
    for _ in 0..8 {
        let d = slope(x1, x2, t);
        if d.abs() < 1e-5 {
            break;
        }
        t = (t - (curve(x1, x2, t) - x) / d).clamp(0., 1.);
    }
    curve(y1, y2, t)
}

/// `ease.out`: everything that enters or responds to the user.
pub fn ease_out(t: f32) -> f32 {
    bezier(0.23, 1., 0.32, 1., t)
}

/// `ease.in_out`: something already on screen changing shape.
pub fn ease_in_out(t: f32) -> f32 {
    bezier(0.77, 0., 0.175, 1., t)
}

/// A spring given as a perceptual duration and bounce, stepped analytically so
/// it can be retargeted mid-flight without a jump.
#[derive(Clone, Copy)]
pub struct Spring {
    config: SpringConfig,
    pub state: SpringState,
}

/// A spring from a perceptual duration (seconds) and bounce (0 is none).
pub fn spring_config(duration: f32, bounce: f32) -> SpringConfig {
    let stiffness = (std::f32::consts::TAU / duration).powi(2);
    let damping = 4. * std::f32::consts::PI * (1. - bounce) / duration;
    SpringConfig::new(stiffness, damping, 1.)
}

impl Spring {
    pub fn new(duration: f32, bounce: f32, at: f32) -> Self {
        Self {
            config: spring_config(duration, bounce),
            state: SpringState {
                position: at,
                velocity: 0.,
            },
        }
    }
    /// `spring.snappy`
    pub fn snappy(at: f32) -> Self {
        Self::new(0.25, 0., at)
    }
    /// `spring.pop`
    pub fn pop(at: f32) -> Self {
        Self::new(0.3, 0.2, at)
    }
    /// Advance towards `target`; returns true while still moving.
    pub fn step(&mut self, target: f32, dt: f32) -> bool {
        if self.config.is_settled(self.state, target, 0.001) {
            self.state = SpringState {
                position: target,
                velocity: 0.,
            };
            return false;
        }
        self.state = self.config.step(self.state, target, dt);
        true
    }
    pub fn value(&self) -> f32 {
        self.state.position
    }
}

/// `spring.pop` as a pure function of time from 0 to 1, for one-shot marks.
pub fn pop(t_secs: f32) -> f32 {
    let mut s = Spring::pop(0.);
    let mut left = t_secs;
    while left > 0. {
        let dt = left.min(1. / 120.);
        s.step(1., dt);
        left -= dt;
    }
    s.value()
}

/// A number that changed: `from` rolls to `to` (the Tick motion).
#[derive(Clone)]
pub struct Tick {
    pub from: String,
    pub to: String,
    pub start: Instant,
}

impl Tick {
    pub fn new(to: String) -> Self {
        Self {
            from: to.clone(),
            to,
            start: Instant::now(),
        }
    }
    /// Retarget; digits already rolling restart from the current value.
    pub fn set(&mut self, to: String) {
        if to != self.to {
            self.from = std::mem::replace(&mut self.to, to);
            self.start = Instant::now();
        }
    }
    pub fn animating(&self) -> bool {
        self.start.elapsed().as_secs_f32() * 1000. < motion::SLOW + motion::STAGGER_CAP
    }
    /// Render in a monospace style so every digit column is the same width.
    pub fn render(&self, ty: Type, color: Hsla) -> Div {
        let n = self.from.chars().count().max(self.to.chars().count());
        let pad = |s: &str| format!("{s:>n$}").chars().collect::<Vec<_>>();
        let (from, to) = (pad(&self.from), pad(&self.to));
        let up = digits(&self.to) >= digits(&self.from);
        let mut changed = 0.;
        div().flex().ty(ty).text_color(color).children(
            (0..n)
                .rev()
                .map(|i| {
                    let (a, b) = (from[i], to[i]);
                    if a == b {
                        return div().child(b.to_string());
                    }
                    let delay = (changed * motion::STAGGER).min(motion::STAGGER_CAP);
                    changed += 1.;
                    let k = ease_out(progress_after(self.start, delay, motion::SLOW));
                    // Old glyph leaves, new glyph enters from below (up) or above.
                    let shift = if up { -k } else { k - 1. } * ty.line;
                    let (first, second) = if up { (a, b) } else { (b, a) };
                    div().h(px(ty.line)).overflow_hidden().child(
                        div()
                            .relative()
                            .top(px(shift))
                            .flex()
                            .flex_col()
                            .child(div().h(px(ty.line)).child(first.to_string()))
                            .child(div().h(px(ty.line)).child(second.to_string())),
                    )
                })
                .collect::<Vec<_>>()
                .into_iter()
                .rev(),
        )
    }
}

fn digits(s: &str) -> u64 {
    s.chars()
        .filter(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap_or(0)
}
