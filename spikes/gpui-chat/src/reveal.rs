//! `--compare`: the same reply streamed three ways, side by side, from the
//! same bursty arrival timing, to choose how streaming should feel.
//!
//! - Raw: text appears the moment each burst arrives, mid-word if need be.
//! - Smooth: arrivals are buffered and revealed word by word at a steady
//!   pace that catches up with the backlog; new words fade in.
//! - Blocks: a paragraph, list, table or code block appears only once it is
//!   complete, and fades in.

use std::time::{Duration, Instant};

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{Inline, Specimen};
use crate::ds::{LABEL, Typed, space};
use crate::theme::Theme;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Reveal {
    /// A constant rate, for the specimen and stress pages.
    Rate,
    Raw,
    Smooth,
    Blocks,
}

impl Reveal {
    pub fn label(self) -> &'static str {
        match self {
            Reveal::Rate => "",
            Reveal::Raw => "Word by word, as it arrives",
            Reveal::Smooth => "Word by word, smoothed",
            Reveal::Blocks => "Block by block",
        }
    }
}

/// How long new text takes to fade in.
pub const FADE: Duration = Duration::from_millis(180);
/// How long a new block takes to fade in.
pub const BLOCK_FADE: Duration = Duration::from_millis(300);
/// Smooth reveal never trails the arrivals by much more than this.
const SMOOTH_LAG: f32 = 0.25;
/// …and never crawls slower than this, in characters per second.
const SMOOTH_MIN_CPS: f32 = 160.;

/// When each part of the message arrives, as (seconds, end byte). Bursts of
/// a few tokens every 25–70 ms, with the occasional longer pause, which is
/// roughly how agent text arrives over the network. Deterministic, so all
/// three panes get the same timing.
pub fn arrivals(msg: &str) -> Vec<(f32, usize)> {
    let mut seed = 0x9e3779b97f4a7c15u64;
    let mut rand = move |lo: u64, hi: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        lo + seed % (hi - lo + 1)
    };
    let mut out = Vec::new();
    let (mut t, mut at, mut n) = (0.5f32, 0usize, 0);
    while at < msg.len() {
        let mut end = (at + rand(4, 22) as usize).min(msg.len());
        while !msg.is_char_boundary(end) {
            end += 1;
        }
        at = end;
        n += 1;
        t += rand(25, 70) as f32 / 1000.;
        if n % 23 == 0 {
            t += rand(200, 450) as f32 / 1000.;
        }
        out.push((t, at));
    }
    out
}

impl Specimen {
    /// How many bytes of the message should be in the streamer now.
    pub fn reveal_target(&mut self) -> usize {
        let elapsed = self.start.elapsed().as_secs_f32();
        let arrived = self
            .arrivals
            .iter()
            .take_while(|(t, _)| *t <= elapsed)
            .last()
            .map_or(0, |(_, b)| *b);
        match self.reveal {
            Reveal::Rate => ((elapsed - 0.4).max(0.) * self.rate) as usize,
            Reveal::Raw | Reveal::Blocks => arrived,
            Reveal::Smooth => {
                let now = Instant::now();
                let dt = self
                    .last_tick
                    .map_or(0., |t| now.duration_since(t).as_secs_f32());
                self.last_tick = Some(now);
                let backlog = (arrived as f32 - self.revealed).max(0.);
                let cps = (backlog / SMOOTH_LAG).max(SMOOTH_MIN_CPS);
                self.revealed = (self.revealed + cps * dt).min(arrived as f32);
                let at = self.revealed as usize;
                if at >= self.message.len() {
                    return at;
                }
                // Whole words only.
                self.message[..at].rfind([' ', '\n']).map_or(0, |i| i + 1)
            }
        }
    }

    /// In smooth mode, fade in the characters of `inline` that are new since
    /// the last frames. Each inline is tracked by its block and position, in
    /// rendered characters, so markup never counts.
    pub(super) fn fade_inline(&mut self, inline: Inline) -> Inline {
        if self.reveal != Reveal::Smooth {
            return inline;
        }
        let key = (self.block_start, self.inline_ix);
        self.inline_ix += 1;
        let now = Instant::now();
        let len = inline.text.len();
        let seen = self.seen.entry(key).or_default();
        if seen.last().is_none_or(|(l, _)| *l < len) {
            seen.push((len, now));
        }
        let mut ramps = Vec::new();
        let mut prev = 0;
        for &(l, t) in seen.iter() {
            let age = now.duration_since(t);
            if age < FADE {
                ramps.push((
                    prev.min(len),
                    l.min(len),
                    age.as_secs_f32() / FADE.as_secs_f32(),
                ));
            }
            prev = l;
        }
        // Keep the newest entry as the baseline for the next frame.
        if seen.len() > 1 {
            let keep = seen
                .iter()
                .rposition(|(_, t)| now.duration_since(*t) >= FADE)
                .unwrap_or(0);
            seen.drain(..keep);
        }
        if !ramps.is_empty() {
            self.fading = true;
        }
        inline.faded(&ramps)
    }

    /// In block mode, how far a block has faded in.
    pub fn block_alpha(&mut self, start: usize) -> f32 {
        let t = *self.first_seen.entry(start).or_insert_with(Instant::now);
        let a = t.elapsed().as_secs_f32() / BLOCK_FADE.as_secs_f32();
        if a < 1. {
            self.fading = true;
        }
        a.min(1.)
    }
}

impl Inline {
    /// Multiply the colour alpha of byte ranges by `alpha`, splitting runs at
    /// the range edges.
    pub(super) fn faded(mut self, ramps: &[(usize, usize, f32)]) -> Inline {
        if ramps.is_empty() {
            return self;
        }
        let floor = |mut i: usize| {
            while i > 0 && !self.text.is_char_boundary(i) {
                i -= 1;
            }
            i
        };
        let mut cuts: Vec<usize> = ramps
            .iter()
            .flat_map(|(a, b, _)| [floor(*a), floor(*b)])
            .collect();
        let mut runs = Vec::new();
        let mut at = 0;
        for run in std::mem::take(&mut self.runs) {
            let end = at + run.len;
            cuts.sort_unstable();
            let mut pieces: Vec<usize> = cuts
                .iter()
                .copied()
                .filter(|c| *c > at && *c < end)
                .collect();
            pieces.push(end);
            let mut from = at;
            for to in pieces {
                let mid = from;
                let alpha = ramps
                    .iter()
                    .filter(|(a, b, _)| mid >= floor(*a) && mid < floor(*b))
                    .map(|r| r.2)
                    .fold(1f32, f32::min);
                let mut piece = run.clone();
                piece.len = to - from;
                piece.color = piece.color.opacity(piece.color.a * alpha);
                piece.background_color = piece.background_color.map(|c| c.opacity(c.a * alpha));
                runs.push(piece);
                from = to;
            }
            at = end;
        }
        self.runs = runs;
        self
    }
}

/// Three panes side by side, one per way of revealing.
pub struct Compare {
    panes: Vec<Entity<Specimen>>,
    theme: Theme,
    hid_cursor: bool,
}

impl Compare {
    pub fn new(panes: Vec<Entity<Specimen>>, theme: Theme) -> Self {
        Self {
            panes,
            theme,
            hid_cursor: false,
        }
    }
}

impl Render for Compare {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Keep the pointer out of the recording while the window is in front.
        if !self.hid_cursor {
            self.hid_cursor = true;
            window.on_next_frame(|_, _| hide_cursor());
        }
        let t = self.theme;
        div()
            .size_full()
            .flex()
            .bg(t.surface)
            .children(self.panes.iter().enumerate().map(|(i, pane)| {
                let label = pane.read(cx).reveal.label();
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .when(i > 0, |d| d.border_l_1().border_color(t.hairline))
                    .child(
                        div()
                            .px(px(space::S4))
                            .py(px(space::S2))
                            .border_b_1()
                            .border_color(t.hairline)
                            .ty(LABEL)
                            .text_color(t.quiet)
                            .child(label),
                    )
                    .child(div().flex_1().min_h_0().child(pane.clone()))
            }))
    }
}

#[cfg(target_os = "macos")]
// objc 0.2's macros test a `cargo-clippy` feature this crate doesn't declare.
#[allow(unexpected_cfgs)]
pub fn hide_cursor() {
    use objc::{class, msg_send, sel, sel_impl};
    // SAFETY: a class method on NSCursor, called on the main thread.
    unsafe {
        let _: () = msg_send![class!(NSCursor), hide];
    }
}

#[cfg(not(target_os = "macos"))]
pub fn hide_cursor() {}
