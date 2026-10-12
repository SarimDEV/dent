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
    case compose
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
struct Root: View {
    let screen: String
    @Environment(\.look) private var look
    @State private var path: [Route] = []
    @State private var sheet: SheetKind?

    init(screen: String) {
        self.screen = screen
        var path: [Route] = [], sheet: SheetKind?
        switch screen {
        case "chat", "chat-pr", "chat-transcript", "chat-tools", "chat-working", "chat-offline": path = [.chat]
        case "new", "new-typed": sheet = .compose
        case "doc": path = [.chat, .doc]
        case "changes": path = [.chat, .changes]
        case "settings", "settings-limited": path = [.settings]
        case "machine", "machine-remove": path = [.settings, .machine(Fake.machines[0])]
        case "add", "empty-add": sheet = .add
        case "code": sheet = .code
        case "qr": sheet = .qr
        case "paired": sheet = .paired
        default: break
        }
        _path = State(initialValue: path)
        _sheet = State(initialValue: sheet)
    }

    var body: some View {
        if ["lock", "notify-actions", "island"].contains(screen) {
            SystemMock(screen: screen)
        } else if screen == "welcome" {
            Welcome()
        } else {
            NavigationStack(path: $path) {
                Group {
                    if screen.hasPrefix("empty") { EmptyHome(sheet: $sheet).homeToolbar(path: $path, sheet: $sheet, full: false) }
                    else if screen == "syncing" { Syncing().homeToolbar(path: $path, sheet: $sheet) }
                    else { Home(zero: screen == "home-zero").homeToolbar(path: $path, sheet: $sheet) }
                }
                .navigationDestination(for: Route.self) { destination($0) }
            }
            .sheet(item: $sheet) { kind in
                Group {
                    if kind == .compose {
                        NavigationStack { NewSession(typed: screen == "new-typed") }
                            .presentationDetents([.large])
                    } else {
                        PairingFlow(start: kind, sheet: $sheet)
                    }
                }
                .environment(\.look, look)
                .tint(look.tint)
            }
        }
    }

    @ViewBuilder func destination(_ r: Route) -> some View {
        switch r {
        case .chat:
            ChatScreen(style: screen == "chat-transcript" ? .transcript : .bubbles,
                       fromTop: screen == "chat-transcript" || screen == "chat-tools",
                       toolsOpen: screen == "chat-tools",
                       state: ["chat-working": .working, "chat-offline": .offline, "chat-pr": .pr][screen] ?? .needs)
        case .compose: NewSession(typed: screen == "new-typed")
        case .doc: DocScreen()
        case .changes: ChangesScreen()
        case .settings: Settings(limited: screen == "settings-limited", sheet: $sheet)
        case .machine(let m): MachineScreen(machine: m)
        }
    }
}
