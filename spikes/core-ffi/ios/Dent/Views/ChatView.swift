import SwiftUI

/// One machine's session: replies as Markdown blocks, and the composer.
struct ChatView: View {
    @ObservedObject var m: MachineModel
    @Environment(\.dismiss) private var dismiss
    @State private var draft = ""

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 12) {
                Button { dismiss() } label: {
                    Image(systemName: "chevron.left").font(.system(size: 15, weight: .medium))
                        .foregroundStyle(Theme.ink).frame(width: 28, height: 44)
                }
                .accessibilityLabel("Machines")
                Square(fill: m.line.fill)
                VStack(alignment: .leading, spacing: 0) {
                    Text(m.name).font(Theme.uiMedium).foregroundStyle(Theme.ink).lineLimit(1)
                    Text(m.line.text).font(Theme.ui).foregroundStyle(Theme.quiet).lineLimit(1)
                }
                Spacer()
            }
            .padding(.leading, 8)
            .padding(.trailing, 16)
            .frame(minHeight: 52)
            .overlay(alignment: .bottom) { Rectangle().fill(Theme.hairline).frame(height: 1) }

            if let n = m.pushNotice {
                NoticeCard(text: n).padding(.horizontal, 16).padding(.top, 12)
            }

            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 16) {
                        if m.messages.isEmpty && m.pending.isEmpty {
                            Text("Ask \(m.name) anything. Replies stream here and carry on if the connection drops.")
                                .font(Theme.reading)
                                .foregroundStyle(Theme.quiet)
                                .lineSpacing(Theme.readingSpacing)
                                .padding(.top, 32)
                        }
                        ForEach(m.messages) { msg in MessageView(msg: msg) }
                        ForEach(m.pending) { p in
                            VStack(alignment: .leading, spacing: 4) {
                                Text(p.text).font(Theme.reading).foregroundStyle(Theme.quiet)
                                Text(m.status == .online ? "sending…" : "waiting for \(m.name)")
                                    .font(Theme.label).foregroundStyle(Theme.quiet)
                            }
                            .card()
                        }
                        Color.clear.frame(height: 1).id("end")
                    }
                    .padding(.horizontal, 16)
                    .padding(.vertical, 24)
                }
                .scrollDismissesKeyboard(.interactively)
                .onChange(of: m.messages) { _, _ in proxy.scrollTo("end", anchor: .bottom) }
                .onChange(of: m.pending) { _, _ in proxy.scrollTo("end", anchor: .bottom) }
            }

            HStack(alignment: .bottom, spacing: 8) {
                TextField("Ask \(m.name)…", text: $draft, axis: .vertical)
                    .font(Theme.reading)
                    .foregroundStyle(Theme.ink)
                    .lineLimit(1...5)
                    .padding(.vertical, 2)
                    .padding(.horizontal, 4)
                    .accessibilityIdentifier("composer")
                Button("Send") {
                    let t = draft.trimmingCharacters(in: .whitespacesAndNewlines)
                    guard !t.isEmpty else { return }
                    m.send(t)
                    draft = ""
                }
                .buttonStyle(InkButton(radius: 4))
                .disabled(draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                .accessibilityIdentifier("send")
            }
            .padding(8)
            .background(RoundedRectangle(cornerRadius: 12).fill(Theme.raised))
            .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Theme.hairline))
            .padding(.horizontal, 16)
            .padding(.bottom, 8)
        }
        .background(Theme.surface)
        .toolbar(.hidden, for: .navigationBar)
    }
}

struct MessageView: View {
    let msg: MachineModel.Message

    var body: some View {
        let streaming = msg.role == .agent && msg.state == .streaming
        VStack(alignment: .leading, spacing: 12) {
            ForEach(Array(msg.blocks.enumerated()), id: \.offset) { i, b in
                BlockView(block: b, cursor: streaming && i == msg.blocks.count - 1)
            }
            if streaming && msg.blocks.isEmpty {
                Square(fill: Theme.working, size: 8)
            }
            if msg.role == .agent && msg.state != .streaming {
                HStack(spacing: 6) {
                    Square(fill: msg.state == .done ? Theme.done : Theme.needs, size: 8)
                    Text(msg.state == .done ? "Done" : "Interrupted · the machine stopped mid-reply")
                        .font(Theme.label).foregroundStyle(Theme.quiet)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .modifier(UserCard(on: msg.role == .user))
    }
}

struct UserCard: ViewModifier {
    var on: Bool
    func body(content: Content) -> some View {
        if on { content.card() } else { content }
    }
}

extension View {
    func card() -> some View {
        frame(maxWidth: .infinity, alignment: .leading)
            .padding(12)
            .background(RoundedRectangle(cornerRadius: 8).fill(Theme.raised))
            .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Theme.hairline))
    }
}
