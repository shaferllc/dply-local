import SwiftUI

/// A colored capsule for a status string. A leading dot + soft tinted pill
/// reads cleanly against both list rows and detail headers.
struct StatusBadge: View {
    let status: String

    private var color: Color {
        switch status.lowercased() {
        case let s where s.contains("fail") || s.contains("error") || s.contains("denied")
            || s.contains("stopped"):
            return .red
        case let s where s.contains("build") || s.contains("queue") || s.contains("pending")
            || s.contains("progress"):
            return .orange
        case let s where s.contains("live") || s.contains("active") || s.contains("running")
            || s.contains("ready") || s.contains("success") || s.contains("serving")
            || s.contains("ok"):
            return Theme.live
        default:
            return .gray
        }
    }

    var body: some View {
        if status.isEmpty {
            EmptyView()
        } else {
            HStack(spacing: 5) {
                Circle().fill(color).frame(width: 6, height: 6)
                Text(status)
                    .font(.caption2.weight(.semibold))
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .background(color.opacity(0.14), in: Capsule())
            .foregroundStyle(color)
        }
    }
}

/// A master-list row: a gradient icon tile, title + subtitle, trailing badge.
struct ItemRow: View {
    let title: String
    let subtitle: String
    let status: String
    var systemImage: String = "globe"
    /// Dim the tile when the item isn't active/serving.
    var active: Bool = true

    var body: some View {
        HStack(spacing: 11) {
            GradientTile(systemImage: systemImage, size: 30, active: active)
            VStack(alignment: .leading, spacing: 2) {
                Text(title.isEmpty ? "—" : title)
                    .font(.body.weight(.semibold))
                    .lineLimit(1)
                if !subtitle.isEmpty {
                    Text(subtitle)
                        .font(.system(.caption, design: .monospaced))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
            }
            Spacer(minLength: 8)
            StatusBadge(status: status)
        }
        .padding(.vertical, 4)
    }
}

/// A rich domain-list row (PhpMon-style): TLS state, host, detected project
/// type + PHP version, and whether it's serving. Used for local `.test` sites.
struct DomainRow: View {
    let site: Row

    var body: some View {
        let secure = site.dig("secure") == .bool(true)
        let serving = site.dig("serving") == .bool(true)
        let isProxy = site.cell(["source"]) == "proxy"
        let php = site.cell(["php"])
        let framework = site.cell(["framework"])
        let runtime = site.cell(["runtime"])

        HStack(spacing: 10) {
            Image(systemName: secure ? "lock.fill" : "lock.open")
                .font(.caption2)
                .foregroundStyle(secure ? AnyShapeStyle(.green) : AnyShapeStyle(.tertiary))
                .frame(width: 12)
            VStack(alignment: .leading, spacing: 2) {
                Text(site.cell(["host"])).font(.body.weight(.semibold)).lineLimit(1)
                Text(subtitle(isProxy: isProxy, framework: framework, php: php, runtime: runtime))
                    .font(.system(.caption2, design: .monospaced))
                    .foregroundStyle(.secondary).lineLimit(1).truncationMode(.tail)
            }
            Spacer(minLength: 6)
            Image(systemName: isProxy ? "arrow.triangle.branch" : "link")
                .font(.caption2).foregroundStyle(.tertiary)
            Circle().fill(serving ? Color.green : Color.secondary.opacity(0.4)).frame(width: 7, height: 7)
        }
        .padding(.vertical, 3)
    }

    private func subtitle(isProxy: Bool, framework: String, php: String, runtime: String) -> String {
        if isProxy { return "proxy → \(site.cell(["path"]))" }
        var parts: [String] = []
        if !framework.isEmpty { parts.append(framework) }
        parts.append(php.isEmpty ? "PHP default" : "PHP \(php)")
        if !runtime.isEmpty && runtime != "fpm" {
            parts.append(runtime.replacingOccurrences(of: "octane-", with: "⚡"))
        }
        return parts.joined(separator: "  ·  ")
    }
}

/// A labeled key/value list rendered from a Row and a column spec, skipping
/// empty values.
struct KeyValueView: View {
    let row: Row
    let fields: [(String, [String])]

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            ForEach(fields, id: \.0) { label, keys in
                let value = row.cell(keys)
                if !value.isEmpty {
                    HStack(alignment: .top, spacing: 12) {
                        Text(label)
                            .font(.callout)
                            .foregroundStyle(.secondary)
                            .frame(width: 130, alignment: .leading)
                        Text(value)
                            .font(.callout)
                            .textSelection(.enabled)
                        Spacer(minLength: 0)
                    }
                }
            }
        }
    }
}

/// A compact grid of Rows given column specs — stacks (not the dynamic-column
/// `Table` API) so it renders identically on every supported macOS.
struct RowsTable: View {
    let rows: [Row]
    let columns: [(String, [String])]

    var body: some View {
        if rows.isEmpty {
            Text("None.")
                .foregroundStyle(.secondary)
                .font(.callout)
                .padding(.vertical, 4)
        } else {
            VStack(alignment: .leading, spacing: 0) {
                gridRow(cells: columns.map { $0.0 }, header: true)
                Divider()
                ScrollView(.vertical) {
                    VStack(alignment: .leading, spacing: 0) {
                        ForEach(Array(rows.enumerated()), id: \.offset) { idx, row in
                            gridRow(cells: columns.map { row.cell($0.1) }, header: false)
                                .background(idx.isMultiple(of: 2) ? Color.clear : Color.primary.opacity(0.03))
                        }
                    }
                }
                .frame(maxHeight: 220)
            }
        }
    }

    private func gridRow(cells: [String], header: Bool) -> some View {
        HStack(spacing: 12) {
            ForEach(Array(cells.enumerated()), id: \.offset) { _, cell in
                Text(cell)
                    .font(header ? .caption.weight(.semibold) : .system(.callout, design: .monospaced))
                    .foregroundStyle(header ? AnyShapeStyle(.secondary) : AnyShapeStyle(.primary))
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .padding(.vertical, 5)
        .padding(.horizontal, 4)
    }
}

/// A dismissible error banner.
struct ErrorBanner: View {
    let message: String
    var onDismiss: () -> Void

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(.orange)
            Text(message)
                .font(.callout)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 8)
            Button(action: onDismiss) {
                Image(systemName: "xmark.circle.fill").foregroundStyle(.secondary)
            }
            .buttonStyle(.plain)
        }
        .padding(12)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).stroke(.orange.opacity(0.45)))
        .shadow(color: .black.opacity(0.12), radius: 10, y: 4)
        .padding(.horizontal, 14)
        .padding(.top, 10)
    }
}

/// A titled card section for detail panes, with a small gradient accent bar.
struct DetailSection<Content: View>: View {
    let title: String
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 7) {
                Capsule().fill(Theme.brand).frame(width: 3, height: 12)
                Text(title.uppercased())
                    .font(.caption.weight(.bold))
                    .foregroundStyle(.secondary)
                    .kerning(0.5)
            }
            content
        }
        .cardSurface()
    }
}
