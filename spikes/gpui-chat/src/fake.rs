//! Deterministic fake data: no agents, no network, no usage.

use gpui_kit::SharedString;

use crate::theme::Status;

/// xorshift64*: tiny and deterministic, so every run renders the same data.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const WORDS: &[&str] = &[
    "the",
    "daemon",
    "session",
    "event",
    "log",
    "client",
    "renders",
    "from",
    "cache",
    "and",
    "streams",
    "the",
    "gap",
    "approval",
    "card",
    "status",
    "square",
    "hub",
    "relay",
    "machine",
    "diff",
    "hunk",
    "stage",
    "commit",
    "worktree",
    "checkout",
    "project",
    "sequence",
    "number",
    "subscribe",
    "after",
    "reconnect",
    "resume",
    "agent",
    "driver",
    "normalized",
    "tool",
    "call",
    "result",
    "markdown",
    "block",
    "tree",
    "parser",
    "streaming",
    "prefix",
    "finished",
    "blocks",
    "never",
    "move",
    "palette",
    "token",
    "frame",
    "budget",
    "under",
    "one",
    "millisecond",
    "so",
    "it",
    "feels",
    "instant",
    "with",
    "a",
    "virtualized",
    "list",
    "that",
    "only",
    "measures",
    "visible",
    "rows",
    "keeps",
    "memory",
    "flat",
    "over",
    "long",
    "sessions",
];

const COMMANDS: &[&str] = &[
    "cargo test -p dent-daemon",
    "cargo clippy --workspace -- -D warnings",
    "git status --short",
    "rg \"ListState\" crates/",
    "just check dent-client-core",
    "ls -la crates/daemon/src",
];

const FILES: &[&str] = &[
    "crates/daemon/src/event_log.rs",
    "crates/client-core/src/sync.rs",
    "crates/protocol/src/events.rs",
    "crates/desktop/src/chat/list.rs",
    "docs/daemon.md",
];

fn sentence(rng: &mut Rng, min: usize, spread: usize) -> String {
    let words = min + rng.below(spread);
    let mut s = String::new();
    for i in 0..words {
        let w = WORDS[rng.below(WORDS.len())];
        if i == 0 {
            let mut c = w.chars();
            if let Some(f) = c.next() {
                s.extend(f.to_uppercase());
                s.push_str(c.as_str());
            }
        } else {
            s.push(' ');
            s.push_str(w);
        }
    }
    s.push('.');
    s
}

fn paragraph(rng: &mut Rng) -> String {
    let n = 1 + rng.below(4);
    (0..n)
        .map(|_| sentence(rng, 6, 14))
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Clone)]
pub enum Row {
    User(SharedString),
    Assistant(Vec<SharedString>),
    Tool {
        name: SharedString,
        detail: SharedString,
        result: SharedString,
        ok: bool,
    },
    Approval(SharedString),
}

/// One row per event: user messages, assistant text, tool calls and approvals
/// in a plausible rhythm.
pub fn chat(events: usize, seed: u64) -> Vec<Row> {
    let mut rng = Rng::new(seed);
    let mut rows = Vec::with_capacity(events);
    while rows.len() < events {
        rows.push(Row::User(sentence(&mut rng, 5, 25).into()));
        let steps = 2 + rng.below(10);
        for _ in 0..steps {
            if rows.len() >= events {
                break;
            }
            rows.push(match rng.below(10) {
                0..=3 => Row::Assistant(
                    (0..1 + rng.below(3))
                        .map(|_| paragraph(&mut rng).into())
                        .collect(),
                ),
                4..=5 => {
                    let cmd = COMMANDS[rng.below(COMMANDS.len())];
                    let ok = rng.below(5) != 0;
                    Row::Tool {
                        name: "Bash".into(),
                        detail: cmd.into(),
                        result: if ok {
                            "ok · 0.8s".into()
                        } else {
                            "2 failed · 3.1s".into()
                        },
                        ok,
                    }
                }
                6..=8 => {
                    let f = FILES[rng.below(FILES.len())];
                    Row::Tool {
                        name: "Edit".into(),
                        detail: f.into(),
                        result: format!("+{} −{}", 1 + rng.below(40), rng.below(20)).into(),
                        ok: true,
                    }
                }
                _ => Row::Approval(COMMANDS[rng.below(COMMANDS.len())].into()),
            });
        }
    }
    rows.truncate(events);
    rows
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

pub struct DiffLine {
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub kind: LineKind,
    pub text: SharedString,
}

const CODE: &[&str] = &[
    "pub fn subscribe(&self, after: Seq) -> impl Stream<Item = Event> {",
    "    let tail = self.log.read_after(after)?;",
    "    for event in tail.iter().filter(|e| e.session == id) {",
    "        tx.send(event.clone()).await?;",
    "    }",
    "    // Deltas are coalesced into the final message at turn end.",
    "    let status = derive_status(&events);",
    "    Ok(Self { seq, session, kind, payload })",
    "}",
    "",
    "#[derive(Debug, Clone, Serialize, Deserialize)]",
    "pub struct Event { pub seq: Seq, pub session: SessionId, pub kind: EventKind }",
    "    match kind { EventKind::ToolCall(c) => c.render(cx), _ => fallback(kind) }",
];

pub fn diff(lines: usize, seed: u64) -> Vec<DiffLine> {
    let mut rng = Rng::new(seed);
    let (mut old, mut new) = (1u32, 1u32);
    let mut out = Vec::with_capacity(lines);
    while out.len() < lines {
        let run = 3 + rng.below(30);
        let kind = match rng.below(6) {
            0 => LineKind::Removed,
            1 | 2 => LineKind::Added,
            _ => LineKind::Context,
        };
        for _ in 0..run.min(lines - out.len()) {
            let text: SharedString = CODE[rng.below(CODE.len())].into();
            out.push(match kind {
                LineKind::Context => {
                    old += 1;
                    new += 1;
                    DiffLine {
                        old: Some(old - 1),
                        new: Some(new - 1),
                        kind,
                        text,
                    }
                }
                LineKind::Added => {
                    new += 1;
                    DiffLine {
                        old: None,
                        new: Some(new - 1),
                        kind,
                        text,
                    }
                }
                LineKind::Removed => {
                    old += 1;
                    DiffLine {
                        old: Some(old - 1),
                        new: None,
                        kind,
                        text,
                    }
                }
            });
        }
    }
    out
}

pub struct SessionRow {
    pub title: SharedString,
    pub machine: SharedString,
    pub status: Status,
}

pub fn sessions(n: usize, seed: u64) -> Vec<SessionRow> {
    let mut rng = Rng::new(seed);
    let machines = ["laptop", "mac-mini", "linux-box", "ec2-runner"];
    (0..n)
        .map(|i| SessionRow {
            title: if i == 0 {
                "Event log compaction".into()
            } else {
                sentence(&mut rng, 2, 4)
                    .trim_end_matches('.')
                    .to_string()
                    .into()
            },
            machine: machines[rng.below(machines.len())].into(),
            status: match rng.below(8) {
                0 => Status::Needs,
                1 | 2 => Status::Working,
                3 => Status::Waiting,
                4 | 5 => Status::Done,
                _ => Status::Idle,
            },
        })
        .collect()
}

/// Words to append while simulating a streaming reply.
pub fn stream_words(seed: u64) -> impl Iterator<Item = &'static str> {
    let mut rng = Rng::new(seed);
    std::iter::repeat_with(move || WORDS[rng.below(WORDS.len())])
}
