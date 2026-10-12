import SwiftUI

/// Every session on every machine, as an inbox: what needs you, then what's
/// done and waiting for you to steer or close. Working sessions sit dimmed
/// at the bottom. Inbox zero is when everything is working or closed.
struct Home: View {
    @Environment(Store.self) private var store
    @Environment(\.look) private var look
    @Environment(\.isSearching) private var searching
    var query: String

    var body: some View {
        List {
            if !query.isEmpty {
                Section {
                    ForEach(store.search(query)) { SessionLink(s: $0) }
                } header: {
                    Text(store.search(query).isEmpty ? "No sessions match" : "Results")
                }
            } else {
                if store.toClear == 0 {
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
                }
                if !store.needs.isEmpty {
                    Section("Needs you") { ForEach(store.needs) { SessionLink(s: $0) } }
                }
                if !store.done.isEmpty {
                    Section("Done") { ForEach(store.done) { SessionLink(s: $0) } }
                }
                if !store.busy.isEmpty {
                    Section("Working") { ForEach(store.busy) { SessionLink(s: $0, quiet: true) } }
                }
                Section {
                    NavigationLink(value: Route.closed) {
                        LabeledContent {
                            Text("\(store.closed.count)").monospacedDigit().contentTransition(.numericText())
                        } label: {
                            Label("Closed", systemImage: "archivebox")
                        }
                    }
                    .font(look.ui(.body))
                    .listRowBackground(look.row)
                    .accessibilityIdentifier("closed-list")
                }
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(look.background)
        .navigationTitle("Sessions")
        .navigationSubtitle(Text(store.toClear == 0 ? "\(store.busy.count) working" : "\(store.toClear) to clear"))
        // The undo sits above the toolbar and the list scrolls clear of it.
        .safeAreaInset(edge: .bottom) {
            if let t = store.toast {
                UndoToast(toast: t).transition(.move(edge: .bottom).combined(with: .opacity))
            }
        }
    }
}

/// Closed sessions: archived, never deleted. Swipe to reopen.
struct ClosedList: View {
    @Environment(Store.self) private var store
    @Environment(\.look) private var look
    var body: some View {
        Group {
            if store.closed.isEmpty {
                ContentUnavailableView("Nothing Closed", systemImage: "archivebox",
                                       description: Text("Sessions you close land here, and you can reopen them any time."))
            } else {
                List {
                    Section {
                        ForEach(store.closed) { s in
                            NavigationLink(value: Route.chat(s.id)) { SessionRow(s: s) }
                                .accessibilityIdentifier("row-\(s.id)")
                                .swipeActions(edge: .leading) {
                                    Button { store.reopen(s.id) } label: { Label("Reopen", systemImage: "arrow.uturn.backward") }
                                        .tint(look.tint)
                                }
                                .contextMenu {
                                    Button { store.reopen(s.id) } label: { Label("Reopen", systemImage: "arrow.uturn.backward") }
                                }
                        }
                    } footer: {
                        Text("Closed sessions stay searchable. Reopening puts one back in your list.")
                    }
                }
                .listStyle(.insetGrouped)
                .scrollContentBackground(.hidden)
            }
        }
        .background(look.background)
        .navigationTitle("Closed")
        .navigationBarTitleDisplayMode(.inline)
    }
}

/// The undo for a close, floating above the bottom toolbar.
struct UndoToast: View {
    let toast: Store.Toast
    @Environment(Store.self) private var store
    @Environment(\.look) private var look
    var body: some View {
        HStack(spacing: 12) {
            Text(toast.text).font(look.ui(.subheadline))
            Spacer(minLength: 4)
            Button(toast.action) { store.toastAction() }
                .font(look.ui(.subheadline, .semibold))
                .frame(minHeight: 44)
                .accessibilityIdentifier("undo")
        }
        .padding(.leading, 18).padding(.trailing, 14)
        .glassEffect(.regular, in: .capsule)
        .padding(.horizontal, 16)
        .padding(.bottom, 8)
        // Eight seconds of being seen: leaving the screen restarts the clock.
        .task(id: toast.id) {
            try? await Task.sleep(for: .seconds(8))
            if !Task.isCancelled { store.dismissToast(toast) }
        }
    }
}

/// The list's chrome, shared by every state of it: Settings and usage at the
/// top, search and New Session in the bottom toolbar.
struct HomeToolbar: ViewModifier {
    @Binding var path: [Route]
    @Binding var sheet: SheetKind?
    var full = true
    var query: Binding<String> = .constant("")

    func body(content: Content) -> some View {
        if full {
            content
                .searchable(text: query, prompt: "Sessions, messages, docs")
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
                .accessibilityIdentifier("new-session")
        }
    }
}

extension View {
    func homeToolbar(path: Binding<[Route]>, sheet: Binding<SheetKind?>, full: Bool = true, query: Binding<String> = .constant("")) -> some View {
        modifier(HomeToolbar(path: path, sheet: sheet, full: full, query: query))
    }
}

struct SessionLink: View {
    let s: Session
    var showProject = true
    var quiet = false
    @Environment(Store.self) private var store
    var body: some View {
        NavigationLink(value: Route.chat(s.id)) { SessionRow(s: s, showProject: showProject, quiet: quiet) }
            .swipeActions(edge: .trailing, allowsFullSwipe: s.status != .working) {
                if s.closed {
                    Button { store.reopen(s.id) } label: { Label("Reopen", systemImage: "arrow.uturn.backward") }.tint(.blue)
                } else if s.status == .working {
                    // Stopping takes a deliberate tap, never a long swipe, and offers Resume.
                    Button(role: .destructive) { store.stop(s.id) } label: { Label("Stop", systemImage: "stop.fill") }
                } else if store.canClose(s) {
                    // Closing archives: it can always be reopened.
                    Button { store.close(s.id) } label: { Label("Close", systemImage: "archivebox") }.tint(.gray)
                }
            }
            .swipeActions(edge: .leading, allowsFullSwipe: false) {
                if s.status == .needs && s.id == "s1" && !s.closed {
                    Button { store.allow(s.id) } label: { Label("Allow", systemImage: "checkmark") }.tint(.green)
                } else if store.stopped.contains(s.id) && !s.closed {
                    Button { store.resume(s.id) } label: { Label("Resume", systemImage: "play.fill") }.tint(.blue)
                }
            }
            .contextMenu {
                Button {} label: { Label("Mute Notifications", systemImage: "bell.slash") }
                if s.closed {
                    Button { store.reopen(s.id) } label: { Label("Reopen", systemImage: "arrow.uturn.backward") }
                } else if store.canClose(s) {
                    Button { store.close(s.id) } label: { Label("Close", systemImage: "archivebox") }
                }
            }
            .accessibilityIdentifier("row-\(s.id)")
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
    @Environment(Store.self) private var store

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Group {
                if s.closed {
                    // Archived: no signal colour, the one loud colour stays for live work.
                    Image(systemName: "archivebox").font(.footnote).foregroundStyle(look.quiet)
                } else {
                    Mark(status: s.status, popOnAppear: store.finishedAt[s.id].map { Date.now.timeIntervalSince($0) < 1 } ?? false)
                }
            }
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
                Text((s.closed ? "Closed · " : "") + (showProject ? "\(s.project) · \(s.machine) · \(s.agent)" : "\(s.machine) · \(s.agent)"))
                    .font(look.ui(.footnote)).foregroundStyle(look.quiet)
            }
        }
        .padding(.vertical, 4)
        .opacity(quiet && !s.closed ? 0.65 : 1)
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

