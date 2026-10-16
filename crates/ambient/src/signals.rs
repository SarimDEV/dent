//! What a background may know about dent: one narrow struct the client
//! fills from its live state. Anything the client leaves empty is filled in
//! from a synthetic source of the same shape, so every background always has
//! something believable to show and looks the same with or without data.

use crate::math::{Rng, noise3};

/// Kinds of agent work, in spectrum order (violet to orange).
pub const KINDS: [&str; 6] = ["think", "read", "search", "edit", "bash", "web"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Think = 0,
    Read = 1,
    Search = 2,
    Edit = 3,
    Bash = 4,
    Web = 5,
}

impl Kind {
    /// Map an agent's tool name onto a kind of work.
    pub fn of_tool(name: &str) -> Kind {
        let n = name.to_ascii_lowercase();
        let n = n.rsplit("__").next().unwrap_or(&n);
        match n {
            "read" | "view" | "notebookread" | "read_file" => Kind::Read,
            "grep" | "glob" | "ls" | "search" | "find" | "list" | "toolsearch" => Kind::Search,
            "edit"
            | "write"
            | "multiedit"
            | "notebookedit"
            | "apply_patch"
            | "str_replace_based_edit_tool" => Kind::Edit,
            "bash" | "shell" | "exec" | "command" | "local_shell" | "monitor" => Kind::Bash,
            "webfetch" | "websearch" | "fetch" | "web_search" => Kind::Web,
            _ => Kind::Think,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Session {
    pub name: String,
    pub working: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Touch {
    /// A short path, as people would recognise it (`src/chat.rs`).
    pub path: String,
    pub edit: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct File {
    /// The top-level folder or crate it belongs to.
    pub group: String,
    pub name: String,
    /// Size, in any consistent unit (lines or bytes).
    pub weight: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Machine {
    pub name: String,
    /// Round trip to it, when known.
    pub latency_ms: Option<f32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Usage {
    /// Share of the current limit window used, 0..1.
    pub used: f32,
    /// Seconds until the window resets.
    pub resets_in: f32,
    /// Length of the window in seconds.
    pub window: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LogLine {
    /// Local time of day, minutes since midnight.
    pub minute: f32,
    pub text: String,
}

/// Live facts for one frame. Lists that describe events (`calls`,
/// `touches`, `started`) hold what happened since the previous frame; the
/// rest describe the current state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Signals {
    /// Local time of day, minutes since midnight.
    pub minute: Option<f32>,
    /// Today's local date: year, month (1–12), day.
    pub date: Option<(i32, u32, u32)>,
    pub project: Option<String>,
    pub machine: Option<String>,
    /// Tool calls since the last frame, across every session.
    pub calls: Vec<Kind>,
    /// Files read or edited since the last frame.
    pub touches: Vec<Touch>,
    /// Sessions started since the last frame, anywhere.
    pub started: u32,
    /// Recent sessions, newest last.
    pub sessions: Vec<Session>,
    /// How many sessions are working right now, when the client knows. Never
    /// filled from the stand-in.
    pub working: Option<u32>,
    /// Recent things that happened, oldest first (session titles, finished turns).
    pub log: Vec<LogLine>,
    /// The open project's files.
    pub files: Vec<File>,
    pub machines: Vec<Machine>,
    pub usage: Option<Usage>,
}

impl Signals {
    /// Fill every empty field from `other`.
    pub fn or(mut self, other: &Signals) -> Signals {
        if self.minute.is_none() {
            self.minute = other.minute;
        }
        if self.date.is_none() {
            self.date = other.date;
        }
        if self.project.is_none() {
            self.project = other.project.clone();
        }
        if self.machine.is_none() {
            self.machine = other.machine.clone();
        }
        if self.sessions.is_empty() {
            self.sessions = other.sessions.clone();
        }
        if self.log.is_empty() {
            self.log = other.log.clone();
        }
        if self.files.is_empty() {
            self.files = other.files.clone();
        }
        if self.machines.is_empty() {
            self.machines = other.machines.clone();
        }
        if self.usage.is_none() {
            self.usage = other.usage;
        }
        self
    }

    /// True when nothing has happened lately, so event streams should be
    /// topped up from the synthetic source to keep the picture alive.
    pub fn quiet(&self) -> bool {
        self.calls.is_empty() && self.touches.is_empty() && self.started == 0
    }
}

const LOG: [&str; 12] = [
    "Stop guard holds longer after sending",
    "Copy Resume Command",
    "⌘-click opens a web address or file",
    "Page Up and Down scroll the chat",
    "⌘K finds the open project's files",
    "Quote in Reply",
    "Right-click menus in the chat",
    "Your own links open on click",
    "Drops say where they'll land",
    "Notifications clear when opened",
    "Batch 2 review fixes",
    "Keyboard kept when the terminal hides",
];

const FILES: [(&str, &str, f32); 18] = [
    ("desktop", "app.rs", 5376.),
    ("desktop", "walk.rs", 4733.),
    ("desktop", "chat.rs", 4477.),
    ("desktop", "viewer.rs", 2599.),
    ("desktop", "markdown.rs", 2476.),
    ("desktop", "select.rs", 1512.),
    ("desktop", "bridge.rs", 1429.),
    ("desktop", "cards.rs", 1102.),
    ("desktop", "web.rs", 1072.),
    ("desktop", "shell.rs", 1016.),
    ("daemon", "session.rs", 1799.),
    ("daemon", "daemon.rs", 966.),
    ("daemon", "wire.rs", 803.),
    ("client-core", "timeline.rs", 1434.),
    ("protocol", "lib.rs", 1143.),
    ("fake-claude", "main.rs", 1238.),
    ("session-holder", "holder.rs", 575.),
    ("session-holder", "wire.rs", 166.),
];

/// A believable stand-in for live data, driven by time. Its event rates and
/// shapes are tuned to look like a busy afternoon, never like noise.
pub struct Synth {
    rng: Rng,
    next_call: f32,
    next_touch: f32,
    next_line: f32,
    next_start: f32,
    minute: f32,
    li: usize,
    base: Signals,
}

impl Synth {
    pub fn new(seed: u64) -> Self {
        let mut rng = Rng::new(seed ^ 0xA5A5);
        let minute = 9. * 60. + 12.;
        let mut log = Vec::new();
        let mut m = minute;
        for text in LOG.iter().take(10) {
            m += 3. + (rng.f() * 15.).floor();
            log.push(LogLine {
                minute: m,
                text: text.to_string(),
            });
        }
        let base = Signals {
            project: Some("dent".into()),
            machine: Some("mac-mini".into()),
            sessions: [
                "hub", "daemon", "web", "desktop", "cli", "iphone", "docs", "tests", "build",
                "lint", "site", "notes", "vault", "infra", "ci", "bench",
            ]
            .iter()
            .enumerate()
            .map(|(i, n)| Session {
                name: n.to_string(),
                working: i % 3 != 2,
            })
            .collect(),
            files: FILES
                .iter()
                .map(|&(g, n, w)| File {
                    group: g.into(),
                    name: n.into(),
                    weight: w,
                })
                .collect(),
            machines: [
                "mac-mini",
                "studio",
                "iphone",
                "hub",
                "pi-garage",
                "cloud-1",
                "laptop",
                "nas",
                "office",
            ]
            .iter()
            .map(|n| Machine {
                name: n.to_string(),
                latency_ms: None,
            })
            .collect(),
            usage: Some(Usage {
                used: 0.42,
                resets_in: 3. * 3600.,
                window: 5. * 3600.,
            }),
            log,
            ..Default::default()
        };
        Synth {
            rng,
            next_call: 0.,
            next_touch: 1.,
            next_line: 1.4,
            next_start: 12.,
            minute: m,
            li: 10,
            base,
        }
    }

    /// The stand-in signals at time `t` (seconds since the background
    /// started). Updated in place, so nothing is copied unless a caller needs it.
    pub fn at(&mut self, t: f32) -> &Signals {
        let s = &mut self.base;
        s.calls.clear();
        s.touches.clear();
        s.started = 0;
        s.minute = Some(self.minute + t / 60.);
        // Tool calls arrive in bursts, with the mix drifting slowly.
        while self.next_call <= t {
            let busy = 0.5 + 0.5 * noise3(t * 0.05, 1.7, 3.3);
            self.next_call += 0.08 + (1. - busy) * 0.9 * self.rng.f();
            let w: Vec<f32> = (0..6)
                .map(|i| (0.6 + 0.5 * noise3(t * 0.04, i as f32 * 3.1, 9.)).max(0.05))
                .collect();
            let total: f32 = w.iter().sum();
            let mut pick = self.rng.f() * total;
            let mut k = 0;
            while k < 5 && pick > w[k] {
                pick -= w[k];
                k += 1;
            }
            s.calls.push(
                [
                    Kind::Think,
                    Kind::Read,
                    Kind::Search,
                    Kind::Edit,
                    Kind::Bash,
                    Kind::Web,
                ][k],
            );
        }
        while self.next_touch <= t {
            self.next_touch += 1.6 + self.rng.f() * 1.6;
            let f = &FILES[(self.rng.f().powf(1.6) * FILES.len() as f32) as usize % FILES.len()];
            s.touches.push(Touch {
                path: f.1.to_string(),
                edit: self.rng.f() < 0.4,
            });
        }
        if self.next_start <= t {
            self.next_start = t + 14. + self.rng.f() * 10.;
            s.started = 1;
        }
        while self.next_line <= t {
            self.next_line += 4. + self.rng.f() * 6.;
            self.minute += 3. + (self.rng.f() * 15.).floor();
            s.log.push(LogLine {
                minute: self.minute + t / 60.,
                text: LOG[self.li % LOG.len()].to_string(),
            });
            self.li += 1;
            if s.log.len() > 40 {
                s.log.remove(0);
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_map_to_kinds() {
        assert_eq!(Kind::of_tool("Read"), Kind::Read);
        assert_eq!(Kind::of_tool("Grep"), Kind::Search);
        assert_eq!(Kind::of_tool("MultiEdit"), Kind::Edit);
        assert_eq!(Kind::of_tool("Bash"), Kind::Bash);
        assert_eq!(Kind::of_tool("WebFetch"), Kind::Web);
        assert_eq!(Kind::of_tool("mcp__dent__update_plan"), Kind::Think);
    }

    #[test]
    fn synth_produces_events_over_time() {
        let mut s = Synth::new(1);
        let (mut calls, mut touches) = (0, 0);
        for f in 0..600 {
            let sig = s.at(f as f32 / 60.);
            calls += sig.calls.len();
            touches += sig.touches.len();
        }
        assert!(calls > 5 && touches >= 2);
    }

    #[test]
    fn real_fields_win() {
        let real = Signals {
            project: Some("mine".into()),
            ..Default::default()
        };
        let merged = real.or(Synth::new(1).at(0.));
        assert_eq!(merged.project.as_deref(), Some("mine"));
        assert!(!merged.files.is_empty());
    }
}
