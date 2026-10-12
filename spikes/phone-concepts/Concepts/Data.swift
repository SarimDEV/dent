import SwiftUI

// Fake data for the concepts: three machines, three projects, a handful of
// sessions in every state, and one chat that shows every kind of item.

struct Machine: Identifiable, Hashable {
    let id: String
    let name: String
    let status: Status
    let detail: String
    let address: String
    let via: String
}

struct Session: Identifiable, Hashable {
    let id: String
    var title: String
    var project: String
    var machine: String
    var agent: String
    var status: Status
    var detail: String
    var age: String
    /// Closed is archived: off the list, searchable, and can be reopened.
    var closed = false
}

enum Fake {
    static let machines = [
        Machine(id: "studio", name: "studio", status: .done, detail: "Online · 18 ms", address: "192.168.0.39:7480", via: "This network"),
        Machine(id: "mini", name: "mac-mini", status: .waiting, detail: "Asleep · wakes for new work", address: "mac-mini.tail3f.ts.net", via: "Tailscale"),
        Machine(id: "devbox", name: "devbox", status: .offline, detail: "Offline since 9:12", address: "10.0.4.12:7480", via: "Tailscale"),
    ]

    static let sessions = [
        Session(id: "s1", title: "Fix reply tail lost on reconnect", project: "dent", machine: "studio", agent: "claude",
                status: .needs, detail: "Wants to run cargo test -p spike-core", age: "now"),
        Session(id: "s2", title: "Pricing page copy", project: "website", machine: "studio", agent: "codex",
                status: .needs, detail: "Asks: keep the free tier wording?", age: "4m"),
        Session(id: "s3", title: "Batch events during catch-up", project: "dent", machine: "studio", agent: "claude",
                status: .working, detail: "Editing spikes/link/src/daemon/push.rs", age: "1m"),
        Session(id: "s4", title: "Rotate the staging certificates", project: "infra", machine: "mac-mini", agent: "codex",
                status: .working, detail: "Running terraform plan", age: "6m"),
        Session(id: "s5", title: "Dark mode for the docs viewer", project: "dent", machine: "studio", agent: "claude",
                status: .done, detail: "Changed 4 files · tests pass", age: "22m"),
        Session(id: "s6", title: "Blog post: one core, three places", project: "website", machine: "studio", agent: "claude",
                status: .done, detail: "Draft ready in posts/one-core.md", age: "1h"),
        Session(id: "s7", title: "Upgrade the base image", project: "infra", machine: "devbox", agent: "codex",
                status: .waiting, detail: "Paused at the usage limit · resumes 3:10 pm", age: "2h"),
        Session(id: "s8", title: "Nightly dependency check", project: "dent", machine: "mac-mini", agent: "claude",
                status: .done, detail: "Automation · 3 updates available, PR ready", age: "6:00"),
        Session(id: "s9", title: "Goal: docs site builds clean", project: "website", machine: "studio", agent: "codex",
                status: .done, detail: "Goal finished after 4 runs", age: "Yest."),
    ]

    static var needs: [Session] { sessions.filter { $0.status == .needs } }
    static var working: [Session] { sessions.filter { $0.status == .working } }
    /// Finished and waiting for you to steer or close: sessions, automation results, goals.
    static var done: [Session] { sessions.filter { $0.status == .done } }
    /// Nothing to do: running, or paused until a limit resets.
    static var busy: [Session] { sessions.filter { $0.status == .working || $0.status == .waiting } }
    static let projects = ["dent", "website", "infra"]

    static let chat = sessions[0]
}

/// One chat, every kind of item.
enum Item: Identifiable {
    case user(String)
    case text(String)
    case list([String])
    case code(lang: String, text: String)
    case tools(summary: String, lines: [String])
    case edit(file: String, added: Int, removed: Int)
    case approval(command: String, why: String)
    case pr(number: Int, title: String)
    case working(String)
    /// An approval after you answered it, folded to one line.
    case resolved(String)
    /// A reply that streams in, word by word.
    case reply(String)
    /// A message sent while the agent works, waiting its turn.
    case queued(String)

    var isReply: Bool { if case .reply = self { true } else { false } }
    var isQueued: Bool { if case .queued = self { true } else { false } }

    var id: String {
        switch self {
        case .user(let s), .text(let s): s
        case .list(let l): l.joined()
        case .code(_, let t): t
        case .tools(let s, _): s
        case .edit(let f, _, _): f
        case .approval(let c, _): c
        case .working(let w): w
        case .pr(let n, _): "pr\(n)"
        case .resolved(let r): "resolved-" + r
        case .reply(let r): "reply-" + r
        case .queued(let q): "queued-" + q
        }
    }
}

enum Chat {
    static let items: [Item] = [
        .user("The phone drops the last few lines of a reply when the link flaps. Find out why and fix it."),
        .tools(summary: "Read 4 files, searched twice", lines: [
            "read  spikes/core/src/link.rs",
            "read  spikes/core/src/chat.rs",
            "grep  \"resume\" spikes/",
            "read  spikes/link/src/daemon/session.rs",
            "grep  \"after:\" spikes/core",
            "read  spikes/core/src/proto.rs",
        ]),
        .text("Found it. On reconnect the phone asks to resume from the last event it **rendered**, not the last one it **received**. Anything that arrived in the final frame before the drop is skipped."),
        .list([
            "Track the highest event received, before rendering",
            "Resume from that, not from the rendered count",
            "Add a test that cuts the link mid-frame",
        ]),
        .code(lang: "rust", text: "fn resume(&self) -> Up {\n    Up::Follow { after: self.received }\n}"),
        .edit(file: "spikes/core/src/link.rs", added: 12, removed: 3),
        .approval(command: "cargo test -p spike-core", why: "Runs the core's tests, including the new one that cuts the link mid-frame."),
    ]

    static let doc = """
    The phone drops the tail of a reply when the link flaps, because it resumes from what it rendered rather than what it received.
    """
}
