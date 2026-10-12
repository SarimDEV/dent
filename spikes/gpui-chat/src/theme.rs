//! Field Notes palette (docs/design.md) and status squares.

use gpui_kit::*;

#[derive(Clone, Copy)]
pub struct Theme {
    pub dark: bool,
    pub surface: Hsla,
    pub raised: Hsla,
    pub panel: Hsla,
    pub ink: Hsla,
    pub quiet: Hsla,
    pub hairline: Hsla,
    pub working: (Hsla, Hsla),
    pub needs: (Hsla, Hsla),
    pub done: (Hsla, Hsla),
    pub waiting: (Hsla, Hsla),
    pub idle: (Hsla, Hsla),
    pub added: Hsla,
    pub removed: Hsla,
}

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

impl Theme {
    /// Signal colours pair with a paler partner on Paper and a deeper one on
    /// Carbon, for the dither.
    pub fn paper() -> Self {
        Self {
            dark: false,
            surface: c(0xF1EEE7),
            raised: c(0xFAF8F3),
            panel: c(0xFAF8F3),
            ink: c(0x171614),
            quiet: c(0x6E6A62),
            hairline: c(0xDCD7CC),
            working: (c(0x2BB8D6), c(0x9FE3F0)),
            needs: (c(0xE0309F), c(0xF4A6D6)),
            done: (c(0x3FBF4F), c(0xA8E6AE)),
            waiting: (c(0xE8A317), c(0xF5D58C)),
            idle: (c(0xB9B4A9), c(0xDCD7CC)),
            added: c(0xE2F2DE),
            removed: c(0xF6E0EC),
        }
    }

    pub fn carbon() -> Self {
        Self {
            dark: true,
            surface: c(0x161513),
            raised: c(0x1F1E1B),
            panel: c(0x1B1A17),
            ink: c(0xECE8DF),
            quiet: c(0x8F8A80),
            hairline: c(0x2E2C28),
            working: (c(0x4FD3EC), c(0x1D5864)),
            needs: (c(0xF04FB8), c(0x6A1F50)),
            done: (c(0x5FD86B), c(0x22592A)),
            waiting: (c(0xF0B43A), c(0x6B4F14)),
            idle: (c(0x6B675F), c(0x2E2C28)),
            added: c(0x1A2E1C),
            removed: c(0x35192A),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Working,
    Needs,
    Done,
    Waiting,
    Idle,
}

pub const SANS: &str = "Geist";
pub const MONO: &str = "Geist Mono";
pub const SERIF: &str = "Instrument Serif";

/// A 12px status square. Each status has its own pattern so it never relies on
/// colour alone: working = 2px dither, needs you = solid, done = dots,
/// waiting = outline, idle = quiet outline.
pub fn status_square(status: Status, t: &Theme) -> Div {
    let (signal, partner) = match status {
        Status::Working => t.working,
        Status::Needs => t.needs,
        Status::Done => t.done,
        Status::Waiting => t.waiting,
        Status::Idle => t.idle,
    };
    match status {
        Status::Needs => div().size(px(12.)).bg(signal),
        // Fine 2px dither of signal and partner.
        Status::Working => div()
            .size(px(12.))
            .flex()
            .flex_col()
            .children((0..6).map(move |y| {
                div().flex().children((0..6).map(move |x| {
                    div()
                        .size(px(2.))
                        .bg(if (x + y) % 2 == 0 { signal } else { partner })
                }))
            })),
        // Four round dots on a clear ground.
        Status::Done => div()
            .size(px(12.))
            .flex()
            .flex_wrap()
            .gap(px(2.))
            .p(px(1.))
            .children((0..4).map(move |_| div().size(px(4.)).rounded_full().bg(signal))),
        Status::Waiting | Status::Idle => div().size(px(12.)).border_2().border_color(signal),
    }
}
