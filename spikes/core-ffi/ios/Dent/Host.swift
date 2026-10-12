import Foundation

/// Runs one core link. It owns the link's WebSockets and its timer, all on
/// one serial queue, and hands the core's notes to the main thread once per
/// frame. The core decides everything; this is the only place with I/O.
///
/// This is the hand-written layer decision 0023 asks for: async work and
/// cancellation stay in Swift, and the core is called synchronously from a
/// single queue, so nothing crosses the FFI while another call is running.
final class Host: NSObject, URLSessionWebSocketDelegate, @unchecked Sendable {
    let link: CoreLink
    private let queue = DispatchQueue(label: "dev.dent.host")
    private var session: URLSession!
    private var sockets: [UInt64: URLSessionWebSocketTask] = [:]
    private var conns: [Int: UInt64] = [:]
    private var timer: DispatchSourceTimer?
    private var drainPending = false
    private let onNotes: @Sendable ([Note]) -> Void
    /// Time spent taking notes (parsing and building blocks), for the bench.
    private(set) var drainNanos: UInt64 = 0
    /// Time inside the core handling frames (decrypt, parse JSON, apply).
    private(set) var coreNanos: UInt64 = 0

    init(link: CoreLink, onNotes: @escaping @Sendable ([Note]) -> Void) {
        self.link = link
        self.onNotes = onNotes
        super.init()
        let ops = OperationQueue()
        ops.underlyingQueue = queue
        ops.maxConcurrentOperationCount = 1
        let cfg = URLSessionConfiguration.ephemeral
        cfg.waitsForConnectivity = false
        session = URLSession(configuration: cfg, delegate: self, delegateQueue: ops)
    }

    static func now() -> UInt64 { UInt64(Date().timeIntervalSince1970 * 1000) }

    /// Call into the core on the host's queue, then do what it asks.
    func run(_ f: @escaping @Sendable (CoreLink, UInt64) -> Void) {
        queue.async {
            f(self.link, Self.now())
            self.pump()
        }
    }

    func start() {
        queue.async {
            // The core's timers are coarse (pings each second, 3 s
            // timeouts), so a steady 100 ms tick is plenty.
            let t = DispatchSource.makeTimerSource(queue: self.queue)
            t.schedule(deadline: .now(), repeating: .milliseconds(100))
            t.setEventHandler { [weak self] in
                guard let self else { return }
                self.link.tick(now: Self.now())
                self.pump()
            }
            t.resume()
            self.timer = t
            self.link.start(now: Self.now())
            self.pump()
        }
    }

    func stop() {
        queue.async {
            self.link.stop()
            self.pump()
            self.timer?.cancel()
            self.timer = nil
        }
    }

    private func pump() {
        for io in link.takeIo() {
            switch io {
            case let .connect(conn, addr):
                guard let url = URL(string: "ws://\(addr)/") else {
                    link.closed(conn: conn, reason: "bad address", now: Self.now())
                    continue
                }
                let task = session.webSocketTask(with: url)
                task.maximumMessageSize = 1 << 20
                sockets[conn] = task
                conns[task.taskIdentifier] = conn
                task.resume()
            case let .send(conn, data):
                // A failed send shows up as the socket closing.
                sockets[conn]?.send(.data(data)) { _ in }
            case let .close(conn):
                if let t = sockets.removeValue(forKey: conn) {
                    conns[t.taskIdentifier] = nil
                    t.cancel(with: .normalClosure, reason: nil)
                }
            }
        }
        scheduleDrain()
    }

    private func receive(_ task: URLSessionWebSocketTask, conn: UInt64) {
        task.receive { [weak self] result in
            guard let self, self.sockets[conn] === task else { return }
            switch result {
            case let .success(.data(d)):
                let t = DispatchTime.now().uptimeNanoseconds
                self.link.received(conn: conn, data: d, now: Self.now())
                self.coreNanos += DispatchTime.now().uptimeNanoseconds - t
                self.pump()
                self.receive(task, conn: conn)
            case .success:
                self.receive(task, conn: conn)
            case let .failure(e):
                self.gone(task, e.localizedDescription)
            }
        }
    }

    private func scheduleDrain() {
        guard !drainPending else { return }
        drainPending = true
        queue.asyncAfter(deadline: .now() + .milliseconds(16)) { [weak self] in
            guard let self else { return }
            self.drainPending = false
            let t = DispatchTime.now().uptimeNanoseconds
            let notes = self.link.takeNotes()
            self.drainNanos += DispatchTime.now().uptimeNanoseconds - t
            if !notes.isEmpty {
                let cb = self.onNotes
                DispatchQueue.main.async { cb(notes) }
            }
        }
    }

    private func gone(_ task: URLSessionTask, _ why: String) {
        guard let conn = conns.removeValue(forKey: task.taskIdentifier),
              sockets.removeValue(forKey: conn) != nil else { return }
        link.closed(conn: conn, reason: why, now: Self.now())
        pump()
    }

    func urlSession(_ s: URLSession, webSocketTask: URLSessionWebSocketTask, didOpenWithProtocol p: String?) {
        guard let conn = conns[webSocketTask.taskIdentifier], sockets[conn] === webSocketTask else { return }
        link.opened(conn: conn, now: Self.now())
        receive(webSocketTask, conn: conn)
        pump()
    }

    func urlSession(_ s: URLSession, webSocketTask: URLSessionWebSocketTask,
                    didCloseWith code: URLSessionWebSocketTask.CloseCode, reason: Data?) {
        gone(webSocketTask, "closed (\(code.rawValue))")
    }

    func urlSession(_ s: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        gone(task, error?.localizedDescription ?? "closed")
    }
}
