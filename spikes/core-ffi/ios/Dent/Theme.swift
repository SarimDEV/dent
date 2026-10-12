import SwiftUI
import UIKit

/// Field Notes tokens (docs/design.md, docs/design-system.md). Colours
/// follow light and dark; signals are only ever fills.
enum Theme {
    static func pair(_ light: UInt32, _ dark: UInt32) -> Color {
        Color(UIColor { $0.userInterfaceStyle == .dark ? UIColor(hex: dark) : UIColor(hex: light) })
    }

    static let surface = pair(0xF1EEE7, 0x161513)
    static let raised = pair(0xFAF8F3, 0x1F1E1B)
    static let ink = pair(0x171614, 0xECE8DF)
    static let quiet = pair(0x6E6A62, 0x8F8A80)
    static let faint = pair(0x8C877D, 0x706B63)
    static let hairline = pair(0xDCD7CC, 0x2E2C28)
    static let hairlineStrong = pair(0xC9C3B6, 0x3A3833)
    static let working = pair(0x2BB8D6, 0x4FD3EC)
    static let needs = pair(0xE0309F, 0xF04FB8)
    static let done = pair(0x3FBF4F, 0x5FD86B)
    static let waiting = pair(0xE8A317, 0xF0B43A)

    static func wash(_ c: Color) -> Color { c.opacity(0.09) }
    static func edge(_ c: Color) -> Color { c.opacity(0.45) }

    // Type: five sizes, two weights. Custom fonts still follow Dynamic Type.
    static let display = Font.custom("InstrumentSerif-Regular", size: 28, relativeTo: .largeTitle)
    static let displayCompact = Font.custom("InstrumentSerif-Regular", size: 20, relativeTo: .title3)
    static let reading = Font.custom("Literata-Regular", size: 15, relativeTo: .body)
    static let readingMedium = Font.custom("Literata-Medium", size: 15, relativeTo: .body)
    static let ui = Font.custom("IBMPlexSans", size: 13, relativeTo: .subheadline)
    static let uiMedium = Font.custom("IBMPlexSans-Medm", size: 13, relativeTo: .subheadline)
    static let code = Font.custom("CommitMono-Regular", size: 12, relativeTo: .callout)
    static let label = Font.custom("CommitMono-Regular", size: 11, relativeTo: .caption)

    /// Extra space between lines so 15 pt reading text sits on 24.
    static let readingSpacing: CGFloat = 24 - (UIFont(name: "Literata-Regular", size: 15)?.lineHeight ?? 19)
    static let codeSpacing: CGFloat = 20 - (UIFont(name: "CommitMono-Regular", size: 12)?.lineHeight ?? 15)
}

extension UIColor {
    convenience init(hex: UInt32) {
        self.init(red: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255,
                  blue: CGFloat(hex & 0xFF) / 255, alpha: 1)
    }
}

/// A status square: a signal fill, or an outline when there's nothing to say.
struct Square: View {
    var fill: Color?
    var size: CGFloat = 12
    var body: some View {
        RoundedRectangle(cornerRadius: 2)
            .fill(fill ?? .clear)
            .overlay(RoundedRectangle(cornerRadius: 2).strokeBorder(fill == nil ? Theme.faint : .clear, lineWidth: 1))
            .frame(width: size, height: size)
    }
}

/// An uppercase section label.
struct SectionLabel: View {
    var text: String
    var body: some View {
        Text(text.uppercased())
            .font(Theme.label)
            .kerning(0.66)
            .foregroundStyle(Theme.quiet)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 16)
            .padding(.top, 24)
            .padding(.bottom, 4)
    }
}

/// The primary button: ink, quiet and square-ish.
struct InkButton: ButtonStyle {
    var radius: CGFloat = 6
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(Theme.uiMedium)
            .foregroundStyle(Theme.surface)
            .padding(.horizontal, 12)
            .frame(minHeight: 28)
            .background(RoundedRectangle(cornerRadius: radius).fill(Theme.ink.opacity(configuration.isPressed ? 0.85 : 1)))
            .scaleEffect(configuration.isPressed ? 0.97 : 1)
    }
}

/// A notice on a signal wash, for things worth knowing but not alarming.
struct NoticeCard: View {
    var text: String
    var signal: Color = Theme.waiting
    var body: some View {
        Text(text)
            .font(Theme.ui)
            .foregroundStyle(Theme.ink)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .background(RoundedRectangle(cornerRadius: 8).fill(Theme.wash(signal)))
            .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Theme.edge(signal), lineWidth: 1))
    }
}
