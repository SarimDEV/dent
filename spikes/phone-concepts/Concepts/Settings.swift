import SwiftUI

/// Machines first, then notifications, this iPhone and appearance.
struct Settings: View {
    var limited: Bool
    @Binding var sheet: SheetKind?
    @Environment(\.look) private var look
    @ScaledMetric(relativeTo: .body) private var icon: CGFloat = 28

    var body: some View {
        List {
            if limited {
                Section {
                    HStack(alignment: .firstTextBaseline, spacing: 12) {
                        Mark(status: .waiting).alignmentGuide(.firstTextBaseline) { $0[.bottom] - 1 }
                        VStack(alignment: .leading, spacing: 4) {
                            Text("Notifications paused until 9:00").font(look.ui(.body, .semibold))
                            Text("This iPhone reached today's limit. Everything still shows in the app.")
                                .font(look.ui(.subheadline)).foregroundStyle(look.quiet)
                        }
                    }
                    .padding(.vertical, 4)
                    .listRowBackground(Status.waiting.color.opacity(0.12))
                }
            }
            Section("Machines") {
                ForEach(Fake.machines) { m in
                    NavigationLink(value: Route.machine(m)) {
                        HStack(spacing: 12) {
                            Image(systemName: m.id == "mini" ? "macmini" : (m.id == "devbox" ? "server.rack" : "desktopcomputer"))
                                .foregroundStyle(look.quiet).frame(width: icon)
                            VStack(alignment: .leading, spacing: 3) {
                                Text(m.name).font(look.ui(.body))
                                HStack(spacing: 6) {
                                    Mark(status: m.status)
                                    Text(m.detail).font(look.ui(.footnote)).foregroundStyle(look.quiet)
                                }
                            }
                        }
                    }
                    .listRowBackground(look.row)
                }
                Button { sheet = .add } label: { Label("Add a Machine", systemImage: "plus") }
                    .listRowBackground(look.row)
            }
            Section {
                Toggle("Needs you", isOn: .constant(true))
                Toggle("Finished", isOn: .constant(true))
                Toggle("Errors", isOn: .constant(true))
                Toggle("Usage nearly reached", isOn: .constant(false))
                NavigationLink { EmptyView() } label: { LabeledContent("Per Project", value: "2 changed") }
            } header: {
                Text("Notifications")
            } footer: {
                Text("Sent through dent-push and encrypted so only this iPhone can read them. Free, with a fair-use limit per phone.")
            }
            .listRowBackground(look.row)
            Section("This iPhone") {
                Toggle("Require Face ID", isOn: .constant(false))
                LabeledContent("Device ID") { Text("7df3 cb01 7246").font(look.code) }
            }
            .listRowBackground(look.row)
            Section("Appearance") {
                Picker("Theme", selection: .constant(0)) { Text("Match System").tag(0); Text("Paper").tag(1); Text("Carbon").tag(2) }
                NavigationLink { EmptyView() } label: { LabeledContent("Reading Font", value: "Literata") }
            }
            .listRowBackground(look.row)
            Section {
                LabeledContent("Version", value: "0.1.0")
                Link("Help", destination: URL(string: "https://dent.sh")!)
            }
            .listRowBackground(look.row)
        }
        .font(look.ui(.body))
        .scrollContentBackground(.hidden)
        .background(look.background)
        .navigationTitle("Settings")
    }
}

struct MachineScreen: View {
    let machine: Machine
    @Environment(\.look) private var look
    @State private var confirm = arg("-screen") == "machine-remove"

    var body: some View {
        List {
            Section {
                VStack(spacing: 8) {
                    Image(systemName: "desktopcomputer").font(.system(size: 48, weight: .light)).foregroundStyle(look.quiet)
                    Text(machine.name).font(look.ui(.title2, .semibold))
                    HStack(spacing: 6) {
                        Mark(status: machine.status)
                        Text(machine.detail).font(look.ui(.subheadline)).foregroundStyle(look.quiet)
                    }
                }
                .frame(maxWidth: .infinity)
                .listRowBackground(Color.clear)
            }
            Section("Connection") {
                LabeledContent("Reached through", value: machine.via)
                LabeledContent("Address") { Text(machine.address).font(look.code) }
                LabeledContent("Round trip", value: "18 ms")
            }
            .listRowBackground(look.row)
            Section("On \(machine.name)") {
                LabeledContent("Sessions", value: "5, 2 need you")
                LabeledContent("Agents", value: "claude, codex")
                LabeledContent("dent", value: "0.1.0")
            }
            .listRowBackground(look.row)
            Section {
                Toggle("Notifications", isOn: .constant(true))
            }
            .listRowBackground(look.row)
            Section {
                Button("Remove This iPhone from \(machine.name)", role: .destructive) { confirm = true }
            } footer: {
                Text("\(machine.name) stops accepting this iPhone. Adding it again needs a new code.")
            }
            .listRowBackground(look.row)
        }
        .font(look.ui(.body))
        .scrollContentBackground(.hidden)
        .background(look.background)
        .defaultScrollAnchor(confirm ? .bottom : .top)
        .navigationTitle(machine.name)
        .navigationBarTitleDisplayMode(.inline)
        .confirmationDialog("Remove this iPhone from \(machine.name)?", isPresented: $confirm, titleVisibility: .visible) {
            Button("Remove", role: .destructive) {}
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Its sessions disappear from this iPhone. Nothing on \(machine.name) is deleted.")
        }
    }
}
