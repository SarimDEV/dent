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
    case chat
    case doc
    case changes
    case machine(Machine)
}

enum SheetKind: String, Identifiable {
    case add, code, qr, paired, new
    var id: String { rawValue }
}

struct Root: View {
    let screen: String
    @Environment(\.look) private var look
    @State private var tab = "sessions"
    @State private var path: [Route] = []
    @State private var settingsPath: [Route] = []
    @State private var sheet: SheetKind?

    init(screen: String) {
        self.screen = screen
        var tab = "sessions", path: [Route] = [], settings: [Route] = [], sheet: SheetKind?
        switch screen {
        case "chat", "chat-pr", "chat-bubbles", "chat-transcript", "chat-tools", "chat-working", "chat-offline": path = [.chat]
        case "doc": path = [.chat, .doc]
        case "changes": path = [.chat, .changes]
        case "inbox": tab = "inbox"
        case "settings", "settings-limited": tab = "settings"
        case "machine", "machine-remove": tab = "settings"; settings = [.machine(Fake.machines[0])]
        case "add", "empty-add": sheet = .add
        case "code": sheet = .code
        case "qr": sheet = .qr
        case "paired": sheet = .paired
        case "new": sheet = .new
        default: break
        }
        _tab = State(initialValue: tab)
        _path = State(initialValue: path)
        _settingsPath = State(initialValue: settings)
        _sheet = State(initialValue: sheet)
    }

    var body: some View {
        let tabs = TabView(selection: $tab) {
            Tab("Sessions", systemImage: "bubble.left.and.text.bubble.right", value: "sessions") {
                NavigationStack(path: $path) {
                    Group {
                        if screen.hasPrefix("empty") { EmptyHome(sheet: $sheet) } else if screen == "syncing" { Syncing() } else { Home(grouping: screen == "home-projects" ? .project : .status, sheet: $sheet) }
                    }
                    .navigationDestination(for: Route.self) { destination($0) }
                }
            }
            .badge(screen.hasPrefix("empty") ? 0 : Fake.needs.count)

            Tab("Inbox", systemImage: "tray", value: "inbox") {
                NavigationStack { Inbox() }
            }
            .badge(screen.hasPrefix("empty") ? 0 : 2)

            Tab("Settings", systemImage: "gearshape", value: "settings") {
                NavigationStack(path: $settingsPath) {
                    Settings(limited: screen == "settings-limited", sheet: $sheet)
                        .navigationDestination(for: Route.self) { destination($0) }
                }
            }

            Tab(value: "search", role: .search) {
                NavigationStack { SearchScreen() }
            }
        }
        .tabBarMinimizeBehavior(.onScrollDown)
        .sheet(item: $sheet) { kind in
            PairingFlow(start: kind, sheet: $sheet)
                .environment(\.look, look)
                .tint(look.tint)
        }

        if ["lock", "notify-actions", "island"].contains(screen) {
            SystemMock(screen: screen)
        } else if screen == "welcome" {
            Welcome()
        } else if screen == "home-usage" {
            tabs.tabViewBottomAccessory { UsageAccessory() }
        } else {
            tabs
        }
    }

    @ViewBuilder func destination(_ r: Route) -> some View {
        switch r {
        case .chat:
            ChatScreen(style: screen == "chat-transcript" ? .transcript : .bubbles,
                       fromTop: screen == "chat-transcript" || screen == "chat-tools",
                       toolsOpen: screen == "chat-tools",
                       state: ["chat-working": .working, "chat-offline": .offline, "chat-pr": .pr][screen] ?? .needs)
        case .doc: DocScreen()
        case .changes: ChangesScreen()
        case .machine(let m): MachineScreen(machine: m)
        }
    }
}
