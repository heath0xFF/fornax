import SwiftUI

struct MarkdownText: View {
    let content: String

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            ForEach(Array(parse(content).enumerated()), id: \.offset) { _, seg in
                switch seg {
                case .prose(let text):
                    ProseText(text)
                case .code(let lang, let code):
                    CodeFenceView(language: lang, code: code)
                case .header(let level, let text):
                    HeaderText(level: level, text: text)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    enum Segment { case prose(String); case code(String?, String); case header(Int, String) }

    private func parse(_ raw: String) -> [Segment] {
        splitCodeFences(raw).flatMap { seg -> [Segment] in
            if case .prose(let text) = seg { return splitHeaders(text) }
            return [seg]
        }
    }

    private func splitCodeFences(_ raw: String) -> [Segment] {
        var result: [Segment] = []
        var remaining = raw

        while let fenceRange = remaining.range(of: "```") {
            let prose = String(remaining[..<fenceRange.lowerBound])
                .trimmingCharacters(in: .init(charactersIn: "\n"))
            if !prose.isEmpty { result.append(.prose(prose)) }
            remaining = String(remaining[fenceRange.upperBound...])

            var lang: String?
            if let nl = remaining.firstIndex(of: "\n") {
                let candidate = String(remaining[..<nl]).trimmingCharacters(in: .whitespaces)
                if !candidate.isEmpty { lang = candidate }
                remaining = String(remaining[remaining.index(after: nl)...])
            }

            if let closeRange = remaining.range(of: "```") {
                var code = String(remaining[..<closeRange.lowerBound])
                if code.hasSuffix("\n") { code = String(code.dropLast()) }
                result.append(.code(lang, code))
                remaining = String(remaining[closeRange.upperBound...])
                if remaining.hasPrefix("\n") { remaining = String(remaining.dropFirst()) }
            } else {
                result.append(.code(lang, remaining))
                remaining = ""
            }
        }

        let tail = remaining.trimmingCharacters(in: .init(charactersIn: "\n"))
        if !tail.isEmpty { result.append(.prose(tail)) }
        return result.isEmpty ? [.prose(raw)] : result
    }

    private func splitHeaders(_ text: String) -> [Segment] {
        var result: [Segment] = []
        var proseLines: [String] = []

        for line in text.components(separatedBy: "\n") {
            if let level = headingLevel(line) {
                if !proseLines.isEmpty {
                    let prose = proseLines.joined(separator: "\n")
                        .trimmingCharacters(in: .init(charactersIn: "\n"))
                    if !prose.isEmpty { result.append(.prose(prose)) }
                    proseLines = []
                }
                let headingText = String(line.dropFirst(level + 1))
                    .trimmingCharacters(in: .whitespaces)
                result.append(.header(level, headingText))
            } else {
                proseLines.append(line)
            }
        }

        if !proseLines.isEmpty {
            let prose = proseLines.joined(separator: "\n")
                .trimmingCharacters(in: .init(charactersIn: "\n"))
            if !prose.isEmpty { result.append(.prose(prose)) }
        }

        return result.isEmpty ? [.prose(text)] : result
    }

    private func headingLevel(_ line: String) -> Int? {
        var level = 0
        for ch in line { if ch == "#" { level += 1 } else { break } }
        guard level >= 1, level <= 6 else { return nil }
        return line.dropFirst(level).hasPrefix(" ") ? level : nil
    }
}

private struct ProseText: View {
    let raw: String
    init(_ raw: String) { self.raw = raw }

    var body: some View {
        Group {
            if let attr = try? AttributedString(
                markdown: raw,
                options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)
            ) {
                Text(attr)
            } else {
                Text(raw)
            }
        }
        .textSelection(.enabled)
    }
}

private struct HeaderText: View {
    let level: Int
    let text: String

    var body: some View {
        Text(text)
            .font(font)
            .fontWeight(.semibold)
            .textSelection(.enabled)
            .padding(.top, level <= 2 ? 6 : 2)
    }

    private var font: Font {
        switch level {
        case 1: return .title2
        case 2: return .title3
        case 3: return .headline
        default: return .subheadline
        }
    }
}

struct CodeFenceView: View {
    let language: String?
    let code: String

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if let lang = language {
                Text(lang)
                    .font(.system(size: 11, weight: .medium, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 12)
                    .padding(.top, 8)
                    .padding(.bottom, 2)
            }
            ScrollView(.horizontal, showsIndicators: false) {
                Text(code)
                    .font(.system(size: 13, design: .monospaced))
                    .foregroundStyle(.primary)
                    .padding(.horizontal, 12)
                    .padding(.vertical, language == nil ? 10 : 6)
                    .textSelection(.enabled)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(.systemFill))
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .overlay(
            RoundedRectangle(cornerRadius: 8)
                .strokeBorder(Color(.separator), lineWidth: 0.5)
        )
    }
}

struct TypingIndicator: View {
    @State private var animating = false

    var body: some View {
        HStack(spacing: 5) {
            ForEach(0..<3, id: \.self) { i in
                Circle()
                    .frame(width: 7, height: 7)
                    .foregroundStyle(Color.secondary)
                    .scaleEffect(animating ? 1.15 : 0.75)
                    .opacity(animating ? 0.9 : 0.35)
                    .animation(
                        .easeInOut(duration: 0.55)
                        .repeatForever(autoreverses: true)
                        .delay(Double(i) * 0.18),
                        value: animating
                    )
            }
        }
        .onAppear { animating = true }
        .onDisappear { animating = false }
    }
}
