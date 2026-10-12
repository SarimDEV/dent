import SwiftUI

/// Your machines, ones found nearby, and adding one by address.
struct HomeView: View {
    @EnvironmentObject var app: AppModel
    @ObservedObject var discovery: Discovery
    @State private var manual = arg("-addr") ?? ""

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 0) {
                    Text("dent")
                        .font(Theme.display)
                        .foregroundStyle(Theme.ink)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 16)
                        .padding(.top, 8)

                    if !app.machines.isEmpty {
                        SectionLabel(text: "Your machines")
                        ForEach(app.machines) { m in
                            NavigationLink(value: m) { MachineRow(m: m) }
                                .buttonStyle(RowPress())
                            Hairline()
                        }
                        ForEach(app.machines.compactMap(\.pushNotice).prefix(1), id: \.self) { n in
                            NoticeCard(text: n).padding(.horizontal, 16).padding(.top, 12)
                        }
                    }

                    SectionLabel(text: "Found nearby")
                    if app.nearby.isEmpty {
                        Text("Looking for machines running dent on this network…")
                            .font(Theme.ui)
                            .foregroundStyle(Theme.quiet)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.horizontal, 16)
                            .padding(.vertical, 12)
                    }
                    ForEach(app.nearby) { f in
                        Row(fill: nil, name: f.name, sub: "On this network") {
                            Button("Add") {
                                if let a = f.addr { app.add(id: f.id, name: f.name, addr: a) }
                            }
                            .buttonStyle(InkButton())
                            .disabled(f.addr == nil)
                            .accessibilityIdentifier("add-\(f.name)")
                        }
                        Hairline()
                    }

                    SectionLabel(text: "Add by address")
                    HStack(spacing: 8) {
                        TextField("192.168.0.39:7480", text: $manual)
                            .font(Theme.code)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                            .keyboardType(.URL)
                            .padding(.horizontal, 8)
                            .frame(height: 32)
                            .background(RoundedRectangle(cornerRadius: 6).fill(Theme.raised))
                            .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(Theme.hairlineStrong))
                            .accessibilityIdentifier("addr")
                        Button("Connect") {
                            let a = manual.trimmingCharacters(in: .whitespaces)
                            if !a.isEmpty { app.add(id: "addr-\(a)", name: a, addr: a) }
                        }
                        .buttonStyle(InkButton())
                    }
                    .padding(.horizontal, 16)
                    .padding(.vertical, 8)

                    Text("this iPhone · \(deviceId(keys: Store.shared.keys))")
                        .font(Theme.label)
                        .foregroundStyle(Theme.quiet)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 16)
                        .padding(.top, 32)
                        .padding(.bottom, 24)
                }
            }
            .background(Theme.surface)
            .toolbar(.hidden, for: .navigationBar)
            .navigationDestination(for: MachineModel.self) { m in ChatView(m: m) }
            .navigationDestination(item: $app.open) { m in ChatView(m: m) }
        }
        .sheet(item: $app.adding, onDismiss: { app.cancelAdding() }) { m in
            PairSheet(m: m) { app.cancelAdding() }
        }
        .tint(Theme.ink)
    }
}

struct MachineRow: View {
    @ObservedObject var m: MachineModel
    var body: some View {
        Row(fill: m.line.fill, name: m.name, sub: m.line.text) {
            Image(systemName: "chevron.right")
                .font(.system(size: 12, weight: .medium))
                .foregroundStyle(Theme.faint)
        }
    }
}

/// A list row: status square, name over one quiet line, and an action.
struct Row<Trailing: View>: View {
    var fill: Color?
    var name: String
    var sub: String
    @ViewBuilder var trailing: () -> Trailing

    var body: some View {
        HStack(spacing: 12) {
            Square(fill: fill)
            VStack(alignment: .leading, spacing: 0) {
                Text(name).font(Theme.uiMedium).foregroundStyle(Theme.ink).lineLimit(1)
                if !sub.isEmpty {
                    Text(sub).font(Theme.ui).foregroundStyle(Theme.quiet).lineLimit(1)
                }
            }
            Spacer(minLength: 8)
            trailing()
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
        .contentShape(Rectangle())
    }
}

/// Rows tint edge to edge when pressed.
struct RowPress: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label.background(configuration.isPressed ? Theme.ink.opacity(0.09) : .clear)
    }
}

/// A divider inset past the status square.
struct Hairline: View {
    var body: some View {
        Rectangle().fill(Theme.hairline).frame(height: 1).padding(.leading, 40)
    }
}
