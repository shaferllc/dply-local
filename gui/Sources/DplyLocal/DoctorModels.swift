import SwiftUI

/// A check report from the CLI — `dpl doctor --json` or `dpl parity … --json`,
/// which share a wire format. The CLI owns every probe; this is a faithful
/// decode of its output, so a check added in Rust shows up here with no GUI
/// change beyond an icon.
struct DoctorReport: Decodable {
    /// What produced the report; older builds omit it.
    let title: String?
    /// One line of context, e.g. the two environments being compared.
    let subtitle: String?
    let ok: Bool
    let summary: DoctorSummary
    /// Category names in the order the CLI wants them displayed.
    let categories: [String]
    let checks: [DoctorCheck]

    /// The checks in `category`, in report order.
    func checks(in category: String) -> [DoctorCheck] {
        checks.filter { $0.category == category }
    }

    /// The worst status present in a category — drives its badge.
    func severity(of category: String) -> DoctorStatus {
        let statuses = checks(in: category).map(\.status)
        if statuses.contains(.fail) { return .fail }
        if statuses.contains(.warn) { return .warn }
        return statuses.contains(.pass) ? .pass : .info
    }

    /// Everything that needs attention, worst first.
    var problems: [DoctorCheck] {
        checks.filter { $0.status == .fail || $0.status == .warn }
            .sorted { $0.status == .fail && $1.status != .fail }
    }

    /// The report as text, for the "Copy report" button.
    func plainText() -> String {
        var out = [title ?? "dpl doctor", subtitle, ""].compactMap { $0 }
        for category in categories {
            let rows = checks(in: category)
            guard !rows.isEmpty else { continue }
            out.append(category)
            for check in rows {
                out.append("  \(check.status.mark) \(check.title): \(check.detail)")
                if let hint = check.hint { out.append("      \(hint)") }
                if let fix = check.fix, !fix.command.isEmpty { out.append("      → \(fix.command)") }
            }
            out.append("")
        }
        out.append("\(summary.pass) passed, \(summary.warn) warning(s), \(summary.fail) problem(s)")
        return out.joined(separator: "\n")
    }
}

struct DoctorSummary: Decodable {
    let pass: Int
    let warn: Int
    let fail: Int
    let info: Int
}

/// A command that resolves a check. `sudo` commands prompt for a password, so
/// they're handed to Terminal rather than run headless. `placeholder` commands
/// contain a `<value>` the user must fill in and are only ever copied.
struct DoctorFix: Decodable, Hashable {
    let label: String
    let command: String
    let sudo: Bool
    let placeholder: Bool?

    var needsEditing: Bool { placeholder ?? command.contains("<") }
}

struct DoctorCheck: Decodable, Identifiable, Hashable {
    let id: String
    let category: String
    let title: String
    let status: DoctorStatus
    let detail: String
    let hint: String?
    let fix: DoctorFix?
}

enum DoctorStatus: String, Decodable {
    case pass, warn, fail, info

    var mark: String {
        switch self {
        case .pass: return "✓"
        case .fail: return "✗"
        case .warn: return "⚠"
        case .info: return "·"
        }
    }

    var systemImage: String {
        switch self {
        case .pass: return "checkmark.circle.fill"
        case .fail: return "xmark.octagon.fill"
        case .warn: return "exclamationmark.triangle.fill"
        case .info: return "info.circle"
        }
    }

    var color: Color {
        switch self {
        case .pass: return Theme.live
        case .fail: return .red
        case .warn: return .orange
        case .info: return .secondary
        }
    }

    var label: String {
        switch self {
        case .pass: return "Healthy"
        case .fail: return "Problem"
        case .warn: return "Warning"
        case .info: return "Info"
        }
    }
}

/// SF Symbol per doctor category (matched on the CLI's category names).
func doctorCategoryIcon(_ category: String) -> String {
    switch category {
    case "System": return "cpu"
    case "Daemon": return "gearshape.2"
    case "Networking": return "network"
    case "TLS": return "lock.shield"
    case "PHP": return "chevron.left.forwardslash.chevron.right"
    case "Sites": return "house"
    case "Services": return "cylinder.split.1x2"
    case "Tooling": return "wrench.and.screwdriver"
    // Parity categories.
    case "Project": return "folder"
    case "Environment": return "key"
    default: return "circle"
    }
}
