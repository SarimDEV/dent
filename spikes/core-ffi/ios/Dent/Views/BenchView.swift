import SwiftUI

/// `-bench -addr HOST:PORT -code CODE`: pair, have the machine log 100,000
/// events, then time a fresh core catching up on them on this device.
/// Results go to stdout and to the file in `DENT_OUT`.
struct BenchView: View {
    @State private var lines: [String] = ["Running…"]

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            ForEach(lines, id: \.self) { Text($0).font(Theme.code).foregroundStyle(Theme.ink) }
        }
        .padding(16)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(Theme.surface)
        .task { await run() }
    }

    @MainActor private func wait(_ limit: Double, _ f: () -> Bool) async -> Bool {
        let end = Date().addingTimeInterval(limit)
        while Date() < end {
            if f() { return true }
            try? await Task.sleep(for: .milliseconds(10))
        }
        return f()
    }

    @MainActor private func run() async {
        var out: [String: Any] = [:]
        let addr = arg("-addr") ?? "127.0.0.1:7480"
        let s = "s-ios-100k"
        let a = MachineModel(id: "bench", name: "studio", addr: addr, key: nil, session: s)
        a.start()
        _ = await wait(5) { a.status == .needsCode }
        a.pair(arg("-code") ?? "")
        let paired = await wait(10) { a.status == .online }
        out["paired"] = paired
        print("DENT_BENCH paired \(paired) status \(a.status)")
        // Asked for once the link is up; if it wasn't quite, ask again.
        a.host.run { l, _ in l.bulk(session: s, n: 100_000) }
        if !(await wait(3) { a.host.link.stats().events > 0 }) {
            print("DENT_BENCH bulk again, status \(a.status)")
            a.host.run { l, _ in l.bulk(session: s, n: 100_000) }
        }
        _ = await wait(60) { a.host.link.stats().events >= 100_000 }
        print("DENT_BENCH logged \(a.host.link.stats().events)")
        let key = a.saved?.key
        a.stop()

        let b = MachineModel(id: "bench2", name: "studio", addr: addr, key: key, session: s)
        let t = Date()
        b.start()
        let ok = await wait(60) {
            b.host.link.stats().events >= 100_000 && (b.messages.first?.blocks.count ?? 0) >= 10_000
        }
        let secs = Date().timeIntervalSince(t)
        let st = b.host.link.stats()
        out["caught_up"] = ok
        out["events"] = st.events
        out["gaps"] = st.gaps
        out["seconds"] = secs
        out["events_per_s"] = Int(Double(st.events) / secs)
        out["drain_ms"] = Int(b.host.drainNanos / 1_000_000)
        out["core_ms"] = Int(b.host.coreNanos / 1_000_000)
        out["frames"] = st.frames
        out["mb"] = Double(st.bytes) / 1e6
        b.stop()

        let md = String(repeating: "## Heading\n\nSome **bold** text with `code` and a [link](https://x).\n\n- one\n- two\n\n```rust\nfn x() {}\n```\n\n", count: 2000)
        let p = Date()
        let blocks = parseMarkdown(text: md)
        out["parse_250kb_ms"] = Int(Date().timeIntervalSince(p) * 1000)
        out["parse_blocks"] = blocks.count

        // The core's sealing, opened with CryptoKit: the vector Rust and
        // Node agree on (spikes/core/push-vector.hex), then a live one.
        let vector = Data(hexString: "0101010101010101010101010768f57ee48944ef19abc8cf5e6b438a2a3ec765e099589c04f0608aac2d9af60c006693a2671ab3")
        out["cryptokit_opens_vector"] = SealedNotice.open(vector, key: Data(repeating: 7, count: 32))?.body == "b"
        let other = try? openNotice(key: newPushKey(), sealed: vector)
        out["core_rejects_wrong_key"] = other == nil

        let json = (try? JSONSerialization.data(withJSONObject: out, options: [.sortedKeys, .prettyPrinted])) ?? Data()
        let text = String(data: json, encoding: .utf8) ?? "{}"
        print("DENT_BENCH \(text)")
        if let path = ProcessInfo.processInfo.environment["DENT_OUT"] {
            try? json.write(to: URL(fileURLWithPath: path))
        }
        lines = text.split(separator: "\n").map(String.init)
    }
}

extension Data {
    init(hexString: String) {
        var d = Data()
        var i = hexString.startIndex
        while i < hexString.endIndex {
            let j = hexString.index(i, offsetBy: 2)
            d.append(UInt8(hexString[i..<j], radix: 16) ?? 0)
            i = j
        }
        self = d
    }
}
