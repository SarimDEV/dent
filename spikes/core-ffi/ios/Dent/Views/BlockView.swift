import SwiftUI

/// One block of a reply, drawn from the core's block tree. The same tree
/// feeds the desktop (GPUI) and the web (DOM) renderers.
struct BlockView: View {
    let block: Block
    var cursor = false

    var body: some View {
        content
            .padding(.leading, block.quote > 0 ? 12 : 0)
            .overlay(alignment: .leading) {
                if block.quote > 0 { Rectangle().fill(Theme.hairlineStrong).frame(width: 2) }
            }
    }

    @ViewBuilder private var content: some View {
        switch block.kind {
        case .paragraph:
            reading(Self.text(block.spans, base: .regular), cursor: cursor)
        case let .heading(level):
            reading(Self.text(block.spans, base: level == 1 ? .display : .medium), cursor: cursor)
        case let .item(marker):
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(marker).font(Theme.reading).foregroundStyle(Theme.quiet)
                    .frame(minWidth: 16, alignment: .leading)
                reading(Self.text(block.spans, base: .regular), cursor: cursor)
            }
            .padding(.leading, CGFloat(max(0, Int(block.depth) - 1)) * 20)
        case let .code(lang):
            ZStack(alignment: .topTrailing) {
                ScrollView(.horizontal, showsIndicators: false) {
                    Text(block.text.hasSuffix("\n") ? String(block.text.dropLast()) : block.text)
                        .font(Theme.code)
                        .lineSpacing(Theme.codeSpacing)
                        .foregroundStyle(Theme.ink)
                        .padding(.vertical, 8)
                        .padding(.horizontal, 12)
                        .padding(.trailing, lang.isEmpty ? 0 : 32)
                }
                if !lang.isEmpty {
                    Text(lang).font(Theme.label).foregroundStyle(Theme.faint).padding(.horizontal, 8).padding(.vertical, 6)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: 6).fill(Theme.raised))
            .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(Theme.hairline))
        case .math:
            Text(block.text).font(Theme.code).foregroundStyle(Theme.quiet)
        case .rule:
            Rectangle().fill(Theme.hairline).frame(height: 1)
        case let .row(header):
            let cells = Dictionary(grouping: block.spans, by: \.cell).sorted { $0.key < $1.key }
            HStack(alignment: .top, spacing: 12) {
                ForEach(cells, id: \.key) { _, spans in
                    Text(Self.text(spans, base: .regular, size: 13))
                        .font(header ? Theme.uiMedium : Theme.ui)
                        .foregroundStyle(header ? Theme.quiet : Theme.ink)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            .padding(.vertical, 4)
            .overlay(alignment: .bottom) { Rectangle().fill(Theme.hairline).frame(height: 1) }
        }
    }

    private func reading(_ t: AttributedString, cursor: Bool) -> some View {
        var text = Text(t)
        if cursor {
            // The working square at the end of text still arriving.
            text = text + Text(" ") + Text(Image(systemName: "square.fill"))
                .font(.system(size: 7))
                .foregroundColor(Theme.working)
        }
        return text
            .font(Theme.reading)
            .foregroundStyle(Theme.ink)
            .lineSpacing(Theme.readingSpacing)
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
            .tint(Theme.ink)
    }

    enum Base { case regular, medium, display }

    static func text(_ spans: [Span], base: Base, size: CGFloat = 15) -> AttributedString {
        var out = AttributedString()
        for s in spans {
            var a = AttributedString(s.text)
            let bold = s.bold || base == .medium
            if s.code || s.math {
                a.font = .custom("CommitMono-Regular", size: size - 3, relativeTo: .callout)
                a.backgroundColor = Theme.raised
            } else if base == .display {
                a.font = Theme.displayCompact
            } else {
                let name = switch (bold, s.italic) {
                case (true, true): "Literata-MediumItalic"
                case (true, false): "Literata-Medium"
                case (false, true): "Literata-Italic"
                case (false, false): "Literata-Regular"
                }
                a.font = .custom(name, size: size, relativeTo: .body)
            }
            if s.strike { a.strikethroughStyle = .single }
            if let l = s.link, let u = URL(string: l) {
                a.link = u
                a.underlineStyle = .single
            }
            out += a
        }
        return out
    }
}
