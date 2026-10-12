import SwiftUI

/// Every session on every machine, as an inbox: what needs you, then what's
/// done and waiting for you to steer or close. Working sessions sit dimmed
/// at the bottom. Inbox zero is when everything is working or closed.
struct Home: View {
    var zero = false
    @Environment(\.look) private var look

    var body: some View {
        List {
            if zero {
                Section {
                    HStack(spacing: 14) {
                        Image(systemName: "checkmark.circle").font(.title2).foregroundStyle(look.quiet)
                        VStack(alignment: .leading, spacing: 2) {
                            Text("All caught up").font(look.ui(.body, .semibold))
                            Text("Everything is working or closed.").font(look.ui(.subheadline)).foregroundStyle(look.quiet)
                        }
                    }
                    .padding(.vertical, 8)
                    .listRowBackground(look.row)
                }
            } else {
                Section("Needs you") { ForEach(Fake.needs) { SessionLink(s: $0) } }
                Section {
                    ForEach(Fake.done) { SessionLink(s: $0) }
                } header: {
                    HStack {
                        Text("Done")
                        Spacer()
                        Button("Close All") {}.font(look.ui(.subheadline, .medium)).textCase(nil)
                    }
                }
            }
            Section("Working") { ForEach(Fake.busy) { SessionLink(s: $0, quiet: true) } }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(look.background)
        .navigationTitle("Sessions")
        .navigationSubtitle(zero ? "\(Fake.busy.count) working" : "\(Fake.needs.count + Fake.done.count) to clear")
    }
}

/// The list's chrome, shared by every state of it: Settings and usage at the
/// top, search and New Session in the bottom toolbar.
struct HomeToolbar: ViewModifier {
    @Binding var path: [Route]
    @Binding var sheet: SheetKind?
    var full = true
    @State private var query = ""

    func body(content: Content) -> some View {
        if full {
            content
                .searchable(text: $query, prompt: "Sessions, messages, docs")
                .searchToolbarBehavior(.minimize)
                .toolbar { top; bottom }
        } else {
            content.toolbar { top }
        }
    }

    @ToolbarContentBuilder var top: some ToolbarContent {
        ToolbarItem(placement: .topBarLeading) {
            Button { path.append(.settings) } label: { Label("Settings", systemImage: "gearshape") }
        }
        if full {
            ToolbarItem(placement: .topBarTrailing) {
                Menu {
                    Text("Claude: 62% of this week")
                    Text("Codex: 18% of this week")
                } label: {
                    Label("Usage", systemImage: "gauge.with.dots.needle.50percent")
                }
            }
        }
    }

    @ToolbarContentBuilder var bottom: some ToolbarContent {
        DefaultToolbarItem(kind: .search, placement: .bottomBar)
        ToolbarSpacer(.flexible, placement: .bottomBar)
        ToolbarItem(placement: .bottomBar) {
            Button { sheet = .compose } label: { Label("New Session", systemImage: "square.and.pencil") }
        }
    }
}

extension View {
    func homeToolbar(path: Binding<[Route]>, sheet: Binding<SheetKind?>, full: Bool = true) -> some View {
        modifier(HomeToolbar(path: path, sheet: sheet, full: full))
    }
}

struct SessionLink: View {
    let s: Session
    var showProject = true
    var quiet = false
    var body: some View {
        NavigationLink(value: Route.chat) { SessionRow(s: s, showProject: showProject, quiet: quiet) }
            .swipeActions(edge: .trailing) {
                if s.status == .working {
                    // Working sessions need nothing; the only action is stopping one.
                    Button(role: .destructive) {} label: { Label("Stop", systemImage: "stop.fill") }
                } else {
                    // Closing is how the list gets to zero.
                    Button {} label: { Label("Close", systemImage: "xmark") }.tint(.gray)
                }
            }
            .swipeActions(edge: .leading, allowsFullSwipe: false) {
                if s.status == .needs {
                    Button {} label: { Label("Allow", systemImage: "checkmark") }.tint(.green)
                }
            }
            .contextMenu {
                Button {} label: { Label("Open", systemImage: "arrow.up.forward") }
                Button {} label: { Label("Mute notifications", systemImage: "bell.slash") }
                Divider()
                Button {} label: { Label("Close", systemImage: "xmark") }
            }
    }
}

/// One session: title, what it's doing in plain words, and where it runs.
struct SessionRow: View {
    let s: Session
    var showProject = true
    /// Working rows step back: lighter title, nothing to do yet.
    var quiet = false
    @Environment(\.look) private var look
    @Environment(\.dynamicTypeSize) private var size

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Mark(status: s.status)
                .alignmentGuide(.firstTextBaseline) { $0[.bottom] - 1 }
            VStack(alignment: .leading, spacing: 3) {
                // At accessibility sizes the title wraps and the age moves below it.
                if size.isAccessibilitySize {
                    Text(s.title).font(look.ui(.body, quiet ? .regular : .medium)).foregroundStyle(quiet ? look.quiet : look.ink).lineLimit(4)
                    Text(s.age).font(look.ui(.subheadline)).foregroundStyle(look.quiet).monospacedDigit()
                } else {
                    HStack(alignment: .firstTextBaseline) {
                        Text(s.title).font(look.ui(.body, quiet ? .regular : .medium)).foregroundStyle(quiet ? look.quiet : look.ink).lineLimit(1)
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
        .opacity(quiet ? 0.65 : 1)
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

