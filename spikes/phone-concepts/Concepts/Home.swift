import SwiftUI

/// Sessions across every machine. Blocked ones are always first.
struct Home: View {
    enum Grouping { case status, project }
    var grouping: Grouping
    @Binding var sheet: SheetKind?
    @Environment(\.look) private var look
    @Environment(\.dynamicTypeSize) private var size

    var body: some View {
        List {
            Section {
                ForEach(Fake.needs) { SessionLink(s: $0) }
            } header: {
                Text("Needs you")
            }
            switch grouping {
            case .status:
                Section("Working") { ForEach(Fake.working) { SessionLink(s: $0) } }
                Section("Recent") { ForEach(Fake.recent) { SessionLink(s: $0) } }
            case .project:
                ForEach(Fake.projects, id: \.self) { p in
                    Section(p) {
                        ForEach(Fake.sessions.filter { $0.project == p && $0.status != .needs }) { SessionLink(s: $0, showProject: false) }
                    }
                }
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(look.background)
        .navigationTitle("Sessions")
        .navigationSubtitle(grouping == .status ? (size.isAccessibilitySize ? "" : "Claude 62% · Codex 18% used this week") : "")
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Menu {
                    Picker("Group by", selection: .constant(grouping == .status ? 0 : 1)) {
                        Text("Status").tag(0)
                        Text("Project").tag(1)
                        Text("Machine").tag(2)
                    }
                    Toggle("Show archived", isOn: .constant(false))
                } label: {
                    Label("View options", systemImage: "line.3.horizontal.decrease")
                }
            }
            ToolbarItem(placement: .topBarTrailing) {
                Button { sheet = .new } label: { Label("New session", systemImage: "square.and.pencil") }
            }
        }
    }
}

struct SessionLink: View {
    let s: Session
    var showProject = true
    var body: some View {
        NavigationLink(value: Route.chat) { SessionRow(s: s, showProject: showProject) }
            .swipeActions(edge: .trailing) {
                Button(role: .destructive) {} label: { Label("Archive", systemImage: "archivebox") }
                if s.status == .working {
                    Button {} label: { Label("Stop", systemImage: "stop.fill") }.tint(.orange)
                }
            }
            .swipeActions(edge: .leading) {
                if s.status == .needs {
                    Button {} label: { Label("Allow", systemImage: "checkmark") }.tint(.green)
                }
            }
            .contextMenu {
                Button {} label: { Label("Open", systemImage: "arrow.up.forward") }
                Button {} label: { Label("Pin", systemImage: "pin") }
                Button {} label: { Label("Mute notifications", systemImage: "bell.slash") }
                Divider()
                Button(role: .destructive) {} label: { Label("Archive", systemImage: "archivebox") }
            }
    }
}

/// One session: title, what it's doing in plain words, and where it runs.
struct SessionRow: View {
    let s: Session
    var showProject = true
    @Environment(\.look) private var look
    @Environment(\.dynamicTypeSize) private var size

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Mark(status: s.status)
                .alignmentGuide(.firstTextBaseline) { $0[.bottom] - 1 }
            VStack(alignment: .leading, spacing: 3) {
                // At accessibility sizes the title wraps and the age moves below it.
                if size.isAccessibilitySize {
                    Text(s.title).font(look.ui(.body, .medium)).foregroundStyle(look.ink).lineLimit(4)
                    Text(s.age).font(look.ui(.subheadline)).foregroundStyle(look.quiet).monospacedDigit()
                } else {
                    HStack(alignment: .firstTextBaseline) {
                        Text(s.title).font(look.ui(.body, .medium)).foregroundStyle(look.ink).lineLimit(1)
                        Spacer(minLength: 8)
                        Text(s.age).font(look.ui(.subheadline)).foregroundStyle(look.quiet).monospacedDigit()
                    }
                }
                Text(s.detail).font(look.ui(.subheadline)).foregroundStyle(look.quiet).lineLimit(size.isAccessibilitySize ? 5 : 2)
                Text(showProject ? "\(s.project) · \(s.machine) · \(s.agent)" : "\(s.machine) · \(s.agent)")
                    .font(look.ui(.footnote)).foregroundStyle(look.quiet)
            }
        }
        .padding(.vertical, 4)
        .listRowBackground(look.row)
        .accessibilityElement(children: .combine)
        .accessibilityValue(s.status.label)
    }
}

/// First run: no machines yet.
struct EmptyHome: View {
    @Binding var sheet: SheetKind?
    @Environment(\.look) private var look
    var body: some View {
        ContentUnavailableView {
            Label("No Machines Yet", systemImage: "desktopcomputer")
        } description: {
            Text("Install dent on your computer, then add it here. Sessions from every machine show up in one list.")
        } actions: {
            Button { sheet = .add } label: { Text("Add a Machine").padding(.horizontal, 8).foregroundStyle(look.onTint) }
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
            Link("How to install dent", destination: URL(string: "https://dent.sh")!)
                .font(look.ui(.subheadline))
        }
        .font(look.ui(.body))
        .background(look.background)
        .navigationTitle("Sessions")
    }
}

/// Usage limits as the tab bar's accessory (the alternative to the subtitle).
struct UsageAccessory: View {
    @Environment(\.look) private var look
    var body: some View {
        HStack(spacing: 10) {
            Gauge(value: 0.62) { EmptyView() }
                .gaugeStyle(.accessoryCircularCapacity)
                .scaleEffect(0.42)
                .frame(width: 22, height: 22)
                .tint(look.ink)
            Text("claude 62%").font(look.ui(.subheadline, .medium)).monospacedDigit()
            Text("codex 18%").font(look.ui(.subheadline)).foregroundStyle(look.quiet).monospacedDigit()
            Spacer()
            Text("resets Thu").font(look.ui(.footnote)).foregroundStyle(look.quiet)
        }
        .padding(.horizontal, 16)
    }
}

/// Automation results and finished goals.
struct Inbox: View {
    @Environment(\.look) private var look
    var body: some View {
        List {
            Section("New") {
                InboxRow(icon: "clock.arrow.circlepath", title: "Nightly dependency check", detail: "3 updates available · dent", age: "6:00", status: .done)
                InboxRow(icon: "flag.checkered", title: "Goal: docs site builds clean", detail: "Finished after 4 runs · website", age: "Yesterday", status: .done)
            }
            Section("Running") {
                VStack(alignment: .leading, spacing: 8) {
                    InboxRow(icon: "flag", title: "Goal: every spike has a findings note", detail: "Run 3 of about 5 · dent", age: "12m", status: .working)
                    ProgressView(value: 0.6).tint(look.tint).padding(.leading, 34)
                }
                .listRowBackground(look.row)
            }
            Section("Paused") {
                InboxRow(icon: "flag", title: "Goal: migrate CI to the new runners", detail: "Paused at the usage limit · resumes 3:10 pm", age: "2h", status: .waiting)
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(look.background)
        .navigationTitle("Inbox")
    }
}

struct InboxRow: View {
    let icon: String, title: String, detail: String, age: String
    let status: Status
    @Environment(\.look) private var look
    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Image(systemName: icon).foregroundStyle(look.quiet).frame(width: 22)
            VStack(alignment: .leading, spacing: 3) {
                HStack(alignment: .firstTextBaseline) {
                    Text(title).font(look.ui(.body, .medium)).lineLimit(1)
                    Spacer(minLength: 8)
                    Text(age).font(look.ui(.subheadline)).foregroundStyle(look.quiet)
                }
                HStack(spacing: 6) {
                    Mark(status: status)
                    Text(detail).font(look.ui(.subheadline)).foregroundStyle(look.quiet).lineLimit(2)
                }
            }
        }
        .padding(.vertical, 4)
        .listRowBackground(look.row)
        .swipeActions {
            Button(role: .destructive) {} label: { Label("Discard", systemImage: "trash") }
            Button {} label: { Label("Continue", systemImage: "arrow.turn.down.right") }.tint(.blue)
        }
    }
}

struct SearchScreen: View {
    @Environment(\.look) private var look
    @State private var query = ""
    var body: some View {
        List {
            Section("Recent") {
                ForEach(Fake.sessions.prefix(3)) { SessionRow(s: $0) }
            }
        }
        .scrollContentBackground(.hidden)
        .background(look.background)
        .navigationTitle("Search")
        .searchable(text: $query, prompt: "Sessions, messages, docs")
    }
}
