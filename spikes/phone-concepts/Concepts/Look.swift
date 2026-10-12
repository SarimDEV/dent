import SwiftUI
import UIKit

/// The three visual directions. Structure, components and behaviour are the
/// same in all of them; only colour and type change.
///
/// - system: stock iOS. System font, grouped backgrounds, system tint.
/// - paper: Field Notes colours and Literata for reading; system font for
///   every control, so chrome behaves exactly like iOS.
/// - serif: paper, with Instrument Serif for large titles only.
/// - book: Field Notes colours and the whole Book pairing (Instrument Serif
///   titles, IBM Plex Sans chrome, Literata reading).
enum LookID: String { case system, paper, serif, book }

struct Look {
    let id: LookID

    // MARK: Colour

    var background: Color {
        id == .system ? Color(.systemGroupedBackground) : pair(0xF1EEE7, 0x161513)
    }
    /// Rows, cards and the reading page.
    var row: Color {
        id == .system ? Color(.secondarySystemGroupedBackground) : pair(0xFAF8F3, 0x1F1E1B)
    }
    /// The chat and doc page, which is not grouped.
    var page: Color {
        id == .system ? Color(.systemBackground) : pair(0xF1EEE7, 0x161513)
    }
    var ink: Color { id == .system ? .primary : pair(0x171614, 0xECE8DF) }
    var quiet: Color { id == .system ? .secondary : pair(0x6E6A62, 0x9A958B) }
    var hairline: Color { id == .system ? Color(.separator) : pair(0xDCD7CC, 0x2E2C28) }
    /// Fountain-pen blue: tappable things only. Never a status, never the
    /// needs-you magenta. White labels on it pass in both themes.
    var tint: Color { id == .system ? .accentColor : pair(0x2B5A8C, 0x5E93D6) }
    /// Text and glyphs on a filled tint button: white on the light blue,
    /// deep ink on the dark-mode blue (white there is only 3.2:1).
    var onTint: Color { id == .system ? .white : pair(0xFFFFFF, 0x0F1B2B) }
    /// Big screen titles: Instrument Serif in the serif and book looks.
    var title: Font {
        id == .serif || id == .book ? .custom("InstrumentSerif-Regular", size: 44, relativeTo: .largeTitle) : ui(.largeTitle, .bold)
    }
    /// User messages: a quiet tint, never a signal.
    var bubble: Color { id == .system ? Color(.secondarySystemBackground) : pair(0xE6E1D6, 0x2A2925) }

    // MARK: Type (sizes are Apple's defaults at the Large setting)

    static let sizes: [Font.TextStyle: CGFloat] = [
        .largeTitle: 34, .title: 28, .title2: 22, .title3: 20, .headline: 17, .body: 17,
        .callout: 16, .subheadline: 15, .footnote: 13, .caption: 12, .caption2: 11,
    ]

    /// Interface text: system everywhere except the book direction.
    func ui(_ style: Font.TextStyle, _ weight: Font.Weight = .regular) -> Font {
        guard id == .book else { return .system(style, weight: weight) }
        let name = weight == .regular ? "IBMPlexSans" : (weight == .semibold ? "IBMPlexSans-SmBld" : "IBMPlexSans-Medm")
        return .custom(name, size: Self.sizes[style] ?? 17, relativeTo: style)
    }

    /// Replies, messages and docs.
    var reading: Font {
        id == .system ? .system(.body) : .custom("Literata-Regular", size: 17, relativeTo: .body)
    }
    var readingSpacing: CGFloat { id == .system ? 3 : 5 }
    func heading(_ style: Font.TextStyle) -> Font {
        switch id {
        case .system: .system(style, weight: .semibold)
        case .paper: .custom("Literata-Medium", size: Self.sizes[style] ?? 20, relativeTo: style)
        case .serif, .book: .custom("InstrumentSerif-Regular", size: (Self.sizes[style] ?? 20) + 6, relativeTo: style)
        }
    }
    var code: Font {
        id == .system ? .system(.footnote, design: .monospaced) : .custom("CommitMono-Regular", size: 13, relativeTo: .footnote)
    }

    /// Status shown as a pixel square (Field Notes) or an SF Symbol (stock).
    var squares: Bool { id != .system }

    /// Navigation bar title faces for the book direction.
    func applyAppearance() {
        guard id == .book || id == .serif else { return }
        let large = UIFontMetrics(forTextStyle: .largeTitle).scaledFont(for: UIFont(name: "InstrumentSerif-Regular", size: 40)!)
        UINavigationBar.appearance().largeTitleTextAttributes = [.font: large]
        guard id == .book else { return }
        let inline = UIFontMetrics(forTextStyle: .headline).scaledFont(for: UIFont(name: "IBMPlexSans-Medm", size: 17)!)
        UINavigationBar.appearance().largeTitleTextAttributes = [.font: large]
        UINavigationBar.appearance().titleTextAttributes = [.font: inline]
    }
}

func pair(_ light: UInt32, _ dark: UInt32) -> Color {
    Color(UIColor { $0.userInterfaceStyle == .dark ? UIColor(hex: dark) : UIColor(hex: light) })
}

extension UIColor {
    convenience init(hex: UInt32) {
        self.init(red: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255,
                  blue: CGFloat(hex & 0xFF) / 255, alpha: 1)
    }
}

private struct LookKey: EnvironmentKey { static let defaultValue = Look(id: .paper) }
extension EnvironmentValues {
    var look: Look {
        get { self[LookKey.self] }
        set { self[LookKey.self] = newValue }
    }
}

// MARK: - Status

enum Status: Hashable {
    case needs, working, done, waiting, idle, offline

    var label: String {
        switch self {
        case .needs: "Needs you"
        case .working: "Working"
        case .done: "Done"
        case .waiting: "Paused"
        case .idle: "Idle"
        case .offline: "Offline"
        }
    }
    var color: Color {
        switch self {
        case .needs: pair(0xE0309F, 0xF04FB8)
        case .working: pair(0x2BB8D6, 0x4FD3EC)
        case .done: pair(0x3FBF4F, 0x5FD86B)
        case .waiting: pair(0xE8A317, 0xF0B43A)
        case .idle, .offline: pair(0xA8A398, 0x5E5A53)
        }
    }
    var symbol: String {
        switch self {
        case .needs: "exclamationmark.circle.fill"
        case .working: "circle.dotted.circle"
        case .done: "checkmark.circle.fill"
        case .waiting: "pause.circle.fill"
        case .idle: "circle"
        case .offline: "moon.circle.fill"
        }
    }
}

/// The status mark. Never colour alone: the square has a pattern per status
/// and rows always say the status in words next to it.
///
/// Motion: a working square "drifts" (its checker steps at 8 fps); a square
/// that turns done "confirms" with one small pop. Reduce Motion stills both.
struct Mark: View {
    @Environment(\.look) private var look
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @ScaledMetric(relativeTo: .body) private var side: CGFloat = 10
    var status: Status
    /// Pop once on appearing: a row that just moved to Done is a new view.
    var popOnAppear = false
    @State private var pop: CGFloat = 1

    var body: some View {
        Group {
            if look.squares {
                if status == .working && !reduceMotion {
                    TimelineView(.periodic(from: .now, by: 0.125)) { ctx in
                        Pixel(status: status, phase: Int(ctx.date.timeIntervalSinceReferenceDate * 8) % 2)
                    }
                } else {
                    Pixel(status: status)
                }
            } else {
                Image(systemName: status.symbol)
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(status.color)
                    .symbolEffect(.pulse, isActive: status == .working && !reduceMotion)
            }
        }
        .frame(width: side, height: side)
        .scaleEffect(pop)
        .accessibilityHidden(true)
        .onChange(of: status) { _, new in if new == .done { confirm() } }
        .onAppear { if popOnAppear && status == .done { confirm() } }
    }

    private func confirm() {
        guard !reduceMotion else { return }
        pop = 0.6
        withAnimation(.spring(duration: 0.3, bounce: 0.2)) { pop = 1 }
    }
}

/// A square drawn as a 4×4 pixel pattern, one pattern per status.
struct Pixel: View {
    var status: Status
    var phase = 0
    var body: some View {
        Canvas { ctx, size in
            let c = size.width / 4
            for y in 0..<4 { for x in 0..<4 {
                let on: Bool = switch status {
                case .needs: true                                   // solid
                case .done: x <= y                                  // a corner step
                case .working: (x + y + phase) % 2 == 0             // checker, drifting
                case .waiting: x % 2 == 0 && y % 2 == 0             // dots
                case .idle, .offline: x == 0 || y == 0 || x == 3 || y == 3  // outline
                }
                if on { ctx.fill(Path(CGRect(x: CGFloat(x) * c, y: CGFloat(y) * c, width: c, height: c)), with: .color(status.color)) }
            } }
        }
    }
}
