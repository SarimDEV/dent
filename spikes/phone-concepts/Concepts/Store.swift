import SwiftUI

/// The prototype's state, standing in for the client core: sessions, their
/// transcripts, and what the fake agents do over time. Every change that the
/// list shows goes through `withAnimation`, so rows move instead of jumping.
@MainActor @Observable
final class Store {
    var sessions: [Session]
    var transcripts: [String: [Entry]] = [:]
    /// The reply currently streaming, per session: words with when each arrived.
    var streams: [String: [Word]] = [:]
    /// When each session started its current stretch of work, for its timer.
    var workingSince: [String: Date] = [:]
    /// When each session last finished, so its square can pop as it lands in Done.
    var finishedAt: [String: Date] = [:]
    var toast: Toast?
    /// An unsent new-session message survives cancelling the sheet.
    var composeDraft = ""
    /// Bumped on each event that has a haptic; Root turns them into feedback.
    var closedCount = 0
    var allowedCount = 0
    var stoppedCount = 0
    /// Sessions you stopped, so Resume stays available after the toast is gone.
    var stopped: Set<String> = []

    /// The fake agent's work per session, so Stop can really stop it.
    private var tasks: [String: Task<Void, Never>] = [:]
    /// Messages sent while the agent works wait here and run in order.
    private var queues: [String: [String]] = [:]

    struct Entry: Identifiable {
        let id = UUID()
        var item: Item
    }

    struct Word: Hashable {
        let text: String
        let born: Date
    }

    struct Toast: Identifiable, Equatable {
        enum Kind { case closed, stopped }
        let id = UUID()
        let kind: Kind
        let sessionID: String
        var text: String { kind == .closed ? "Session closed" : "Session stopped" }
        var action: String { kind == .closed ? "Undo" : "Resume" }
        static func == (a: Toast, b: Toast) -> Bool { a.id == b.id }
    }

    /// How fast the fake agents move. Tests and recordings can slow it down.
    let pace: Double

    init(sessions: [Session] = Fake.sessions, pace: Double = 1) {
        self.sessions = sessions
        self.pace = pace
        for s in sessions {
            transcripts[s.id] = Self.transcript(for: s)
            if s.status == .working { workingSince[s.id] = .now.addingTimeInterval(-Double(60 + s.title.count * 7)) }
        }
    }

    // MARK: Lists

    var open: [Session] { sessions.filter { !$0.closed } }
    var needs: [Session] { open.filter { $0.status == .needs } }
    var done: [Session] { open.filter { $0.status == .done } }
    var busy: [Session] { open.filter { $0.status == .working || $0.status == .waiting } }
    var closed: [Session] { sessions.filter(\.closed) }
    var toClear: Int { needs.count + done.count }

    func session(_ id: String) -> Session? { sessions.first { $0.id == id } }

    func search(_ q: String) -> [Session] {
        let q = q.lowercased()
        return sessions.filter { "\($0.title) \($0.detail) \($0.project) \($0.machine)".lowercased().contains(q) }
    }

    private func update(_ id: String, _ change: (inout Session) -> Void) {
        guard let i = sessions.firstIndex(where: { $0.id == id }) else { return }
        withAnimation(.snappy(duration: 0.3)) { change(&sessions[i]) }
    }

    private func show(_ kind: Toast.Kind, _ id: String) {
        withAnimation(.snappy(duration: 0.25)) { toast = Toast(kind: kind, sessionID: id) }
    }

    func dismissToast(_ t: Toast) {
        guard toast == t else { return }
        withAnimation(.easeOut(duration: 0.2)) { toast = nil }
    }

    func toastAction() {
        guard let t = toast else { return }
        switch t.kind {
        case .closed: reopen(t.sessionID)
        case .stopped: resume(t.sessionID)
        }
    }

    // MARK: Closing (archive, never delete)

    /// Sessions that need you or are working can't be closed: answer or stop first.
    func canClose(_ s: Session) -> Bool { !s.closed && (s.status == .done || s.status == .idle) }

    func close(_ id: String) {
        guard let s = session(id), canClose(s) else { return }
        update(id) { $0.closed = true }
        closedCount += 1
        show(.closed, id)
    }

    func reopen(_ id: String) {
        update(id) { $0.closed = false }
        if let t = toast, t.sessionID == id, t.kind == .closed { dismissToast(t) }
    }

    // MARK: Approvals

    func allow(_ id: String) { answer(id, allowed: true) }
    func deny(_ id: String) { answer(id, allowed: false) }

    private func answer(_ id: String, allowed: Bool) {
        guard var t = transcripts[id], let i = t.firstIndex(where: { if case .approval = $0.item { true } else { false } }),
              case .approval(let command, _) = t[i].item else { return }
        if allowed { allowedCount += 1 }
        withAnimation(.smooth(duration: 0.24)) {
            t[i].item = .resolved(allowed ? "Allowed \(command)" : "Denied \(command)")
            transcripts[id] = t
        }
        beginWork(id, detail: allowed ? "Running \(command)" : "Finding another way")
        let reply = allowed
            ? "All 212 tests pass, including the new one that cuts the link mid-frame. The reply now arrives whole after a reconnect, so this is ready for review."
            : "Understood, I won't run it. The change is small and covered by the existing tests, so I've left it for you to run when you're ready."
        run(id, reply: reply, after: 1.6, done: allowed ? "212 tests pass · ready for review" : "Left for you to test")
    }

    // MARK: Stopping (with Resume)

    func stop(_ id: String) {
        tasks[id]?.cancel()
        tasks[id] = nil
        queues[id] = nil
        withAnimation(.snappy(duration: 0.2)) {
            if let words = streams[id], !words.isEmpty {
                transcripts[id, default: []].append(Entry(item: .reply(words.map(\.text).joined(separator: " ") + " …")))
            }
            streams[id] = nil
            transcripts[id] = transcripts[id]?.map { e in
                if case .queued(let q) = e.item { return Entry(item: .resolved("Not sent: \(q)")) }
                return e
            }
            transcripts[id, default: []].append(Entry(item: .resolved("Stopped by you")))
        }
        stoppedCount += 1
        stopped.insert(id)
        update(id) { $0.status = .done; $0.detail = "Stopped by you"; $0.age = "now" }
        show(.stopped, id)
    }

    func resume(_ id: String) {
        stopped.remove(id)
        if let t = toast, t.sessionID == id { dismissToast(t) }
        withAnimation(.snappy(duration: 0.2)) { transcripts[id, default: []].append(Entry(item: .resolved("Resumed"))) }
        beginWork(id, detail: "Picking up where it stopped")
        run(id, reply: "Picking up where I stopped. Nothing was lost; I'll finish the change and run the tests again.", after: 1.2, done: "Done · ready for review")
    }

    // MARK: Sending and starting

    /// Starts a session from the first message, like the desktop's new-session view.
    @discardableResult
    func start(_ text: String, project: String?) -> String {
        let id = "n\(sessions.count + 1)"
        let words = text.split(separator: " ").prefix(6).joined(separator: " ")
        let title = words.count < text.count ? words + "…" : words
        let s = Session(id: id, title: title, project: project ?? "no project", machine: "studio", agent: "claude",
                        status: .working, detail: "Reading the code", age: "now")
        transcripts[id] = [Entry(item: .user(text))]
        withAnimation(.snappy(duration: 0.3)) { sessions.insert(s, at: 0) }
        workingSince[id] = .now
        run(id, reply: "The line is a navigation subtitle, which never wraps. I'll move usage into a small header row that wraps at large sizes and keep the subtitle for the count.",
            after: 2.0, done: "Changed 1 file · ready for review")
        return id
    }

    /// Sends a message. While the agent works it waits in a queue, shown as
    /// "Queued", and runs when the current reply finishes.
    func send(_ text: String, to id: String) {
        if tasks[id] != nil {
            queues[id, default: []].append(text)
            withAnimation(.snappy(duration: 0.25)) { transcripts[id, default: []].append(Entry(item: .queued(text))) }
            return
        }
        stopped.remove(id)
        withAnimation(.snappy(duration: 0.25)) { transcripts[id, default: []].append(Entry(item: .user(text))) }
        beginWork(id, detail: "Working on your message")
        run(id, reply: Self.onIt, after: 1.2, done: "Done · ready for review")
    }

    private func beginWork(_ id: String, detail: String) {
        workingSince[id] = .now
        update(id) { $0.status = .working; $0.detail = detail; $0.age = "now" }
    }

    private func run(_ id: String, reply: String, after delay: Double, done detail: String) {
        tasks[id] = Task { [weak self] in
            await self?.stream(reply, into: id, after: delay, done: detail)
        }
    }

    /// Streams a reply word by word, then either runs the next queued message
    /// or marks the session done. Cancelling (Stop) ends it where it is.
    private func stream(_ text: String, into id: String, after delay: Double, done detail: String) async {
        try? await Task.sleep(for: .seconds(delay * pace))
        if Task.isCancelled { return }
        streams[id] = []
        for w in text.split(separator: " ") {
            if Task.isCancelled { return }
            withAnimation(.linear(duration: 0.1)) { streams[id, default: []].append(Word(text: String(w), born: .now)) }
            try? await Task.sleep(for: .milliseconds(Int(55 * pace)))
        }
        if Task.isCancelled { return }
        // The finished text replaces the stream in place: same words, no movement.
        var t = Transaction()
        t.disablesAnimations = true
        withTransaction(t) {
            // The reply goes before any queued messages, matching what was on screen.
            var list = transcripts[id, default: []]
            let at = list.firstIndex { $0.item.isQueued } ?? list.endIndex
            list.insert(Entry(item: .reply(text)), at: at)
            transcripts[id] = list
            streams[id] = nil
        }
        tasks[id] = nil
        if var q = queues[id], !q.isEmpty {
            let next = q.removeFirst()
            queues[id] = q
            withAnimation(.snappy(duration: 0.25)) {
                if let i = transcripts[id]?.firstIndex(where: { if case .queued(next) = $0.item { true } else { false } }) {
                    transcripts[id]?[i].item = .user(next)
                }
            }
            beginWork(id, detail: "Working on your next message")
            run(id, reply: "Done with that. Now the next one: I'll start on it and report back here.", after: 1.0, done: "Done · ready for review")
            return
        }
        finishedAt[id] = .now
        update(id) { $0.status = .done; $0.detail = detail; $0.age = "now" }
    }

    // MARK: Fixed states for stills

    enum Preset { case working, pr, zero, many }

    func prepare(_ p: Preset) {
        switch p {
        case .working, .pr:
            guard var t = transcripts["s1"], let i = t.firstIndex(where: { if case .approval = $0.item { true } else { false } }) else { return }
            t[i].item = .resolved("Allowed cargo test -p spike-core")
            if p == .pr {
                t.append(Entry(item: .text("All 212 tests pass. I opened a pull request.")))
                t.append(Entry(item: .pr(number: 142, title: "Resume from the last event received")))
            }
            transcripts["s1"] = t
            if let k = sessions.firstIndex(where: { $0.id == "s1" }) {
                sessions[k].status = p == .pr ? .done : .working
                sessions[k].detail = p == .pr ? "PR #142 · CI failed" : "Running cargo test -p spike-core"
            }
            workingSince["s1"] = .now.addingTimeInterval(-72)
        case .zero:
            for k in sessions.indices where sessions[k].status == .needs || sessions[k].status == .done {
                sessions[k].status = .done
                sessions[k].closed = true
            }
        case .many:
            let long = "Rewrite the reconnect backoff so a laptop waking from sleep on a captive-portal network doesn't hammer the daemon"
            for n in 0..<36 {
                let st: [Status] = [.needs, .done, .done, .working]
                sessions.append(Session(id: "m\(n)", title: n % 5 == 0 ? long : "Session \(n + 1): tidy the \(["parser", "daemon", "docs", "tests"][n % 4])",
                                        project: Fake.projects[n % 3], machine: ["studio", "mac-mini"][n % 2], agent: n % 2 == 0 ? "claude" : "codex",
                                        status: st[n % 4], detail: n % 7 == 0 ? "A much longer detail line that explains in plain words what the agent is doing right now and why" : (n % 6 == 0 ? "Changed 1 file" : "Changed \(n % 6 + 1) files"),
                                        age: "\(n + 2)m"))
                transcripts["m\(n)"] = Self.transcript(for: sessions.last!)
            }
        }
    }

    static let onIt = "On it. I'll read the files involved first, make the change in one place rather than patching each caller, run the tests that cover it, and then tell you what changed and anything I wasn't sure about, so you can check it before it goes any further."

    // MARK: Transcripts

    static func transcript(for s: Session) -> [Entry] {
        if s.id == "s1" { return Chat.items.map { Entry(item: $0) } }
        return [Entry(item: .user(s.title)), Entry(item: .text(s.detail + "."))]
    }
}
