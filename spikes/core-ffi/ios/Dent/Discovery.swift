import Foundation
import Network

/// Machines announcing dent on this network (Bonjour, `_dent._tcp`), each
/// resolved to an IPv4 address and port for the core to dial.
@MainActor
final class Discovery: ObservableObject {
    struct Found: Identifiable, Equatable {
        let id: String
        let name: String
        var addr: String?
    }

    @Published private(set) var found: [Found] = []

    /// Only machines that answered: an announcement alone can be stale.
    var answering: [Found] { found.filter { $0.addr != nil } }
    private var browser: NWBrowser?
    private var resolving: [String: NWConnection] = [:]

    func start() {
        let b = NWBrowser(for: .bonjourWithTXTRecord(type: "_dent._tcp", domain: nil), using: .tcp)
        b.browseResultsChangedHandler = { [weak self] results, _ in
            MainActor.assumeIsolated { self?.update(results) }
        }
        b.start(queue: .main)
        browser = b
    }

    private func update(_ results: Set<NWBrowser.Result>) {
        var next: [Found] = []
        for r in results {
            guard case let .service(service, _, _, _) = r.endpoint else { continue }
            var id = service, name = service
            if case let .bonjour(txt) = r.metadata {
                id = txt["id"] ?? id
                name = txt["name"] ?? name
            }
            // One machine can be announced on several interfaces.
            guard !next.contains(where: { $0.id == id }) else { continue }
            let addr = found.first { $0.id == id }?.addr
            next.append(Found(id: id, name: name, addr: addr))
            if addr == nil { resolve(id, r.endpoint) }
        }
        found = next.sorted { $0.name < $1.name }
    }

    private func resolve(_ id: String, _ endpoint: NWEndpoint) {
        guard resolving[id] == nil else { return }
        let params = NWParameters.tcp
        if let ip = params.defaultProtocolStack.internetProtocol as? NWProtocolIP.Options {
            ip.version = .v4
        }
        let c = NWConnection(to: endpoint, using: params)
        resolving[id] = c
        c.stateUpdateHandler = { [weak self] state in
            MainActor.assumeIsolated {
                guard let self else { return }
                switch state {
                case .ready:
                    if case let .hostPort(host, port)? = c.currentPath?.remoteEndpoint {
                        let h = "\(host)".split(separator: "%").first.map(String.init) ?? "\(host)"
                        if let i = self.found.firstIndex(where: { $0.id == id }) {
                            self.found[i].addr = "\(h):\(port)"
                        }
                    }
                    c.cancel()
                    self.resolving[id] = nil
                case .failed, .cancelled, .waiting:
                    // Announced but not answering: a machine that died
                    // without saying goodbye. Bonjour keeps it until its
                    // record expires; we don't list it.
                    c.cancel()
                    self.resolving[id] = nil
                default:
                    break
                }
            }
        }
        c.start(queue: .main)
    }
}
