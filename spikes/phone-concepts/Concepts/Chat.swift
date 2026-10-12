import SwiftUI

/// A session. The reply reads like a page; chrome is the system's own bars.
struct ChatScreen: View {
    enum Style { case bubbles, transcript }
    enum Phase { case needs, working, offline, pr }
    var style: Style
    var fromTop = false
    var toolsOpen: Bool
    var state: Phase
    @Environment(\.look) private var look
    @State private var draft = ""

    var items: [Item] {
        let rest = Chat.items.filter { if case .approval = $0 { false } else { true } }
        switch state {
        case .needs: return Chat.items
        case .pr: return rest + [.text("Tests pass locally. I opened a pull request."), .pr(number: 142, title: "Resume from the last event received")]
        default: return rest
        }
    }

    var subtitle: String {
        switch state {
        case .needs: "claude · studio · Needs you"
        case .working: "claude · studio · Working"
        case .offline: "claude · studio · Offline"
        case .pr: "claude · studio · Done · CI failed"
        }
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                ForEach(Array(items.enumerated()), id: \.element.id) { i, item in
                    if style == .transcript && i == 1 { Speaker(name: "claude") }
                    ItemView(item: item, style: style, toolsOpen: toolsOpen)
                }
                if state == .working {
                    HStack(spacing: 8) {
                        ProgressView().controlSize(.small)
                        Text("Working · 1m 12s").font(look.ui(.subheadline)).foregroundStyle(look.quiet).monospacedDigit()
                    }
                }
            }
            .padding(.horizontal, 20)
            .padding(.top, 12)
            .padding(.bottom, 24)
        }
        .defaultScrollAnchor(fromTop ? .top : .bottom)
        .scrollDismissesKeyboard(.interactively)
        .background(look.page)
        .navigationTitle(Fake.chat.title)
        .navigationBarTitleDisplayMode(.inline)
        .navigationSubtitle(subtitle)
        .toolbar(.hidden, for: .tabBar)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Menu {
                    NavigationLink(value: Route.changes) { Label("Changed Files", systemImage: "doc.on.doc") }
                    NavigationLink(value: Route.doc) { Label("Plan", systemImage: "doc.richtext") }
                    Button {} label: { Label("Open Preview", systemImage: "safari") }
                    Divider()
                    Button {} label: { Label("My Messages", systemImage: "text.bubble") }
                    Button {} label: { Label("Mute", systemImage: "bell.slash") }
                    Divider()
                    Button(role: .destructive) {} label: { Label("Stop", systemImage: "stop.fill") }
                } label: {
                    Label("More", systemImage: "ellipsis")
                }
            }
        }
        .safeAreaBar(edge: .bottom) {
            VStack(spacing: 8) {
                if state == .offline {
                    HStack(spacing: 8) {
                        Mark(status: .offline)
                        Text("studio is offline. Messages wait here and send when it's back.")
                            .font(look.ui(.footnote)).foregroundStyle(look.quiet)
                        Spacer(minLength: 0)
                    }
                    .padding(.horizontal, 20)
                }
                Composer(draft: $draft, working: state == .working, machine: "studio")
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
    let item: Item
    let style: ChatScreen.Style
    let toolsOpen: Bool
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
        case .text(let md):
            Text(try! AttributedString(markdown: md))
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
            Approval(command: command, why: why)
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
    @Environment(\.look) private var look
    var deny: some View {
        Button {} label: { Text("Deny").lineLimit(1).frame(maxWidth: .infinity) }.buttonStyle(.bordered)
    }
    var allow: some View {
        Button {} label: { Text("Allow").lineLimit(1).frame(maxWidth: .infinity).foregroundStyle(look.onTint) }.buttonStyle(.borderedProminent)
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
    @Environment(\.look) private var look
    @ScaledMetric(relativeTo: .body) private var control: CGFloat = 44

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
                        .lineLimit(1...6)
                        .padding(.leading, 16).padding(.vertical, 11)
                    // Send appears once there is something to send; Stop while working.
                    if working || !draft.isEmpty {
                    Button {} label: {
                        Image(systemName: working ? "stop.fill" : "arrow.up")
                            .font(.body.weight(.bold))
                            .foregroundStyle(look.onTint)
                            .frame(width: control - 12, height: control - 12)
                    }
                    .buttonStyle(.borderedProminent)
                    .buttonBorderShape(.circle)
                    .accessibilityLabel(working ? "Stop" : "Send")
                    .padding(6)
                    } else {
                        Button {} label: {
                            Image(systemName: "mic").font(.body).frame(width: control - 12, height: control - 12)
                        }
                        .tint(look.quiet)
                        .accessibilityLabel("Dictate")
                        .padding(6)
                    }
                }
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
