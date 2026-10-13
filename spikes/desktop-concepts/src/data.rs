//! Fake sessions, artifacts and changes. Everything is local: no agents run.

use std::rc::Rc;

use crate::theme::Status;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// A web page, shown live in the preview.
    Html,
    /// A Markdown document, rendered natively.
    Doc,
    /// A picture: a screenshot, a chart.
    Image,
}

impl Kind {
    pub fn icon(self) -> &'static str {
        match self {
            Kind::Html => "app-window",
            Kind::Doc => "file-text",
            Kind::Image => "image",
        }
    }
}

/// Something an agent made that is worth looking at, with every version it
/// wrote. Writing the same path again adds a version; nothing is replaced.
#[derive(Clone)]
pub struct Artifact {
    pub id: &'static str,
    pub kind: Kind,
    pub title: &'static str,
    pub path: &'static str,
    pub versions: Vec<Rc<[u8]>>,
}

#[derive(Clone)]
pub enum Item {
    User(String),
    /// A reply in Markdown-lite: paragraphs, `code`, **strong**.
    Text(String),
    /// Tool calls folded to one line, expandable.
    Tools {
        summary: String,
        lines: Vec<String>,
        open: bool,
    },
    /// The agent wrote a viewable file: a card that opens it.
    Made {
        art: &'static str,
        version: usize,
    },
    Approval {
        command: String,
        why: String,
    },
    /// A question with choices; the reply is one click.
    Question {
        text: String,
        options: Vec<String>,
    },
    /// Files changed this turn, opening Changes.
    Changed {
        files: usize,
        added: usize,
        removed: usize,
    },
    Pr {
        number: u32,
        title: String,
        checks: String,
    },
    /// One quiet line: Allowed, Stopped, Resumed…
    Note {
        icon: &'static str,
        text: String,
    },
    Queued(String),
}

#[derive(Clone)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub project: String,
    pub machine: String,
    pub agent: String,
    pub permission: String,
    pub status: Status,
    /// What it is doing, or what it wants, in plain words.
    pub detail: String,
    pub age: String,
    pub closed: bool,
    pub items: Vec<Item>,
    pub arts: Vec<&'static str>,
    pub diff: Vec<FileDiff>,
    pub terminal: Vec<(Tone, String)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Quiet,
    Good,
    Command,
}

#[derive(Clone)]
pub struct FileDiff {
    pub path: String,
    pub added: usize,
    pub removed: usize,
    pub lines: Vec<DiffLine>,
}

#[derive(Clone)]
pub struct DiffLine {
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub kind: Line,
    pub text: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Line {
    Context,
    Added,
    Removed,
    Hunk,
}

pub fn pricing(v: usize) -> Rc<[u8]> {
    let s: &[u8] = match v {
        1 => include_bytes!("../fixtures/pricing-v1.html"),
        2 => include_bytes!("../fixtures/pricing-v2.html"),
        3 => include_bytes!("../fixtures/pricing-v3.html"),
        _ => include_bytes!("../fixtures/pricing-v4.html"),
    };
    s.into()
}

pub fn artifacts() -> Vec<Artifact> {
    vec![
        Artifact {
            id: "pricing",
            kind: Kind::Html,
            title: "Pricing page",
            path: "site/pricing.html",
            versions: vec![pricing(1), pricing(2)],
        },
        Artifact {
            id: "plan",
            kind: Kind::Doc,
            title: "Moving invoices to the new ledger",
            path: "docs/ledger-migration.md",
            versions: vec![
                include_bytes!("../fixtures/migration-plan.md")
                    .as_slice()
                    .into(),
            ],
        },
        Artifact {
            id: "settings-shot",
            kind: Kind::Image,
            title: "Settings, on iPhone 17",
            path: "screenshots/settings.png",
            versions: vec![include_bytes!("../fixtures/settings.png").as_slice().into()],
        },
    ]
}

#[allow(clippy::too_many_arguments)]
fn s(
    id: &str,
    title: &str,
    project: &str,
    machine: &str,
    agent: &str,
    status: Status,
    detail: &str,
    age: &str,
) -> Session {
    Session {
        id: id.into(),
        title: title.into(),
        project: project.into(),
        machine: machine.into(),
        agent: agent.into(),
        permission: "Full access".into(),
        status,
        detail: detail.into(),
        age: age.into(),
        closed: false,
        items: Vec::new(),
        arts: Vec::new(),
        diff: Vec::new(),
        terminal: Vec::new(),
    }
}

fn tools(summary: &str, lines: &[&str]) -> Item {
    Item::Tools {
        summary: summary.into(),
        lines: lines.iter().map(|l| l.to_string()).collect(),
        open: false,
    }
}

fn text(t: &str) -> Item {
    Item::Text(t.into())
}

pub fn sessions() -> Vec<Session> {
    let mut v = Vec::new();

    let mut a = s(
        "login",
        "Rate limit the login endpoint",
        "auth-service",
        "studio",
        "Claude · Opus 5.5",
        Status::Needs,
        "Wants to run cargo test -p auth",
        "2m",
    );
    a.items = vec![
        Item::User("Add rate limiting to POST /login: 5 tries a minute per IP and per account, with a clear error.".into()),
        tools("Read 4 files, searched for “login”", &["Read src/routes/login.rs", "Read src/middleware/mod.rs", "Read Cargo.toml", "Read tests/login.rs", "Searched “login” · 23 results"]),
        text("I added a token bucket keyed by IP and by account id, in `src/middleware/limit.rs`. Both limits apply; whichever runs out first answers with **429** and a `Retry-After` header."),
        Item::Changed { files: 3, added: 96, removed: 4 },
        text("The tests need the Redis container, so I'd like to run them now."),
        Item::Approval {
            command: "cargo test -p auth".into(),
            why: "Runs the auth tests against the local Redis container.".into(),
        },
    ];
    a.diff = vec![
        limit_diff(),
        FileDiff {
            path: "src/routes/login.rs".into(),
            added: 6,
            removed: 3,
            lines: vec![
                hunk("@@ -12,9 +12,12 @@ pub fn routes() -> Router {"),
                ctx(12, 12, "pub fn routes() -> Router {"),
                ctx(13, 13, "    Router::new()"),
                del(14, "        .route(\"/login\", post(login))"),
                add(14, "        .route("),
                add(15, "            \"/login\","),
                add(
                    16,
                    "            post(login).layer(RateLimit::per_ip(5).and_per_account(5)),",
                ),
                add(17, "        )"),
                ctx(15, 18, "        .route(\"/logout\", post(logout))"),
            ],
        },
        FileDiff {
            path: "tests/login.rs".into(),
            added: 22,
            removed: 1,
            lines: vec![
                hunk("@@ -40,3 +40,24 @@"),
                add(41, "#[tokio::test]"),
                add(42, "async fn sixth_try_in_a_minute_is_refused() {"),
                add(43, "    let app = test_app().await;"),
                add(
                    44,
                    "    for _ in 0..5 { app.login(\"ana\", \"wrong\").await; }",
                ),
                add(45, "    let res = app.login(\"ana\", \"wrong\").await;"),
                add(46, "    assert_eq!(res.status(), 429);"),
                add(
                    47,
                    "    assert!(res.headers().contains_key(\"retry-after\"));",
                ),
                add(48, "}"),
            ],
        },
    ];
    a.terminal = vec![
        (Tone::Command, "$ cargo build -p auth".into()),
        (
            Tone::Quiet,
            "   Compiling auth v0.4.0 (/srv/auth-service)".into(),
        ),
        (Tone::Good, "    Finished `dev` profile in 6.42s".into()),
    ];
    v.push(a);

    let mut b = s(
        "pick",
        "Choose a layout for the changelog",
        "marketing-site",
        "laptop",
        "Codex · GPT-5.5",
        Status::Needs,
        "Asks which of two layouts to keep",
        "9m",
    );
    b.items = vec![
        Item::User("Redo the changelog page so it's easier to scan.".into()),
        tools(
            "Read 6 files",
            &[
                "Read site/changelog.html",
                "Read site/styles.css",
                "Read content/changelog/*.md · 4 files",
            ],
        ),
        text(
            "I made two layouts. **Timeline** puts every release on a dated line, with the newest at the top. **Cards** groups releases by month in a grid.",
        ),
        Item::Question {
            text: "Which one should I keep?".into(),
            options: vec![
                "Timeline".into(),
                "Cards".into(),
                "Show me both again".into(),
            ],
        },
    ];
    v.push(b);

    let mut c = s(
        "pricing",
        "New pricing page",
        "marketing-site",
        "laptop",
        "Claude · Opus 5.5",
        Status::Done,
        "Pricing page v2 is ready to review",
        "14m",
    );
    c.arts = vec!["pricing"];
    c.items = vec![
        Item::User("Make a pricing page with three plans: Starter free, Pro $12, Team $29. Clean, no stock photos.".into()),
        tools("Read 3 files", &["Read site/index.html", "Read site/styles.css", "Read content/plans.json"]),
        text("Here's a first pass: three plans side by side, using the site's existing type and colours."),
        Item::Made { art: "pricing", version: 1 },
        Item::User("Add a monthly/yearly toggle and make Pro stand out.".into()),
        tools("Edited site/pricing.html", &["Edited site/pricing.html · +38 −2"]),
        Item::Made { art: "pricing", version: 2 },
        text("Pro now has a border and a “Most popular” tag. The toggle switches every price between monthly and yearly."),
    ];
    c.diff = vec![FileDiff {
        path: "site/pricing.html".into(),
        added: 41,
        removed: 3,
        lines: vec![
            hunk("@@ -88,6 +88,16 @@"),
            ctx(88, 88, "<!--plans-->"),
            del(89, "  formatPrices(document.querySelectorAll('.price'));"),
            add(
                89,
                "  const buttons = document.querySelectorAll('.toggle button');",
            ),
            add(
                90,
                "  buttons.forEach(b => b.addEventListener('click', () => {",
            ),
            add(
                91,
                "    buttons.forEach(x => x.classList.toggle('on', x === b));",
            ),
            add(92, "    document.querySelectorAll('.price').forEach(p => {"),
            add(93, "      const v = p.dataset[b.dataset.p];"),
            add(94, "      p.firstChild.textContent = '$' + v;"),
            add(95, "    });"),
            add(96, "  }));"),
            ctx(90, 97, "</script>"),
        ],
    }];
    c.terminal = vec![
        (Tone::Command, "$ npx serve site -l 5173".into()),
        (Tone::Good, "   Serving! http://localhost:5173".into()),
        (Tone::Quiet, " HTTP  GET /pricing.html 200".into()),
    ];
    v.push(c);

    let mut d = s(
        "ledger",
        "Plan the invoice migration",
        "billing",
        "devbox",
        "Codex · GPT-5.5",
        Status::Done,
        "Wrote a four-step plan · docs/ledger-migration.md",
        "1h",
    );
    d.arts = vec!["plan"];
    d.items = vec![
        Item::User("Write a plan for moving invoices to the ledger table. Nothing changes until I sign off.".into()),
        tools("Read 11 files, ran 2 queries", &["Read migrations/*.sql · 7 files", "Read src/invoices/*.rs · 4 files", "Ran SELECT count(*) FROM invoices", "Ran SELECT count(*) FROM ledger_entries"]),
        text("The plan is four steps, each one safe to roll back: dual read, single write behind a flag, an overnight backfill, then retiring the old table."),
        Item::Made { art: "plan", version: 1 },
        text("Two things are still open: a dry run of the backfill on staging, and sign-off from finance."),
    ];
    v.push(d);

    let mut e = s(
        "flaky",
        "Fix the flaky upload test",
        "dent",
        "studio",
        "Claude · Opus 5.5",
        Status::Done,
        "Fixed · tests pass 50 times in a row",
        "2h",
    );
    e.items = vec![
        Item::User(
            "upload_resumes_after_disconnect fails about one run in ten. Find out why.".into(),
        ),
        tools(
            "Ran the test 50 times, read 3 files",
            &[
                "Ran cargo test upload_resumes · 50 runs, 6 failed",
                "Read crates/daemon/src/upload.rs",
                "Read crates/daemon/tests/upload.rs",
            ],
        ),
        text(
            "The test closed the socket before the server had acknowledged the first chunk, so sometimes there was nothing to resume. It now waits for the acknowledgement.",
        ),
        Item::Changed {
            files: 1,
            added: 4,
            removed: 1,
        },
        Item::Pr {
            number: 418,
            title: "Wait for the first ack before dropping the connection".into(),
            checks: "All checks passed".into(),
        },
    ];
    v.push(e);

    let mut f = s(
        "swiftui",
        "Port the settings screen to SwiftUI",
        "ios-app",
        "mac-mini",
        "Claude · Sonnet 5.5",
        Status::Working,
        "Building for the simulator",
        "now",
    );
    f.arts = vec!["settings-shot"];
    f.items = vec![
        Item::User(
            "Port SettingsViewController to SwiftUI with the same sections. Use stock components."
                .into(),
        ),
        tools(
            "Read 5 files, wrote 2",
            &[
                "Read Settings/SettingsViewController.swift",
                "Wrote Settings/SettingsView.swift",
                "Wrote Settings/MachineRow.swift",
            ],
        ),
        text(
            "The first build runs. Here it is on the simulator; I'm fixing the footer spacing next.",
        ),
        Item::Made {
            art: "settings-shot",
            version: 1,
        },
    ];
    v.push(f);

    let g = s(
        "deps",
        "Weekly dependency bumps",
        "auth-service",
        "devbox",
        "Codex · GPT-5.5",
        Status::Working,
        "Updating 7 crates, running tests",
        "4m",
    );
    v.push(g);

    let mut h = s(
        "readme",
        "Tighten the README install steps",
        "dent",
        "laptop",
        "Claude · Opus 5.5",
        Status::Done,
        "Merged #409",
        "Yesterday",
    );
    h.closed = true;
    v.push(h);
    let mut i = s(
        "onboard",
        "Onboarding email copy",
        "marketing-site",
        "laptop",
        "Claude · Opus 5.5",
        Status::Done,
        "Three drafts written",
        "Mon",
    );
    i.closed = true;
    v.push(i);
    v
}

fn limit_diff() -> FileDiff {
    FileDiff {
        path: "src/middleware/limit.rs".into(),
        added: 68,
        removed: 0,
        lines: vec![
            hunk("@@ -0,0 +1,68 @@"),
            add(
                1,
                "//! Token buckets per IP and per account for the login route.",
            ),
            add(2, ""),
            add(3, "use std::time::{Duration, Instant};"),
            add(4, ""),
            add(5, "pub struct RateLimit {"),
            add(6, "    per_ip: u32,"),
            add(7, "    per_account: Option<u32>,"),
            add(8, "    window: Duration,"),
            add(9, "}"),
            add(10, ""),
            add(11, "impl RateLimit {"),
            add(12, "    pub fn per_ip(n: u32) -> Self {"),
            add(
                13,
                "        Self { per_ip: n, per_account: None, window: Duration::from_secs(60) }",
            ),
            add(14, "    }"),
            add(15, ""),
            add(16, "    pub fn and_per_account(mut self, n: u32) -> Self {"),
            add(17, "        self.per_account = Some(n);"),
            add(18, "        self"),
            add(19, "    }"),
            add(20, "}"),
        ],
    }
}

fn hunk(t: &str) -> DiffLine {
    DiffLine {
        old: None,
        new: None,
        kind: Line::Hunk,
        text: t.into(),
    }
}
fn ctx(o: u32, n: u32, t: &str) -> DiffLine {
    DiffLine {
        old: Some(o),
        new: Some(n),
        kind: Line::Context,
        text: t.into(),
    }
}
fn add(n: u32, t: &str) -> DiffLine {
    DiffLine {
        old: None,
        new: Some(n),
        kind: Line::Added,
        text: t.into(),
    }
}
fn del(o: u32, t: &str) -> DiffLine {
    DiffLine {
        old: Some(o),
        new: None,
        kind: Line::Removed,
        text: t.into(),
    }
}

pub const PROJECTS: &[(&str, &[&str])] = &[
    (
        "marketing-site",
        &[
            "pricing",
            "page",
            "landing",
            "changelog",
            "site",
            "copy",
            "email",
            "seo",
        ],
    ),
    (
        "auth-service",
        &[
            "login", "auth", "password", "token", "session", "rate", "oauth", "sso",
        ],
    ),
    (
        "billing",
        &[
            "invoice", "billing", "ledger", "stripe", "refund", "payment", "tax",
        ],
    ),
    (
        "ios-app",
        &[
            "ios",
            "iphone",
            "swift",
            "swiftui",
            "simulator",
            "widget",
            "app store",
        ],
    ),
    (
        "dent",
        &["dent", "daemon", "gpui", "desktop", "event log", "upload"],
    ),
];

/// The project a new session probably belongs to, from what has been typed.
pub fn guess_project(text: &str) -> Option<&'static str> {
    let t = text.to_lowercase();
    PROJECTS
        .iter()
        .map(|(p, words)| (p, words.iter().filter(|w| t.contains(*w)).count()))
        .filter(|(_, n)| *n > 0)
        .max_by_key(|(_, n)| *n)
        .map(|(p, _)| *p)
}

pub struct Machine {
    pub name: &'static str,
    pub running: usize,
    pub online: bool,
}

pub const MACHINES: &[Machine] = &[
    Machine {
        name: "studio",
        running: 2,
        online: true,
    },
    Machine {
        name: "mac-mini",
        running: 1,
        online: true,
    },
    Machine {
        name: "devbox",
        running: 3,
        online: true,
    },
    Machine {
        name: "laptop",
        running: 0,
        online: true,
    },
];

pub const MODELS: &[&str] = &[
    "Claude · Opus 5.5",
    "Claude · Sonnet 5.5",
    "Codex · GPT-5.5",
    "Codex · GPT-5.5 mini",
];
pub const PERMISSIONS: &[(&str, &str)] = &[
    ("Full access", "Runs anything without asking"),
    ("Auto", "Asks only for risky commands"),
    ("Ask", "Asks before every edit and command"),
    ("Plan first", "Writes a plan for you to approve"),
];
pub const WORKTREES: &[(&str, &str)] = &[
    ("New worktree", "Its own branch and folder"),
    ("Main checkout", "Works in the repo as it is"),
];
