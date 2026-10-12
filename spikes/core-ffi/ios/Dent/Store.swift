import Foundation

/// What the app keeps between launches. Shared with the notification
/// extension through the app group, which needs the notification key.
/// The real app keeps the device key in the Keychain.
struct SavedMachine: Codable, Equatable {
    var id: String
    var name: String
    var addr: String
    var key: String
    var session: String
}

final class Store {
    static let shared = Store()
    static let group = "group.dev.dent.spike07"
    let d = UserDefaults(suiteName: Store.group) ?? .standard

    func reset() {
        for k in ["keys", "machines", "pushKey", "pushHandle"] { d.removeObject(forKey: k) }
    }

    var keys: Data {
        if let k = d.data(forKey: "keys"), k.count == 64 { return k }
        let k = generateKeys()
        d.set(k, forKey: "keys")
        return k
    }

    var machines: [SavedMachine] {
        get { (d.data(forKey: "machines")).flatMap { try? JSONDecoder().decode([SavedMachine].self, from: $0) } ?? [] }
        set { d.set(try? JSONEncoder().encode(newValue), forKey: "machines") }
    }

    /// The key every machine seals this phone's notifications with.
    var pushKey: Data {
        if let s = d.string(forKey: "pushKey"), let k = Data(base64Encoded: s), k.count == 32 { return k }
        let k = newPushKey()
        d.set(k.base64EncodedString(), forKey: "pushKey")
        return k
    }

    var pushHandle: String? {
        get { d.string(forKey: "pushHandle") }
        set { d.set(newValue, forKey: "pushHandle") }
    }
}

extension Data {
    var hex: String { map { String(format: "%02x", $0) }.joined() }
}
