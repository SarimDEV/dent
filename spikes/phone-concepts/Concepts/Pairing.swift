import SwiftUI

/// Adding a machine, start to finish, in one sheet: pick it (found nearby,
/// QR code or address), confirm with its code, then notifications.
struct PairingFlow: View {
    let start: SheetKind
    @Binding var sheet: SheetKind?
    @State private var path: [String]

    init(start: SheetKind, sheet: Binding<SheetKind?>) {
        self.start = start
        _sheet = sheet
        _path = State(initialValue: start == .code ? ["code"] : (start == .paired ? ["code", "paired"] : []))
    }

    var body: some View {
        NavigationStack(path: $path) {
            Group {
                switch start {
                case .qr: ScanCode()
                case .new: NewSession()
                default: AddMachine()
                }
            }
            .navigationDestination(for: String.self) { step in
                switch step {
                case "code": EnterCode()
                case "paired": Paired().navigationBarBackButtonHidden()
                default: ScanCode()
                }
            }
        }
        .presentationDetents([.large])
    }
}

struct AddMachine: View {
    @Environment(\.look) private var look
    @ScaledMetric(relativeTo: .body) private var icon: CGFloat = 28
    var body: some View {
        List {
            Section {
                ForEach(Fake.machines.prefix(2)) { m in
                    NavigationLink(value: "code") {
                        HStack(spacing: 12) {
                            Image(systemName: m.id == "studio" ? "desktopcomputer" : "macmini").foregroundStyle(look.quiet).frame(width: icon)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(m.name).font(look.ui(.body))
                                Text(m.id == "studio" ? "On this network" : "Through Tailscale").font(look.ui(.footnote)).foregroundStyle(look.quiet)
                            }
                        }
                    }
                    .listRowBackground(look.row)
                }
            } header: {
                HStack(spacing: 6) {
                    Text("Found nearby")
                    ProgressView().controlSize(.mini)
                }
            }
            Section {
                NavigationLink(value: "qr") { Label("Scan QR Code", systemImage: "qrcode.viewfinder") }
                    .listRowBackground(look.row)
                NavigationLink(value: "code") { Label("Enter Address", systemImage: "keyboard") }
                    .listRowBackground(look.row)
            } footer: {
                Text("Can't see your computer? dent needs to be running on it, and it needs to be on this network or on your Tailscale network.")
            }
        }
        .font(look.ui(.body))
        .scrollContentBackground(.hidden)
        .background(look.background)
        .navigationTitle("Add a Machine")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .cancellationAction) { Button(role: .cancel) {} }
        }
    }
}

/// The one-time code, in one system text field that autofills and pastes.
struct EnterCode: View {
    @Environment(\.look) private var look
    @State private var code = "4829"
    @FocusState private var focused: Bool

    var body: some View {
        ScrollView {
            VStack(spacing: 20) {
                Text("Enter the code from studio").font(look.ui(.title2, .semibold)).multilineTextAlignment(.center)
                    .padding(.top, 16)
                TextField("", text: $code, prompt: Text("6-digit code"))
                    .textContentType(.oneTimeCode)
                    .keyboardType(.numberPad)
                    .font(.system(.largeTitle, design: .monospaced, weight: .medium))
                    .kerning(4)
                    .multilineTextAlignment(.center)
                    .padding(.vertical, 14)
                    .background(look.row, in: .rect(cornerRadius: 16))
                    .overlay(RoundedRectangle(cornerRadius: 16).strokeBorder(look.hairline))
                    .focused($focused)
                    .accessibilityLabel("Pairing code")
                Text("On studio, open dent and choose Settings › Devices › Add a Device. The code works once.")
                    .font(look.ui(.subheadline)).foregroundStyle(look.quiet).multilineTextAlignment(.center)
            }
            .padding(.horizontal, 24)
        }
        .background(look.background)
        .safeAreaInset(edge: .bottom) {
            NavigationLink(value: "qr") { Label("Scan QR Code Instead", systemImage: "qrcode.viewfinder").font(look.ui(.subheadline, .medium)) }
                .frame(maxWidth: .infinity, minHeight: 44)
                .padding(.vertical, 8)
                .background(look.background)
        }
        .navigationTitle("studio")
        .navigationBarTitleDisplayMode(.inline)
        .onAppear { focused = true }
    }
}

/// Scanning the QR code dent shows on the computer. The camera is mocked.
struct ScanCode: View {
    @Environment(\.look) private var look
    var body: some View {
        VStack(spacing: 0) {
            ZStack {
                LinearGradient(colors: [Color(white: 0.16), Color(white: 0.08)], startPoint: .top, endPoint: .bottom)
                Viewfinder().stroke(.white, style: StrokeStyle(lineWidth: 5, lineCap: .round, lineJoin: .round))
                    .frame(width: 240, height: 240)
                Text("Camera").font(.footnote).foregroundStyle(.white.opacity(0.35)).offset(y: 150)
            }
            .ignoresSafeArea(edges: .top)
            VStack(spacing: 12) {
                Text("Point at the QR code on your computer").font(look.ui(.headline, .semibold)).multilineTextAlignment(.center)
                Text("In dent on the computer: Settings › Devices › Add a Device.")
                    .font(look.ui(.subheadline)).foregroundStyle(look.quiet).multilineTextAlignment(.center)
                NavigationLink(value: "code") { Text("Enter Code Instead").font(look.ui(.body)) }
                    .padding(.top, 4)
            }
            .padding(24)
            .frame(maxWidth: .infinity)
            .background(look.background)
        }
        .preferredColorScheme(.dark)
        .navigationTitle("Scan QR Code")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .cancellationAction) { Button(role: .cancel) {} }
        }
    }
}

/// Paired, and the one moment to ask for notifications, with a reason.
struct Paired: View {
    @Environment(\.look) private var look
    var body: some View {
        VStack(spacing: 20) {
            Spacer()
            Image(systemName: "checkmark.circle.fill")
                .font(.system(size: 60))
                .foregroundStyle(Status.done.color)
            VStack(spacing: 8) {
                Text("studio is added").font(look.ui(.title2, .semibold))
                Text("Its sessions are in your list now. Turn on notifications to hear when it needs you or finishes, even with the app closed.")
                    .font(look.ui(.body)).foregroundStyle(look.quiet).multilineTextAlignment(.center)
            }
            Spacer()
            VStack(spacing: 12) {
                Button {} label: { Text("Turn On Notifications").frame(maxWidth: .infinity).foregroundStyle(look.onTint) }
                    .buttonStyle(.borderedProminent)
                Button {} label: { Text("Not Now").frame(maxWidth: .infinity) }
                    .buttonStyle(.borderless)
            }
            .controlSize(.large)
            .font(look.ui(.body, .semibold))
        }
        .padding(24)
        .background(look.background)
    }
}

/// Starting a session from the phone.
struct NewSession: View {
    @Environment(\.look) private var look
    @State private var text = "Add a test that cuts the link mid-frame and check the reply still arrives whole."
    var body: some View {
        Form {
            Section {
                Picker("Project", selection: .constant("dent")) { ForEach(Fake.projects, id: \.self) { Text($0) } }
                Picker("Machine", selection: .constant("studio")) { ForEach(Fake.machines) { Text($0.name).tag($0.name) } }
                Picker("Agent", selection: .constant("claude")) { Text("claude").tag("claude"); Text("codex").tag("codex") }
            }
            .listRowBackground(look.row)
            Section("Message") {
                TextField("What should it do?", text: $text, axis: .vertical)
                    .font(look.reading)
                    .lineLimit(5...12)
            }
            .listRowBackground(look.row)
        }
        .font(look.ui(.body))
        .scrollContentBackground(.hidden)
        .background(look.background)
        .navigationTitle("New Session")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .cancellationAction) { Button(role: .cancel) {} }
            ToolbarItem(placement: .confirmationAction) {
                Button {} label: { Label("Start", systemImage: "arrow.up").foregroundStyle(look.onTint) }.buttonStyle(.glassProminent)
            }
        }
    }
}

/// Four rounded corner brackets, the system's scanner shape.
struct Viewfinder: Shape {
    func path(in r: CGRect) -> Path {
        var p = Path()
        let l: CGFloat = 44, c: CGFloat = 22
        for (x, y, sx, sy) in [(r.minX, r.minY, 1.0, 1.0), (r.maxX, r.minY, -1.0, 1.0), (r.minX, r.maxY, 1.0, -1.0), (r.maxX, r.maxY, -1.0, -1.0)] {
            p.move(to: CGPoint(x: x, y: y + sy * l))
            p.addLine(to: CGPoint(x: x, y: y + sy * c))
            p.addQuadCurve(to: CGPoint(x: x + sx * c, y: y), control: CGPoint(x: x, y: y))
            p.addLine(to: CGPoint(x: x + sx * l, y: y))
        }
        return p
    }
}
