//! Field Notes palette (docs/design.md) and status squares.

use gpui_kit::prelude::*;
use gpui_kit::*;

/// Claude's own mark colour, the same on every theme: it names the agent,
/// so it isn't a signal.
pub const CLAUDE: Hsla = Hsla {
    h: 15. / 360.,
    s: 0.63,
    l: 0.6,
    a: 1.,
};

#[derive(Clone, Copy)]
pub struct Theme {
    pub dark: bool,
    pub surface: Hsla,
    pub raised: Hsla,
    pub panel: Hsla,
    pub ink: Hsla,
    pub quiet: Hsla,
    /// Line numbers, placeholders, separators: 3:1, never for reading.
    pub faint: Hsla,
    pub hairline: Hsla,
    pub working: (Hsla, Hsla),
    pub needs: (Hsla, Hsla),
    pub done: (Hsla, Hsla),
    pub waiting: (Hsla, Hsla),
    pub idle: (Hsla, Hsla),
    pub added: Hsla,
    pub removed: Hsla,
    /// Six hues for kinds of agent work, violet to orange (think, read,
    /// search, edit, bash, web), for ambient backgrounds. Never the
    /// needs-you magenta.
    pub spectrum: [Hsla; 6],
}

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

impl Theme {
    /// Signal colours pair with a paler partner on Paper and a deeper one on
    /// Carbon, which the working square breathes from.
    pub fn paper() -> Self {
        Self {
            dark: false,
            surface: c(0xF1EEE7),
            raised: c(0xFAF8F3),
            panel: c(0xFAF8F3),
            ink: c(0x171614),
            quiet: c(0x6E6A62),
            faint: c(0x8C877D),
            hairline: c(0xDCD7CC),
            working: (c(0x2BB8D6), c(0x9FE3F0)),
            needs: (c(0xE0309F), c(0xF4A6D6)),
            done: (c(0x3FBF4F), c(0xA8E6AE)),
            waiting: (c(0xE8A317), c(0xF5D58C)),
            idle: (c(0xB9B4A9), c(0xDCD7CC)),
            added: c(0xE2F2DE),
            removed: c(0xF3E2DA),
            spectrum: [0x6A4FC9, 0x2A6FD1, 0x1FA9C9, 0x3FAF4F, 0xD99A12, 0xE2672A].map(c),
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
            faint: c(0x706B63),
            hairline: c(0x2E2C28),
            working: (c(0x4FD3EC), c(0x1D5864)),
            needs: (c(0xF04FB8), c(0x6A1F50)),
            done: (c(0x5FD86B), c(0x22592A)),
            waiting: (c(0xF0B43A), c(0x6B4F14)),
            idle: (c(0x6B675F), c(0x2E2C28)),
            added: c(0x1A2E1C),
            removed: c(0x33211B),
            spectrum: [0x9B84F0, 0x4F9BF0, 0x4FD3EC, 0x5FD86B, 0xF0B43A, 0xF5884A].map(c),
        }
    }
}

/// Colours mix in RGB so a tween never swings through other hues.
/// `a` part way to `b`.
pub fn mix_pub(a: Hsla, b: Hsla, k: f32) -> Hsla {
    mix(a, b, k)
}

fn mix(a: Hsla, b: Hsla, k: f32) -> Hsla {
    let (a, b) = (a.to_rgb(), b.to_rgb());
    let l = |x: f32, y: f32| x + (y - x) * k;
    Rgba {
        r: l(a.r, b.r),
        g: l(a.g, b.g),
        b: l(a.b, b.b),
        a: l(a.a, b.a),
    }
    .into()
}

impl Theme {
    /// Every token part way from `a` to `b`, for the theme colour tween.
    pub fn mix(a: &Theme, b: &Theme, k: f32) -> Theme {
        let p = |x: (Hsla, Hsla), y: (Hsla, Hsla)| (mix(x.0, y.0, k), mix(x.1, y.1, k));
        Theme {
            dark: if k < 0.5 { a.dark } else { b.dark },
            surface: mix(a.surface, b.surface, k),
            raised: mix(a.raised, b.raised, k),
            panel: mix(a.panel, b.panel, k),
            ink: mix(a.ink, b.ink, k),
            quiet: mix(a.quiet, b.quiet, k),
            faint: mix(a.faint, b.faint, k),
            hairline: mix(a.hairline, b.hairline, k),
            working: p(a.working, b.working),
            needs: p(a.needs, b.needs),
            done: p(a.done, b.done),
            waiting: p(a.waiting, b.waiting),
            idle: p(a.idle, b.idle),
            added: mix(a.added, b.added, k),
            removed: mix(a.removed, b.removed, k),
            spectrum: std::array::from_fn(|i| mix(a.spectrum[i], b.spectrum[i], k)),
        }
    }
}

/// State tokens are ink at an alpha, so one value works on every surface.
impl Theme {
    fn a(&self, paper: f32, carbon: f32) -> f32 {
        if self.dark { carbon } else { paper }
    }
    pub fn hover(&self) -> Hsla {
        self.ink.opacity(self.a(0.05, 0.06))
    }
    pub fn press(&self) -> Hsla {
        self.ink.opacity(self.a(0.09, 0.10))
    }
    /// Menus and popovers: in Carbon a step lighter than the field they
    /// open from, as the Mac lifts a menu, so one never reads as a second
    /// box; on Paper the shadow does that.
    pub fn popover(&self) -> Hsla {
        if self.dark {
            mix(self.raised, self.ink, 0.05)
        } else {
            self.raised
        }
    }
    /// Between `quiet` and `ink`: what a tool line worked on (a file, a
    /// command), so the verb before it can sit back.
    pub fn secondary(&self) -> Hsla {
        mix(self.quiet, self.ink, 0.45)
    }
    /// The theme for words said on the way to an answer: their ink steps
    /// back to `secondary`, so the work recedes and the answer stands out.
    pub fn narrating(&self) -> Self {
        Self {
            ink: self.secondary(),
            ..*self
        }
    }
    /// Your messages' fill: a tint of ink, no edge, so a message reads as
    /// yours without becoming a card.
    pub fn tint(&self) -> Hsla {
        self.ink.opacity(self.a(0.05, 0.07))
    }
    pub fn selected(&self) -> Hsla {
        self.ink.opacity(self.a(0.10, 0.12))
    }
    /// A fill lifted off what's under it (`lifted`): `raised` on Paper; on
    /// Carbon, where a raised fill is barely off the page or a magenta
    /// wash, a step lighter. A floating control over the chat ("Latest")
    /// and an approval's command box.
    pub fn lifted(&self) -> Hsla {
        if self.dark {
            mix(self.raised, self.ink, 0.08)
        } else {
            self.raised
        }
    }

    /// Selected text, in replies and in inputs alike: a calm blue, as the
    /// Mac tints a selection in the window in use (a grey one reads as a
    /// window in the background), light enough that grey text stays legible
    /// on it.
    pub fn selection(&self) -> Hsla {
        hsla(212. / 360., 0.62, 0.56, self.a(0.22, 0.30))
    }
    pub fn wash(&self, signal: Hsla) -> Hsla {
        signal.opacity(self.a(0.07, 0.12))
    }
    pub fn edge(&self, signal: Hsla) -> Hsla {
        signal.opacity(self.a(0.45, 0.5))
    }
    pub fn signal(&self, s: Status) -> (Hsla, Hsla) {
        match s {
            Status::Working => self.working,
            Status::Needs => self.needs,
            Status::Done => self.done,
            Status::Waiting => self.waiting,
            Status::Idle => self.idle,
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

/// Font roles, chosen at startup so different pairings can be compared.
#[allow(dead_code)] // the name picks a pairing in the gpui-chat spike
pub struct Fonts {
    pub name: &'static str,
    /// Interface chrome.
    pub sans: &'static str,
    /// Chat message text.
    pub body: &'static str,
    /// Code, commands, diffs.
    pub mono: &'static str,
    /// Small uppercase metadata labels.
    pub label: &'static str,
    /// Session titles and display.
    pub serif: &'static str,
}

/// The first entry is the default and is bundled; the rest load from
/// `--font-dir` for comparison (see docs/decisions/0032).
pub const PAIRINGS: &[Fonts] = &[
    Fonts {
        name: "book",
        sans: "IBM Plex Sans",
        body: "Literata",
        mono: "CommitMono",
        label: "IBM Plex Sans",
        serif: "Instrument Serif",
    },
    Fonts {
        name: "geist",
        sans: "Geist",
        body: "Geist",
        mono: "Geist Mono",
        label: "Geist",
        serif: "Instrument Serif",
    },
    Fonts {
        name: "plex",
        sans: "IBM Plex Sans",
        body: "IBM Plex Sans",
        mono: "IBM Plex Mono",
        label: "IBM Plex Sans",
        serif: "Instrument Serif",
    },
    Fonts {
        name: "atkinson",
        sans: "Atkinson Hyperlegible Next",
        body: "Atkinson Hyperlegible Next",
        mono: "Atkinson Hyperlegible Mono",
        label: "Atkinson Hyperlegible Next",
        serif: "Instrument Serif",
    },
    Fonts {
        name: "fira",
        sans: "Fira Sans",
        body: "Fira Sans",
        mono: "Fira Mono",
        label: "Fira Sans",
        serif: "Instrument Serif",
    },
];

pub static FONTS: std::sync::OnceLock<&'static Fonts> = std::sync::OnceLock::new();

fn fonts() -> &'static Fonts {
    FONTS.get().copied().unwrap_or(&PAIRINGS[0])
}
pub fn sans() -> &'static str {
    fonts().sans
}
pub fn body() -> &'static str {
    fonts().body
}
pub fn mono() -> &'static str {
    fonts().mono
}
pub fn label() -> &'static str {
    fonts().label
}
pub fn serif() -> &'static str {
    fonts().serif
}

/// A 12px status square, drawn as a 4×4 pixel pattern so status never relies
/// on colour alone (the same patterns as the phone): needs you = solid,
/// done = a corner step, working = a checker, waiting = dots, idle = an
/// outline. Working is the one that moves: with motion on, a small rounded
/// square that breathes smoothly between its paler partner and the signal
/// (`motion::BREATH_FPS`); with Reduce Motion, it holds still as its
/// checker, so it never reads as needs-you's solid square.
pub fn status_square(status: Status, t: &Theme) -> Div {
    status_square_at(status, t, 1.)
}

/// The square drawn at `scale` of its size inside a fixed 12px box, for the
/// Confirm pop; neighbours never move.
pub fn status_square_at(status: Status, t: &Theme, scale: f32) -> Div {
    let (signal, partner) = t.signal(status);
    let s = crate::ds::size::SQUARE * scale;
    if status == Status::Working && !crate::reduce_motion() {
        // One shared clock, so every working square breathes together.
        let secs = crate::PROCESS_START
            .get()
            .map(|p| p.elapsed().as_secs_f32())
            .unwrap_or(0.);
        let k = breath(secs);
        let inner = s * 0.84 * (0.9 + 0.1 * k);
        return div()
            .size(px(crate::ds::size::SQUARE))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .size(px(inner))
                    .rounded(px(inner * 0.28))
                    .bg(partner)
                    .child(
                        div()
                            .size_full()
                            .rounded(px(inner * 0.28))
                            .bg(signal.opacity(0.3 + 0.7 * k)),
                    ),
            );
    }
    // Flat shapes, never pixel patterns, each its own form so a status
    // reads without its colour: needs is filled, done carries a check,
    // waiting a square within a square, idle an outline, and working (when
    // it can't breathe) its paler fill.
    let line = (s / 8.).max(1.);
    let mark = match status {
        Status::Needs => div().size(px(s)).bg(signal),
        Status::Done => div()
            .size(px(s))
            .bg(signal)
            .flex()
            .items_center()
            .justify_center()
            .child(crate::ui::icon("check", s * 0.82, t.surface)),
        Status::Working => div()
            .size(px(s))
            .bg(partner)
            .child(div().size_full().bg(signal.opacity(0.65))),
        Status::Waiting => div()
            .size(px(s))
            .border(px(line))
            .border_color(signal)
            .flex()
            .items_center()
            .justify_center()
            .child(div().size(px(s * 0.34)).bg(signal)),
        Status::Idle => div().size(px(s)).border(px(line)).border_color(signal),
    };
    div()
        .size(px(crate::ds::size::SQUARE))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(mark)
}

/// The working square's breath at `secs`: 0 (its paler partner) to 1 (the
/// full signal) and back over `motion::WORKING`, eased in and out.
pub fn breath(secs: f32) -> f32 {
    let p = secs * 1000. / crate::ds::motion::WORKING;
    0.5 - 0.5 * (p * std::f32::consts::TAU).cos()
}
