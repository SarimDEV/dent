import Foundation
import SwiftUI

/// One machine as the UI sees it: status, and the conversation in its
/// session, rebuilt from the core's notes.
@MainActor
final class MachineModel: ObservableObject, Identifiable {
    struct Message: Identifiable, Equatable {
        let id: UInt32
        var role: Role
        var state: Turn
        var blocks: [Block]
    }

    struct Pending: Identifiable, Equatable {
        let id: String
        let text: String
    }

    let id: String
    @Published var name: String
    @Published var addr: String
    @Published private(set) var status: Status = .idle
    @Published private(set) var detail = ""
    @Published private(set) var rtt: UInt64?
    @Published private(set) var lostAt: Date?
    @Published private(set) var messages: [Message] = []
    @Published private(set) var pending: [Pending] = []
    @Published var pairError: String?
    @Published private(set) var push: String?
    @Published private(set) var info: Info?
    private(set) var key: String?
    let session: String
    private(set) var host: Host!
    var onPaired: ((MachineModel) -> Void)?

    init(id: String, name: String, addr: String, key: String?, session: String) {
        self.id = id
        self.name = name
        self.addr = addr
        self.key = key
        self.session = session
        let link = try! CoreLink(name: "iPhone", keys: Store.shared.keys, known: key, addrs: [addr])
        host = Host(link: link) { [weak self] notes in
            MainActor.assumeIsolated { self?.apply(notes) }
        }
    }

    var saved: SavedMachine? {
        key.map { SavedMachine(id: id, name: name, addr: addr, key: $0, session: session) }
    }

    func start() {
        let s = session
        if key != nil { host.run { l, _ in l.follow(session: s) } }
        host.start()
    }

    func stop() { host.stop() }

    func pair(_ code: String) {
        pairError = nil
        host.run { l, now in l.pair(code: code, now: now) }
    }

    func send(_ text: String) {
        let s = session
        host.run { l, _ in _ = l.prompt(session: s, text: text) }
    }

    func setPush(handle: String, key: Data) {
        let k = key.hex
        host.run { l, _ in l.setPush(handle: handle, keyHex: k) }
    }

    /// The calm line under the name.
    var line: (fill: Color?, text: String) {
        let reconnecting = lostAt.map { Date().timeIntervalSince($0) < 10 } ?? false
        switch status {
        case .online:
            return (Theme.done, rtt.map { $0 < 1 ? "Online · under 1 ms" : "Online · \($0) ms" } ?? "Online")
        case .connecting: return (Theme.working, key == nil ? "Connecting…" : "Reconnecting…")
        case .pairing: return (Theme.working, "Pairing…")
        case .offline: return reconnecting ? (Theme.working, "Reconnecting…") : (nil, "Offline · retrying")
        case .needsCode: return (Theme.needs, "Needs the code shown on it")
        case .removed: return (Theme.needs, "Removed this iPhone")
        case .incompatible: return (Theme.needs, "Needs a dent update")
        case .idle: return (nil, "")
        }
    }

    /// Whether phone notifications are held back, and why, in words.
    var pushNotice: String? {
        guard let p = push else { return nil }
        if p == "unavailable" { return "Phone notifications are unavailable right now." }
        if p.hasPrefix("limited:"), let ms = Double(p.dropFirst(8)) {
            let f = DateFormatter()
            f.timeStyle = .short
            let until = f.string(from: Date(timeIntervalSince1970: ms / 1000))
            return "Phone notifications paused until \(until): today's limit is reached. Everything still shows here."
        }
        return nil
    }

    private func apply(_ notes: [Note]) {
        for n in notes {
            switch n {
            case let .status(s, d):
                if s == .offline && status != .offline { lostAt = Date() }
                if s == .online { lostAt = nil; pairError = nil }
                status = s
                detail = d
            case let .welcome(name, _, _, _, info):
                self.name = name
                self.info = info
            case let .paired(key, name, _):
                self.key = key
                if !name.isEmpty { self.name = name }
                let s = session
                host.run { l, _ in l.follow(session: s) }
                onPaired?(self)
            case let .pairFailed(error):
                pairError = Self.words(error, name)
            case let .message(u):
                apply(u)
            case let .queued(id, _, text):
                pending.append(Pending(id: id, text: text))
            case let .accepted(id, _, _):
                pending.removeAll { $0.id == id }
            case let .health(h):
                push = h.push
            case let .rtt(ms):
                rtt = ms
            case .bye:
                break
            }
        }
    }

    private func apply(_ u: MessageUpdate) {
        guard u.session == session else { return }
        let i = Int(u.index)
        while messages.count <= i {
            messages.append(Message(id: UInt32(messages.count), role: u.role, state: u.state, blocks: []))
        }
        var m = messages[i]
        m.role = u.role
        m.state = u.state
        m.blocks.removeSubrange(min(Int(u.keep), m.blocks.count)...)
        m.blocks.append(contentsOf: u.blocks)
        messages[i] = m
    }

    static func words(_ e: PairError, _ name: String) -> String {
        switch e {
        case .wrong: return "That code didn't match. Check the code on \(name) and try again."
        case .wrongNewCode: return "That code didn't match three times, so \(name) now shows a new one."
        case .closed: return "No code is showing on \(name). Open Settings › Devices there to show one."
        case let .locked(secs): return "Too many wrong codes. Try again in \(secs) s."
        case let .unreachable(why): return "Lost \(name) while pairing (\(why))."
        case let .failed(why): return "Couldn't pair: \(why)"
        }
    }
}
