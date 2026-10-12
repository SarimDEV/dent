import SwiftUI
import UIKit

// Drawings of system surfaces: the Lock Screen, a notification's actions,
// the Dynamic Island and widgets. The real ones come from a notification
// content extension, ActivityKit and WidgetKit; these show what dent puts
// in them, laid out the way iOS lays them out.

struct SystemMock: View {
    let screen: String
    var body: some View {
        switch screen {
        case "lock": LockScreen(expanded: false)
        case "notify-actions": LockScreen(expanded: true)
        case "island": IslandAndWidgets()
        default: EmptyView()
        }
    }
}

struct Wallpaper: View {
    var body: some View {
        LinearGradient(colors: [Color(hex: 0x3A4A5C), Color(hex: 0x6B5E57), Color(hex: 0x2A2724)], startPoint: .top, endPoint: .bottom)
            .ignoresSafeArea()
    }
}

extension Color {
    init(hex: UInt32) { self.init(UIColor(hex: hex)) }
}

struct AppIcon: View {
    var size: CGFloat = 38
    var body: some View {
        RoundedRectangle(cornerRadius: size * 0.225, style: .continuous)
            .fill(Color(hex: 0xF1EEE7))
            .overlay(Text("d").font(.custom("InstrumentSerif-Regular", size: size * 0.8)).foregroundStyle(Color(hex: 0x171614)).offset(y: -size * 0.05))
            .overlay(RoundedRectangle(cornerRadius: size * 0.225, style: .continuous).strokeBorder(Color(hex: 0x171614).opacity(0.12)))
            .frame(width: size, height: size)
    }
}

/// The approval as a notification. Collapsed it says who wants what; held,
/// it offers Allow and Deny without opening the app.
struct NotificationCard: View {
    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            AppIcon()
            VStack(alignment: .leading, spacing: 2) {
                HStack {
                    Text("Fix reply tail lost on reconnect").font(.subheadline.weight(.semibold)).lineLimit(1)
                    Spacer(minLength: 8)
                    Text("now").font(.footnote).foregroundStyle(.secondary)
                }
                Text("claude on studio wants to run cargo test -p spike-core").font(.subheadline)
            }
        }
        .padding(14)
        .background(.regularMaterial, in: .rect(cornerRadius: 24, style: .continuous))
    }
}

struct LockScreen: View {
    let expanded: Bool
    var body: some View {
        ZStack {
            Wallpaper()
            VStack(spacing: 0) {
                VStack(spacing: 0) {
                    Text("Monday, October 12").font(.system(size: 20, weight: .semibold))
                    Text("9:41").font(.system(size: 96, weight: .bold, design: .rounded))
                }
                .foregroundStyle(.white.opacity(0.92))
                .padding(.top, 50)
                Spacer()
                if !expanded {
                    VStack(spacing: 10) {
                        NotificationCard()
                        LiveActivity()
                    }
                    .padding(.horizontal, 12)
                    .padding(.bottom, 110)
                }
            }
            if expanded {
                Rectangle().fill(.black.opacity(0.35)).ignoresSafeArea()
                    .background(.ultraThinMaterial)
                VStack(alignment: .leading, spacing: 10) {
                    NotificationCard()
                    VStack(spacing: 0) {
                        Action(title: "Allow", symbol: "checkmark")
                        Divider()
                        Action(title: "Always Allow cargo test", symbol: "checkmark.seal")
                        Divider()
                        Action(title: "Deny", symbol: "xmark", destructive: true)
                    }
                    .background(.regularMaterial, in: .rect(cornerRadius: 14, style: .continuous))
                    .frame(maxWidth: 270)
                }
                .padding(.horizontal, 12)
            }
        }
        .environment(\.colorScheme, .dark)
    }

    struct Action: View {
        let title: String, symbol: String
        var destructive = false
        var body: some View {
            HStack {
                Text(title)
                Spacer()
                Image(systemName: symbol)
            }
            .font(.body)
            .foregroundStyle(destructive ? Color.red : Color.primary)
            .padding(.horizontal, 16).frame(height: 44)
        }
    }
}

/// While anything is running: what needs you, then what's working.
struct LiveActivity: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                AppIcon(size: 22)
                Text("dent").font(.subheadline.weight(.semibold))
                Spacer()
                Text("1 needs you · 2 working").font(.footnote).foregroundStyle(.secondary)
            }
            ForEach(Array(Fake.sessions.prefix(4).filter { $0.id != "s2" }.enumerated()), id: \.offset) { _, s in
                HStack(spacing: 10) {
                    Pixel(status: s.status).frame(width: 10, height: 10)
                    Text(s.title).font(.subheadline).lineLimit(1)
                    Spacer(minLength: 8)
                    Text(s.status == .needs ? "Needs you" : s.age).font(.footnote).foregroundStyle(.secondary).monospacedDigit()
                }
            }
        }
        .padding(16)
        .background(.regularMaterial, in: .rect(cornerRadius: 24, style: .continuous))
    }
}

/// The Dynamic Island (compact and expanded) and the two widgets.
struct IslandAndWidgets: View {
    var body: some View {
        ZStack(alignment: .top) {
            Wallpaper()
            VStack(spacing: 28) {
                // Expanded, as when the person holds the island.
                VStack(alignment: .leading, spacing: 12) {
                    HStack(spacing: 8) {
                        Pixel(status: .needs).frame(width: 12, height: 12)
                        Text("Needs you").font(.footnote.weight(.semibold)).foregroundStyle(.white.opacity(0.7))
                        Spacer()
                        Text("studio").font(.footnote).foregroundStyle(.white.opacity(0.6))
                    }
                    Text("Run cargo test -p spike-core?").font(.headline).foregroundStyle(.white)
                    HStack(spacing: 10) {
                        Button {} label: { Text("Deny").frame(maxWidth: .infinity) }.buttonStyle(.bordered).tint(.white)
                        Button {} label: { Text("Allow").frame(maxWidth: .infinity).foregroundStyle(Color(hex: 0x0F1B2B)) }.buttonStyle(.borderedProminent).tint(Color(hex: 0x5E93D6))
                    }
                    .controlSize(.large)
                }
                .padding(.horizontal, 22).padding(.top, 46).padding(.bottom, 18)
                .background(.black, in: .rect(cornerRadius: 44, style: .continuous))
                .padding(.horizontal, 10)
                .padding(.top, 2)

                // Compact: the needs-you square and a count, either side of the camera.
                VStack(spacing: 8) {
                    HStack {
                        Pixel(status: .needs).frame(width: 12, height: 12)
                        Spacer()
                        Text("1").font(.subheadline.weight(.semibold)).foregroundStyle(.white).monospacedDigit()
                    }
                    .padding(.horizontal, 16)
                    .frame(width: 200, height: 37)
                    .background(.black, in: .capsule)
                    Text("Compact").font(.caption).foregroundStyle(.white.opacity(0.7))
                }

                HStack(alignment: .top, spacing: 20) {
                    UsageWidget()
                    VStack(spacing: 6) {
                        AppIcon(size: 64)
                        Text("dent").font(.caption).foregroundStyle(.white)
                    }
                    .frame(maxWidth: .infinity)
                }
                .padding(.horizontal, 24)
                SessionsWidget().padding(.horizontal, 24)
                Spacer()
            }
        }
    }
}

struct UsageWidget: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Usage").font(.caption.weight(.semibold)).foregroundStyle(.secondary)
            Gauge(value: 0.62) { Text("") } currentValueLabel: { Text("62%").monospacedDigit() }
                .gaugeStyle(.accessoryCircularCapacity)
                .tint(Color(hex: 0x2B5A8C))
            Spacer(minLength: 0)
            Text("Claude · resets Thu").font(.caption2).foregroundStyle(.secondary)
            Text("Codex 18%").font(.caption2.weight(.medium)).monospacedDigit()
        }
        .padding(16)
        .frame(width: 158, height: 158, alignment: .leading)
        .background(Color(hex: 0xFAF8F3), in: .rect(cornerRadius: 22, style: .continuous))
        .environment(\.colorScheme, .light)
    }
}

struct SessionsWidget: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 9) {
            HStack {
                Text("Sessions").font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                Spacer()
                Text("2 need you").font(.caption.weight(.semibold))
            }
            ForEach(Fake.sessions.prefix(4)) { s in
                HStack(spacing: 8) {
                    Pixel(status: s.status).frame(width: 10, height: 10)
                    Text(s.title).font(.footnote).lineLimit(1)
                    Spacer(minLength: 6)
                    Text(s.machine).font(.caption2).foregroundStyle(.secondary)
                }
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, minHeight: 158, alignment: .topLeading)
        .background(Color(hex: 0xFAF8F3), in: .rect(cornerRadius: 22, style: .continuous))
        .environment(\.colorScheme, .light)
    }
}

/// First launch.
struct Welcome: View {
    @Environment(\.look) private var look
    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Spacer().frame(height: 40)
            AppIcon(size: 72)
            Text("Your agents, from your pocket").font(look.title).padding(.top, 24)
            VStack(alignment: .leading, spacing: 22) {
                Feature(symbol: "bell.badge", title: "Hear when they need you", text: "Approve a command from the notification, without opening the app.")
                Feature(symbol: "text.bubble", title: "Every session, live", text: "Read replies as they stream, and send the next step.")
                Feature(symbol: "lock", title: "Straight to your machines", text: "Encrypted end to end. Nothing is stored in between.")
            }
            .padding(.top, 36)
            Spacer()
            VStack(spacing: 12) {
                Button {} label: { Text("Add a Machine").frame(maxWidth: .infinity).foregroundStyle(look.onTint) }
                    .buttonStyle(.borderedProminent)
                Button {} label: { Text("Sign In").frame(maxWidth: .infinity) }
                    .buttonStyle(.borderless)
            }
            .controlSize(.large)
            .font(look.ui(.body, .semibold))
        }
        .padding(.horizontal, 32)
        .padding(.bottom, 16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(look.background)
    }

    struct Feature: View {
        let symbol: String, title: String, text: String
        @Environment(\.look) private var look
        var body: some View {
            HStack(alignment: .top, spacing: 16) {
                Image(systemName: symbol).font(.title2).foregroundStyle(look.tint).frame(width: 36)
                VStack(alignment: .leading, spacing: 2) {
                    Text(title).font(look.ui(.headline, .semibold))
                    Text(text).font(look.ui(.subheadline)).foregroundStyle(look.quiet)
                }
            }
        }
    }
}

/// The first time a machine is added: its sessions arrive while it syncs.
struct Syncing: View {
    @Environment(\.look) private var look
    var body: some View {
        List {
            Section {
                VStack(alignment: .leading, spacing: 8) {
                    HStack {
                        Text("Getting sessions from studio").font(look.ui(.subheadline, .medium))
                        Spacer()
                        Text("38%").font(look.ui(.subheadline)).foregroundStyle(look.quiet).monospacedDigit()
                    }
                    ProgressView(value: 0.38).tint(look.tint)
                    Text("1,812 of 4,800 events. You can start using it now.").font(look.ui(.footnote)).foregroundStyle(look.quiet)
                }
                .padding(.vertical, 4)
                .listRowBackground(look.row)
            }
            Section("Needs you") { ForEach(Fake.needs) { SessionLink(s: $0) } }
            Section("Working") { ForEach(Fake.working.filter { $0.machine == "studio" }) { SessionLink(s: $0) } }
        }
        .scrollContentBackground(.hidden)
        .background(look.background)
        .navigationTitle("Sessions")
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button {} label: { Label("New session", systemImage: "square.and.pencil") }
            }
        }
    }
}
