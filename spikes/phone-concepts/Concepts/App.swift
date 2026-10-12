import SwiftUI

/// Launch with `-screen <name> -look system|paper|book`. Every screen is a
/// real navigation state (real back buttons, real sheets), not a picture.
@main
struct ConceptsApp: App {
    let look: Look
    init() {
        look = Look(id: LookID(rawValue: arg("-look") ?? "paper") ?? .paper)
        look.applyAppearance()
    }
    var body: some Scene {
        WindowGroup {
            Root(screen: arg("-screen") ?? "home")
                .environment(\.look, look)
                .tint(look.tint)
        }
    }
}

func arg(_ key: String) -> String? {
    let a = ProcessInfo.processInfo.arguments
    guard let i = a.firstIndex(of: key), i + 1 < a.count else { return nil }
    return a[i + 1]
}

enum Route: Hashable {
    case chat(String)
    case closed
    case doc
    case changes
    case settings
    case machine(Machine)
}

enum SheetKind: String, Identifiable {
    case add, code, qr, paired, compose
    var id: String { rawValue }
}

/// One area: the session list. Settings and search hang off it; there are no tabs.
/// With no `-screen` (or `-screen live`) everything is interactive.
struct Root: View {
    let screen: String
    @Environment(\.look) private var look
    @State private var store: Store
    @State private var path: [Route] = []
    @State private var sheet: SheetKind?
    @State private var query = ""

    init(screen: String) {
        self.screen = screen
        let store = Store(pace: Double(arg("-pace") ?? "1") ?? 1)
        var path: [Route] = [], sheet: SheetKind?
        let chat: [Route] = [.chat("s1")]
        switch screen {
        case "chat", "chat-transcript", "chat-tools", "chat-offline": path = chat
        case "chat-working": store.prepare(.working); path = chat
        case "chat-pr": store.prepare(.pr); path = chat
        case "home-zero": store.prepare(.zero)
        case "closed": store.prepare(.zero); path = [.closed]
        case "many": store.prepare(.many)
        case "new", "new-typed": sheet = .compose
        case "doc": path = chat + [.doc]
        case "changes": path = chat + [.changes]
        case "settings", "settings-limited": path = [.settings]
        case "machine", "machine-remove": path = [.settings, .machine(Fake.machines[0])]
        case "add", "empty-add": sheet = .add
        case "code": sheet = .code
        case "qr": sheet = .qr
        case "paired": sheet = .paired
        default: break
        }
        _store = State(initialValue: store)
        _path = State(initialValue: path)
        _sheet = State(initialValue: sheet)
    }

    var body: some View {
        Group {
            if ["lock", "notify-actions", "island"].contains(screen) {
                SystemMock(screen: screen)
            } else if screen == "welcome" {
                Welcome()
            } else {
                NavigationStack(path: $path) {
                    Group {
                        if screen.hasPrefix("empty") { EmptyHome(sheet: $sheet).homeToolbar(path: $path, sheet: $sheet, full: false) }
                        else if screen == "syncing" { Syncing().homeToolbar(path: $path, sheet: $sheet) }
                        else { Home(query: query).homeToolbar(path: $path, sheet: $sheet, query: $query) }
                    }
                    .navigationDestination(for: Route.self) { destination($0) }
                }
                .sheet(item: $sheet) { kind in
                    Group {
                        if kind == .compose {
                            NavigationStack {
                                NewSession(typed: screen == "new-typed") { text, project in
                                    let id = store.start(text, project: project)
                                    sheet = nil
                                    path = [.chat(id)]
                                }
                            }
                            .presentationDetents([.large])
                        } else {
                            PairingFlow(start: kind, sheet: $sheet)
                        }
                    }
                    .environment(\.look, look)
                    .environment(store)
                    .tint(look.tint)
                }
            }
        }
        .environment(store)
        .sensoryFeedback(.impact(weight: .light), trigger: store.closedCount)
        .sensoryFeedback(.success, trigger: store.allowedCount)
        .sensoryFeedback(.warning, trigger: store.stoppedCount)
    }

    @ViewBuilder func destination(_ r: Route) -> some View {
        switch r {
        case .chat(let id):
            ChatScreen(id: id,
                       style: screen == "chat-transcript" ? .transcript : .bubbles,
                       fromTop: screen == "chat-transcript" || screen == "chat-tools",
                       toolsOpen: screen == "chat-tools",
                       offline: screen == "chat-offline")
        case .closed: ClosedList()
        case .doc: DocScreen()
        case .changes: ChangesScreen()
        case .settings: Settings(limited: screen == "settings-limited", sheet: $sheet)
        case .machine(let m): MachineScreen(machine: m)
        }
    }
}
