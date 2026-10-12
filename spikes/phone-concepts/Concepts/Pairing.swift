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

/// Starting a session is just typing. Where it runs, the model, permissions
/// and worktree sit under the composer, already chosen: the least busy
/// machine, and whatever you picked last time. The project fills in from
/// what you type. Tap any of them to change it.
struct NewSession: View {
    var typed: Bool
    @Environment(\.look) private var look
    @ScaledMetric(relativeTo: .body) private var control: CGFloat = 44
    @Environment(\.dynamicTypeSize) private var size
    @State private var draft: String
    @FocusState private var focused: Bool

    init(typed: Bool) {
        self.typed = typed
        _draft = State(initialValue: typed ? "The usage line under Sessions cuts off at large text sizes. Make it wrap instead." : "")
    }

    var body: some View {
        ScrollView { Color.clear.frame(height: 1) }
            .background(look.page)
            .navigationTitle("New Session")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button(role: .cancel) {} }
            }
            .safeAreaBar(edge: .bottom) {
                VStack(alignment: .leading, spacing: 10) {
                    HStack(alignment: .bottom, spacing: 6) {
                        TextField("What should it do?", text: $draft, axis: .vertical)
                            .font(look.reading)
                            .lineLimit(size.isAccessibilitySize ? 1...3 : 1...8)
                            .focused($focused)
                            .padding(.leading, 16).padding(.vertical, 11)
                        if !draft.isEmpty {
                            Button {} label: {
                                Image(systemName: "arrow.up").font(.body.weight(.bold)).foregroundStyle(look.onTint)
                                    .frame(width: min(control - 12, 44), height: min(control - 12, 44))
                            }
                            .buttonStyle(.borderedProminent)
                            .buttonBorderShape(.circle)
                            .accessibilityLabel("Start")
                            .padding(6)
                        }
                    }
                    .glassEffect(.regular.interactive(), in: .rect(cornerRadius: 22))
                    Defaults(project: typed ? "dent" : nil)
                }
                .padding(.horizontal, 16)
                .padding(.bottom, 4)
            }
            .onAppear { focused = true }
    }
}

/// The session's settings as small menus under the composer, wrapping onto
/// a second line. At accessibility sizes they fold into one menu.
struct Defaults: View {
    var project: String?
    @Environment(\.look) private var look
    @Environment(\.dynamicTypeSize) private var size

    var body: some View {
        if size.isAccessibilitySize {
            Menu {
                projectPicker; machinePicker; modelPicker; accessPicker; worktreePicker
            } label: {
                Text("\(project ?? "Project from message") · studio · Opus 5.5 · Full access · New worktree")
                    .font(look.ui(.footnote)).multilineTextAlignment(.leading)
            }
            .tint(look.tint)
            .padding(.horizontal, 4)
        } else {
            Flow(spacing: 8, lineSpacing: 8) {
                Menu { projectPicker } label: { Label(project ?? "Project from message", systemImage: "sparkles") }
                Menu { machinePicker } label: { Label("studio", systemImage: "desktopcomputer") }
                Menu { modelPicker } label: { Text("Opus 5.5") }
                Menu { accessPicker } label: { Label("Full access", systemImage: "lock.open") }
                Menu { worktreePicker } label: { Label("New worktree", systemImage: "arrow.triangle.branch") }
            }
            .buttonStyle(.bordered)
            .buttonBorderShape(.capsule)
            .controlSize(.regular)
            .font(look.ui(.subheadline, .medium))
            .tint(look.ink)
        }
    }

    var projectPicker: some View {
        Picker("Project", selection: .constant(project ?? "auto")) {
            Text("From Message").tag("auto")
            ForEach(Fake.projects, id: \.self) { Text($0).tag($0) }
            Text("New Project").tag("new")
        }
    }
    var machinePicker: some View {
        Picker("Machine", selection: .constant("auto")) {
            Text("Least Busy (studio)").tag("auto")
            Text("studio · 2 working").tag("studio")
            Text("mac-mini · 1 working").tag("mini")
        }
    }
    var modelPicker: some View {
        Picker("Model", selection: .constant("opus")) {
            Text("Opus 5.5").tag("opus"); Text("Sonnet 5.5").tag("sonnet"); Text("Astra 6.1").tag("astra")
        }
    }
    var accessPicker: some View {
        Picker("Permissions", selection: .constant("full")) {
            Text("Ask Each Time").tag("ask"); Text("Auto").tag("auto"); Text("Full Access").tag("full")
        }
    }
    var worktreePicker: some View {
        Picker("Worktree", selection: .constant("new")) {
            Text("New Worktree").tag("new"); Text("Main Checkout").tag("main")
        }
    }
}

/// Lays children out left to right, wrapping to new lines.
struct Flow: Layout {
    var spacing: CGFloat = 8
    var lineSpacing: CGFloat = 8
    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let rows = arrange(width: proposal.width ?? .infinity, subviews)
        let height = rows.map(\.height).reduce(0, +) + lineSpacing * CGFloat(max(rows.count - 1, 0))
        return CGSize(width: proposal.width ?? rows.map(\.width).max() ?? 0, height: height)
    }
    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var y = bounds.minY
        for row in arrange(width: bounds.width, subviews) {
            var x = bounds.minX
            for i in row.items {
                let s = subviews[i].sizeThatFits(.unspecified)
                subviews[i].place(at: CGPoint(x: x, y: y), proposal: ProposedViewSize(s))
                x += s.width + spacing
            }
            y += row.height + lineSpacing
        }
    }
    private func arrange(width: CGFloat, _ subviews: Subviews) -> [(items: [Int], width: CGFloat, height: CGFloat)] {
        var rows: [(items: [Int], width: CGFloat, height: CGFloat)] = [([], 0, 0)]
        for (i, v) in subviews.enumerated() {
            let s = v.sizeThatFits(.unspecified)
            let w = rows[rows.count - 1].width
            if !rows[rows.count - 1].items.isEmpty && w + spacing + s.width > width { rows.append(([], 0, 0)) }
            let last = rows.count - 1
            rows[last].width += (rows[last].items.isEmpty ? 0 : spacing) + s.width
            rows[last].height = max(rows[last].height, s.height)
            rows[last].items.append(i)
        }
        return rows
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
