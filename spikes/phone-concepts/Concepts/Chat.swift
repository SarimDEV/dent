import SwiftUI

/// A session. The reply reads like a page; chrome is the system's own bars.
struct ChatScreen: View {
    enum Style { case bubbles, transcript }
    let id: String
    var style: Style = .bubbles
    var fromTop = false
    var toolsOpen = false
    var offline = false
    @Environment(Store.self) private var store
    @Environment(\.look) private var look
    @Environment(\.dismiss) private var dismiss
    @State private var draft = ""

    var body: some View {
        let s = store.session(id)
        let all = store.transcripts[id] ?? []
        // Queued messages stay last, after whatever is streaming, until their turn.
        let entries = all.filter { !$0.item.isQueued }
        let waiting = all.filter(\.item.isQueued)
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                ForEach(Array(entries.enumerated()), id: \.element.id) { i, e in
                    if style == .transcript && i == 1 { Speaker(name: s?.agent ?? "claude") }
                    ItemView(item: e.item, style: style, toolsOpen: toolsOpen, sessionID: id)
                        // The finished reply takes the stream's place without moving.
                        .transition(e.item.isReply ? .identity : .arrive)
                }
                if let words = store.streams[id] {
                    StreamingText(words: words).transition(.identity)
                } else if s?.status == .working {
                    WorkingLine(since: store.workingSince[id] ?? .now).transition(.opacity)
                }
                ForEach(waiting) { e in
                    ItemView(item: e.item, style: style, toolsOpen: toolsOpen, sessionID: id).transition(.arrive)
                }
            }
            .padding(.horizontal, 20)
            .padding(.top, 12)
            .padding(.bottom, 24)
        }
        .defaultScrollAnchor(fromTop ? .top : .bottom)
        .defaultScrollAnchor(.bottom, for: .sizeChanges)
        .scrollDismissesKeyboard(.interactively)
        .background(look.page)
        .navigationTitle(s?.title ?? "")
        .navigationBarTitleDisplayMode(.inline)
        .navigationSubtitle(subtitle(s))
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Menu {
                    NavigationLink(value: Route.changes) { Label("Changed Files", systemImage: "doc.on.doc") }
                    NavigationLink(value: Route.doc) { Label("Plan", systemImage: "doc.richtext") }
                    Button {} label: { Label("Open Preview", systemImage: "safari") }
                    Divider()
                    Button {} label: { Label("Mute", systemImage: "bell.slash") }
                    if let s {
                        Divider()
                        if s.closed {
                            Button { store.reopen(id) } label: { Label("Reopen", systemImage: "arrow.uturn.backward") }
                        } else if s.status == .working {
                            Button(role: .destructive) { store.stop(id) } label: { Label("Stop", systemImage: "stop.fill") }
                        } else {
                            if store.stopped.contains(id) {
                                Button { store.resume(id) } label: { Label("Resume", systemImage: "play.fill") }
                            }
                            if store.canClose(s) {
                                Button { dismiss(); store.close(id) } label: { Label("Close", systemImage: "archivebox") }
                            }
                        }
                    }
                } label: {
                    Label("More", systemImage: "ellipsis")
                }
                .accessibilityIdentifier("chat-menu")
            }
        }
        .safeAreaBar(edge: .bottom) {
            VStack(spacing: 8) {
                if offline {
                    HStack(spacing: 8) {
                        Mark(status: .offline)
                        Text("\(s?.machine ?? "studio") is offline. Messages wait here and send when it's back.")
                            .font(look.ui(.footnote)).foregroundStyle(look.quiet)
                        Spacer(minLength: 0)
                    }
                    .padding(.horizontal, 20)
                }
                if let t = store.toast, t.sessionID == id, t.kind == .stopped {
                    UndoToast(toast: t).transition(.move(edge: .bottom).combined(with: .opacity))
                }
                if s?.closed == true {
                    // A closed session reads like any other; reopening is one tap.
                    Button { store.reopen(id) } label: {
                        Label("Reopen Session", systemImage: "arrow.uturn.backward").frame(maxWidth: .infinity, minHeight: 44)
                    }
                    .buttonStyle(.glass)
                    .font(look.ui(.body, .medium))
                    .padding(.horizontal, 16).padding(.bottom, 4)
                    .accessibilityIdentifier("reopen")
                } else {
                    Composer(draft: $draft, working: s?.status == .working, machine: s?.machine ?? "studio",
                             send: { store.send($0, to: id) }, stop: { store.stop(id) })
                }
            }
        }
    }

    func subtitle(_ s: Session?) -> String {
        guard let s else { return "" }
        if s.closed { return "\(s.agent) · \(s.machine) · Closed" }
        if offline { return "\(s.agent) · \(s.machine) · Offline" }
        let extra = store.transcripts[id]?.contains { if case .pr = $0.item { true } else { false } } == true ? " · CI failed" : ""
        return "\(s.agent) · \(s.machine) · \(s.status.label)\(extra)"
    }
}

extension AnyTransition {
    /// "Arrive": a 6 pt rise and a fade, for new chat rows.
    static var arrive: AnyTransition {
        .asymmetric(insertion: .opacity.combined(with: .offset(y: 6)), removal: .opacity)
    }
}

/// While the agent works and nothing is streaming yet.
struct WorkingLine: View {
    let since: Date
    @Environment(\.look) private var look
    var body: some View {
        TimelineView(.periodic(from: since, by: 1)) { ctx in
            let secs = max(0, Int(ctx.date.timeIntervalSince(since)))
            HStack(spacing: 8) {
                Mark(status: .working)
                Text("Working · \(secs / 60):\(String(format: "%02d", secs % 60))")
                    .font(look.ui(.subheadline)).foregroundStyle(look.quiet).monospacedDigit()
                    .contentTransition(.numericText(countsDown: false))
            }
        }
        .accessibilityIdentifier("working")
    }
}

/// "Stream": a reply arriving word by word, each word fading in over 180 ms.
/// Words are appended, never re-laid out. With Reduce Motion they just appear.
struct StreamingText: View {
    let words: [Store.Word]
    @Environment(\.look) private var look
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        TimelineView(.animation(paused: reduceMotion)) { ctx in
            text
                .font(look.reading).lineSpacing(look.readingSpacing).foregroundStyle(look.ink)
                .textRenderer(FadeIn(now: ctx.date, enabled: !reduceMotion))
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .accessibilityIdentifier("streaming")
    }

    var text: Text {
        var t = Text("")
        for (i, w) in words.enumerated() {
            let word = Text(i == 0 ? w.text : " " + w.text).customAttribute(Born(date: w.born))
            t = Text("\(t)\(word)")
        }
        return t
    }
}

struct Born: TextAttribute { let date: Date }

struct FadeIn: TextRenderer {
    let now: Date
    let enabled: Bool
    func draw(layout: Text.Layout, in ctx: inout GraphicsContext) {
        for line in layout {
            for run in line {
                var c = ctx
                if enabled, let born = run[Born.self]?.date {
                    let p = min(1, max(0, now.timeIntervalSince(born) / 0.18))
                    c.opacity = p
                }
                c.draw(run)
            }
        }
    }
}

struct Speaker: View {
    let name: String
    @Environment(\.look) private var look
    var body: some View {
        Text(name).font(look.ui(.footnote, .semibold)).foregroundStyle(look.quiet)
    }
}

struct ItemView: View {
    /// One glyph per kind of status line, so a stop never reads as a denial.
    static func symbol(for line: String) -> String {
        if line.hasPrefix("Allowed") { return "checkmark.circle" }
        if line.hasPrefix("Denied") { return "xmark.circle" }
        if line.hasPrefix("Stopped") { return "stop.circle" }
        if line.hasPrefix("Resumed") { return "play.circle" }
        if line.hasPrefix("Not sent") { return "arrow.uturn.backward.circle" }
        return "circle"
    }
    let item: Item
    let style: ChatScreen.Style
    let toolsOpen: Bool
    var sessionID = ""
    @Environment(\.look) private var look

    var body: some View {
        switch item {
        case .user(let text):
            if style == .bubbles {
                HStack {
                    Spacer(minLength: 48)
                    Text(text)
                        .font(look.reading).lineSpacing(look.readingSpacing - 2)
                        .foregroundStyle(look.ink)
                        .padding(.horizontal, 14).padding(.vertical, 10)
                        .background(look.bubble, in: .rect(cornerRadius: 20))
                }
            } else {
                VStack(alignment: .leading, spacing: 6) {
                    Speaker(name: "You")
                    Text(text).font(look.reading).lineSpacing(look.readingSpacing).foregroundStyle(look.ink)
                }
                .padding(.bottom, 4)
            }
        case .queued(let text):
            VStack(alignment: .trailing, spacing: 4) {
                HStack {
                    Spacer(minLength: 48)
                    Text(text)
                        .font(look.reading).lineSpacing(look.readingSpacing - 2)
                        .foregroundStyle(look.quiet)
                        .padding(.horizontal, 14).padding(.vertical, 10)
                        .background(look.bubble.opacity(0.6), in: .rect(cornerRadius: 20))
                        .overlay(RoundedRectangle(cornerRadius: 20).strokeBorder(look.hairline, style: StrokeStyle(lineWidth: 1, dash: [4, 3])))
                }
                Text("Queued · sends when the current reply finishes").font(look.ui(.caption)).foregroundStyle(look.quiet)
            }
            .accessibilityElement(children: .combine)
            .accessibilityIdentifier("queued")
        case .text(let md), .reply(let md):
            Text((try? AttributedString(markdown: md)) ?? AttributedString(md))
                .font(look.reading).lineSpacing(look.readingSpacing).foregroundStyle(look.ink)
        case .list(let lines):
            VStack(alignment: .leading, spacing: 8) {
                ForEach(Array(lines.enumerated()), id: \.offset) { i, l in
                    HStack(alignment: .firstTextBaseline, spacing: 10) {
                        Text("\(i + 1).").foregroundStyle(look.quiet).monospacedDigit()
                        Text(l).foregroundStyle(look.ink)
                    }
                    .font(look.reading).lineSpacing(look.readingSpacing)
                }
            }
        case .code(let lang, let text):
            VStack(alignment: .leading, spacing: 8) {
                Text(lang).font(look.ui(.caption)).foregroundStyle(look.quiet)
                ScrollView(.horizontal, showsIndicators: false) {
                    Text(text).font(look.code).foregroundStyle(look.ink).lineSpacing(4).fixedSize()
                }
            }
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(look.row, in: .rect(cornerRadius: 14))
            .overlay(RoundedRectangle(cornerRadius: 14).strokeBorder(look.hairline))
        case .tools(let summary, let lines):
            Tools(summary: summary, lines: lines, open: toolsOpen)
        case .edit(let file, let added, let removed):
            NavigationLink(value: Route.changes) {
                HStack(spacing: 10) {
                    Image(systemName: "pencil.line").foregroundStyle(look.quiet)
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Edited link.rs").font(look.ui(.subheadline, .medium)).foregroundStyle(look.ink)
                        Text(file).font(look.ui(.footnote)).foregroundStyle(look.quiet).lineLimit(1).truncationMode(.middle)
                    }
                    Spacer(minLength: 8)
                    Text("+\(added) −\(removed)").font(look.ui(.footnote)).foregroundStyle(look.quiet).monospacedDigit()
                    Image(systemName: "chevron.right").font(.footnote.weight(.semibold)).foregroundStyle(look.quiet)
                }
                .padding(.horizontal, 14).frame(minHeight: 56)
                .background(look.row, in: .rect(cornerRadius: 14))
                .overlay(RoundedRectangle(cornerRadius: 14).strokeBorder(look.hairline))
            }
            .buttonStyle(.plain)
        case .approval(let command, let why):
            Approval(command: command, why: why, sessionID: sessionID)
                .transition(.asymmetric(insertion: .opacity, removal: .opacity.combined(with: .scale(scale: 0.97, anchor: .top))))
        case .resolved(let what):
            // "Fold": the answered approval collapses to one quiet line.
            HStack(spacing: 8) {
                Image(systemName: Self.symbol(for: what))
                    .foregroundStyle(look.quiet)
                Text(what).font(look.ui(.subheadline)).foregroundStyle(look.quiet)
            }
            .transition(.opacity.combined(with: .offset(y: -4)))
            .accessibilityIdentifier("resolved")
        case .pr(let n, let t):
            PullRequest(number: n, title: t)
        case .working:
            EmptyView()
        }
    }
}

/// Tool calls fold to one line, as on desktop.
struct Tools: View {
    let summary: String
    let lines: [String]
    @State var open: Bool
    @Environment(\.look) private var look
    var body: some View {
        DisclosureGroup(isExpanded: $open) {
            VStack(alignment: .leading, spacing: 6) {
                ForEach(lines, id: \.self) { Text($0).font(look.code).foregroundStyle(look.quiet).lineLimit(1) }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.top, 8)
        } label: {
            Label(summary, systemImage: "terminal")
                .font(look.ui(.subheadline)).foregroundStyle(look.quiet)
        }
        .tint(look.quiet)
    }
}

/// An approval: the one loud thing on the screen.
struct Approval: View {
    let command: String
    let why: String
    var sessionID = ""
    @Environment(Store.self) private var store
    @Environment(\.look) private var look
    var deny: some View {
        Button { store.deny(sessionID) } label: { Text("Deny").lineLimit(1).frame(maxWidth: .infinity) }
            .buttonStyle(.bordered)
            .accessibilityIdentifier("deny")
    }
    var allow: some View {
        Button { store.allow(sessionID) } label: { Text("Allow").lineLimit(1).frame(maxWidth: .infinity).foregroundStyle(look.onTint) }
            .buttonStyle(.borderedProminent)
            .accessibilityIdentifier("allow")
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 8) {
                Mark(status: .needs)
                Text("Needs you").font(look.ui(.footnote, .semibold)).foregroundStyle(look.ink)
            }
            Text("Run this command?").font(look.ui(.headline, .semibold)).foregroundStyle(look.ink)
            Text(command)
                .font(look.code).foregroundStyle(look.ink)
                .padding(.horizontal, 12).padding(.vertical, 10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(look.background, in: .rect(cornerRadius: 10))
            Text(why).font(look.ui(.subheadline)).foregroundStyle(look.quiet)
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 10) { deny; allow }
                VStack(spacing: 10) { allow; deny }
            }
            .controlSize(.large)
            .font(look.ui(.body, .semibold))
            Menu {
                Button("Allow cargo test in dent") {}
                Button("Allow all commands for 1 hour") {}
            } label: {
                Text("Always Allow…").font(look.ui(.subheadline)).frame(maxWidth: .infinity, minHeight: 44)
            }
        }
        .padding(16)
        .background(look.row, in: .rect(cornerRadius: 20))
        .overlay(RoundedRectangle(cornerRadius: 20).strokeBorder(Status.needs.color.opacity(0.6), lineWidth: 1.5))
    }
}

/// The composer floats on glass, like every other control layer.
struct Composer: View {
    @Binding var draft: String
    var working: Bool
    var machine: String
    var send: (String) -> Void = { _ in }
    var stop: () -> Void = {}
    @Environment(\.look) private var look
    @Environment(\.dynamicTypeSize) private var size
    @ScaledMetric(relativeTo: .body) private var control: CGFloat = 44
    @State private var justSent = false
    @State private var settle: Task<Void, Never>?

    var body: some View {
        GlassEffectContainer {
            HStack(alignment: .bottom, spacing: 10) {
                Menu {
                    Button {} label: { Label("Photo", systemImage: "photo") }
                    Button {} label: { Label("File", systemImage: "doc") }
                    Button {} label: { Label("Switch Agent", systemImage: "arrow.triangle.2.circlepath") }
                } label: {
                    Image(systemName: "plus").font(.body.weight(.medium)).frame(width: control, height: control)
                }
                .glassEffect(.regular.interactive(), in: .circle)
                .tint(look.ink)

                HStack(alignment: .bottom, spacing: 6) {
                    TextField(working ? "Queue a message" : "Message \(machine)", text: $draft, axis: .vertical)
                        .font(look.reading)
                        .lineLimit(size.isAccessibilitySize ? 1...3 : 1...6)
                        .padding(.leading, 16).padding(.vertical, 11)
                        .accessibilityIdentifier("composer")
                    // Send once there's text (it queues while the agent works);
                    // Stop while working with nothing typed; otherwise dictate.
                    let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
                    // Right after sending, the button stays Send (and inert) for a
                    // moment, so a double tap can't land on Stop.
                    let showStop = text.isEmpty && !justSent
                    if !text.isEmpty || working || justSent {
                        Button {
                            if !text.isEmpty {
                                send(text); draft = ""
                                justSent = true
                                settle?.cancel()
                                settle = Task { try? await Task.sleep(for: .seconds(1)); if !Task.isCancelled { justSent = false } }
                            } else if showStop { stop() }
                        } label: {
                            Image(systemName: showStop ? "stop.fill" : "arrow.up")
                                .font(.body.weight(.bold))
                                .foregroundStyle(look.onTint)
                                .frame(width: min(control - 12, 44), height: min(control - 12, 44))
                                .contentTransition(.symbolEffect(.replace))
                        }
                        .buttonStyle(.borderedProminent)
                        .buttonBorderShape(.circle)
                        .accessibilityLabel(showStop ? "Stop" : "Send")
                        .accessibilityIdentifier("send")
                        .padding(6)
                        .transition(.scale(scale: 0.6).combined(with: .opacity))
                    } else {
                        Button {} label: {
                            Image(systemName: "mic").font(.body).frame(width: min(control - 12, 44), height: min(control - 12, 44))
                        }
                        .tint(look.quiet)
                        .accessibilityLabel("Dictate")
                        .padding(6)
                        .transition(.opacity)
                    }
                }
                .animation(.snappy(duration: 0.18), value: draft.isEmpty)
                .animation(.snappy(duration: 0.18), value: working)
                .animation(.snappy(duration: 0.18), value: justSent)
                .glassEffect(.regular.interactive(), in: .rect(cornerRadius: 22))
            }
        }
        .padding(.horizontal, 16)
        .padding(.bottom, 4)
    }
}

/// Changed files, read only: diffs stay on the computer.
struct ChangesScreen: View {
    @Environment(\.look) private var look
    let files: [(String, String, Int, Int)] = [
        ("link.rs", "spikes/core/src", 12, 3),
        ("chat.rs", "spikes/core/src", 4, 1),
        ("main.rs", "spikes/core-test/src", 38, 0),
    ]
    var body: some View {
        List {
            Section {
                ForEach(files, id: \.0) { f in
                    HStack(spacing: 12) {
                        Image(systemName: "doc.text").foregroundStyle(look.quiet)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(f.0).font(look.ui(.body))
                            Text(f.1).font(look.ui(.footnote)).foregroundStyle(look.quiet)
                        }
                        Spacer()
                        Text("+\(f.2) −\(f.3)").font(look.ui(.subheadline)).foregroundStyle(look.quiet).monospacedDigit()
                    }
                    .listRowBackground(look.row)
                }
            } footer: {
                Text("3 files, +54 −4. Diffs and staging are on your computer.")
            }
        }
        .scrollContentBackground(.hidden)
        .background(look.background)
        .navigationTitle("Changed Files")
        .navigationBarTitleDisplayMode(.inline)
    }
}

/// A Markdown doc from the project, set like a page.
struct DocScreen: View {
    @Environment(\.look) private var look
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                Text("Resume from what was received").font(look.heading(.title)).foregroundStyle(look.ink)
                Text("dent · docs/plans/resume.md · updated 2m ago").font(look.ui(.footnote)).foregroundStyle(look.quiet)
                Text(Chat.doc).font(look.reading).lineSpacing(look.readingSpacing)
                Text("What changes").font(look.heading(.title3)).foregroundStyle(look.ink).padding(.top, 8)
                ForEach(["The link tracks the highest event received.", "Follow asks from that event.", "A test cuts the link mid-frame."], id: \.self) { l in
                    HStack(alignment: .firstTextBaseline, spacing: 10) {
                        Text("•").foregroundStyle(look.quiet)
                        Text(l)
                    }
                    .font(look.reading).lineSpacing(look.readingSpacing)
                }
                Text("Risks").font(look.heading(.title3)).foregroundStyle(look.ink).padding(.top, 8)
                Text("A daemon older than this change still resumes correctly, because it already sends every event after the one asked for.")
                    .font(look.reading).lineSpacing(look.readingSpacing)
            }
            .foregroundStyle(look.ink)
            .padding(.horizontal, 20).padding(.vertical, 16)
        }
        .background(look.page)
        .navigationTitle("resume.md")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                ShareLink(item: "resume.md") { Label("Share", systemImage: "square.and.arrow.up") }
            }
            ToolbarItem(placement: .bottomBar) {
                Button("Ask About This Doc") {}.font(look.ui(.body, .medium))
            }
        }
    }
}

/// The session's pull request, with CI. A failure goes back to the agent in one tap.
struct PullRequest: View {
    let number: Int
    let title: String
    @Environment(\.look) private var look
    @Environment(\.dynamicTypeSize) private var size
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                Image(systemName: "arrow.triangle.pull").foregroundStyle(look.quiet)
                VStack(alignment: .leading, spacing: 4) {
                    Text("#\(number) \(title)").font(look.ui(.subheadline, .medium)).foregroundStyle(look.ink).lineLimit(size.isAccessibilitySize ? nil : 2)
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        // A failed check is not "needs you": its own glyph, not the magenta square.
                        Image(systemName: "xmark.circle").font(look.ui(.footnote, .semibold)).foregroundStyle(look.ink)
                        Text("CI failed: test (macos) · 2 of 3 checks passed").font(look.ui(.footnote)).foregroundStyle(look.quiet)
                    }
                }
            }
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 10) {
                    Button("Send Failure to claude") {}.buttonStyle(.bordered)
                    Button("Open on GitHub") {}.buttonStyle(.borderless)
                }
                VStack(alignment: .leading, spacing: 10) {
                    Button("Send Failure to claude") {}.buttonStyle(.bordered)
                    Button("Open on GitHub") {}.buttonStyle(.bordered)
                }
            }
            .font(look.ui(.subheadline, .medium))
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(look.row, in: .rect(cornerRadius: 14))
        .overlay(RoundedRectangle(cornerRadius: 14).strokeBorder(look.hairline))
    }
}
