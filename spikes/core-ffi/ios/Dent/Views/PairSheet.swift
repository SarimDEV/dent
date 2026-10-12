import SwiftUI

/// Pairing with the code shown on the other machine.
struct PairSheet: View {
    @ObservedObject var m: MachineModel
    var cancel: () -> Void
    @State private var code = ""
    @FocusState private var focused: Bool

    var hint: String {
        switch m.status {
        case .needsCode: return "Enter the 6-digit code shown on \(m.name). Open Settings › Devices there to see it."
        case .pairing: return "Checking the code…"
        case .offline: return "Can't reach \(m.name) right now. It'll keep trying."
        case .online: return "Paired."
        default: return "Connecting to \(m.name)…"
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(spacing: 8) {
                Square(fill: m.line.fill)
                Text("Pair with \(m.name)").font(Theme.displayCompact).foregroundStyle(Theme.ink).lineLimit(1)
                Spacer()
                Button("Cancel", action: cancel).font(Theme.ui).foregroundStyle(Theme.quiet)
            }
            Text(hint).font(Theme.ui).foregroundStyle(Theme.quiet).fixedSize(horizontal: false, vertical: true)

            ZStack {
                // One real field behind six boxes, so paste and one-time
                // code autofill work.
                TextField("", text: $code)
                    .keyboardType(.numberPad)
                    .textContentType(.oneTimeCode)
                    .focused($focused)
                    .opacity(0.02)
                    .accessibilityIdentifier("code")
                HStack(spacing: 8) {
                    ForEach(0..<6, id: \.self) { i in
                        let chars = Array(code)
                        Text(i < chars.count ? String(chars[i]) : "")
                            .font(.custom("CommitMono-Regular", size: 20, relativeTo: .title3))
                            .foregroundStyle(Theme.ink)
                            .frame(width: 40, height: 48)
                            .background(RoundedRectangle(cornerRadius: 6).fill(Theme.raised))
                            .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(
                                focused && i == min(chars.count, 5) ? Theme.ink : Theme.hairlineStrong,
                                lineWidth: focused && i == min(chars.count, 5) ? 2 : 1))
                    }
                }
                .allowsHitTesting(false)
            }
            .frame(height: 48)
            .onTapGesture { focused = true }

            if let e = m.pairError {
                NoticeCard(text: e, signal: Theme.needs)
            }
            Spacer(minLength: 0)
        }
        .padding(16)
        .background(Theme.surface)
        .presentationDetents([.medium])
        .presentationBackground(Theme.surface)
        .onAppear { focused = true }
        .onChange(of: code) { _, c in
            let digits = String(c.filter(\.isNumber).prefix(6))
            if digits != c { code = digits }
            // The core takes a code at any point, even before the machine
            // has answered; only don't start a second attempt mid-check.
            if digits.count == 6 && m.status != .pairing { m.pair(digits) }
        }
        .onChange(of: m.pairError) { _, e in if e != nil { code = "" } }
    }
}
