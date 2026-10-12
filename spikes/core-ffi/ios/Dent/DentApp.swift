import SwiftUI
import UIKit
import UserNotifications

func arg(_ name: String) -> String? {
    let a = ProcessInfo.processInfo.arguments
    return a.firstIndex(of: name).flatMap { a.indices.contains($0 + 1) ? a[$0 + 1] : nil }
}

func flag(_ name: String) -> Bool { ProcessInfo.processInfo.arguments.contains(name) }

@main
struct DentApp: App {
    @UIApplicationDelegateAdaptor(AppDelegate.self) var delegate
    @StateObject private var app = AppModel()

    var body: some Scene {
        WindowGroup {
            Group {
                if flag("-bench") {
                    BenchView()
                } else {
                    HomeView(discovery: app.discovery)
                        .environmentObject(app)
                }
            }
            .onAppear { AppDelegate.model = app }
        }
    }
}

/// Every machine, the ones found nearby, and the one being added.
@MainActor
final class AppModel: ObservableObject {
    @Published var machines: [MachineModel] = []
    @Published var adding: MachineModel?
    @Published var open: MachineModel?
    let discovery = Discovery()

    init() {
        if flag("-fresh") { Store.shared.reset() }
        guard !flag("-bench") else { return }
        machines = Store.shared.machines.map {
            MachineModel(id: $0.id, name: $0.name, addr: $0.addr, key: $0.key, session: $0.session)
        }
        for m in machines { m.start() }
        discovery.start()
        if !machines.isEmpty { Task { await Push.enable() } }
    }

    var nearby: [Discovery.Found] {
        discovery.answering.filter { f in !machines.contains { $0.id == f.id } }
    }

    func add(id: String, name: String, addr: String) {
        adding?.stop()
        let m = MachineModel(id: id, name: name, addr: addr, key: nil, session: Self.newSession())
        m.onPaired = { [weak self] m in self?.paired(m) }
        adding = m
        m.start()
    }

    func cancelAdding() {
        adding?.stop()
        adding = nil
    }

    private func paired(_ m: MachineModel) {
        if !machines.contains(where: { $0.id == m.id }) { machines.append(m) }
        save()
        adding = nil
        open = m
        if let h = Store.shared.pushHandle { m.setPush(handle: h, key: Store.shared.pushKey) }
        Task { await Push.enable() }
    }

    func save() {
        Store.shared.machines = machines.compactMap(\.saved)
    }

    func pushReady(handle: String) {
        Store.shared.pushHandle = handle
        for m in machines { m.setPush(handle: handle, key: Store.shared.pushKey) }
    }

    static func newSession() -> String {
        "phone-" + (0..<4).map { _ in String(format: "%02x", UInt8.random(in: 0...255)) }.joined()
    }
}

extension MachineModel: Hashable {
    nonisolated static func == (a: MachineModel, b: MachineModel) -> Bool { a.id == b.id }
    nonisolated func hash(into h: inout Hasher) { h.combine(id) }
}

final class AppDelegate: NSObject, UIApplicationDelegate, UNUserNotificationCenterDelegate {
    @MainActor static var model: AppModel?

    func application(_ application: UIApplication,
                     didFinishLaunchingWithOptions o: [UIApplication.LaunchOptionsKey: Any]? = nil) -> Bool {
        UNUserNotificationCenter.current().delegate = self
        return true
    }

    func application(_ application: UIApplication, didRegisterForRemoteNotificationsWithDeviceToken token: Data) {
        guard arg("-pushtoken") == nil else { return }
        Task { await Push.register(token: token.hex) }
    }

    func application(_ application: UIApplication, didFailToRegisterForRemoteNotificationsWithError e: Error) {
        NSLog("dent: push registration failed: \(e.localizedDescription)")
    }

    // Banners show while the app is open too.
    func userNotificationCenter(_ c: UNUserNotificationCenter, willPresent n: UNNotification,
                                withCompletionHandler done: @escaping (UNNotificationPresentationOptions) -> Void) {
        done([.banner, .list, .sound])
    }
}

/// Registering with dent-push: the phone's APNs token goes in, a sealed
/// handle comes back, and each paired machine gets the handle and the key.
enum Push {
    static var url: String? { arg("-pushurl") }

    @MainActor static func enable() async {
        guard url != nil else { return }
        _ = try? await UNUserNotificationCenter.current()
            .requestAuthorization(options: [.alert, .sound, .badge, .timeSensitive])
        if let t = arg("-pushtoken") {
            await register(token: t)
        } else {
            UIApplication.shared.registerForRemoteNotifications()
        }
    }

    static func register(token: String) async {
        guard let base = url, let u = URL(string: base + "/register") else { return }
        var req = URLRequest(url: u)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.httpBody = try? JSONSerialization.data(withJSONObject: [
            "token": token, "env": "sandbox", "topic": Bundle.main.bundleIdentifier ?? "",
        ])
        guard let (data, _) = try? await URLSession.shared.data(for: req),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let handle = obj["handle"] as? String else {
            NSLog("dent: dent-push registration failed")
            return
        }
        await MainActor.run { AppDelegate.model?.pushReady(handle: handle) }
    }
}
