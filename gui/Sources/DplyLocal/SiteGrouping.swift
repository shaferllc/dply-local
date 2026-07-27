import Foundation

/// How the local-site list is bucketed.
///
/// Four axes, because they answer different questions: *framework* is what the
/// code is built on, *stack* is what's layered on top of it (Filament, Inertia,
/// Livewire — the thing that actually shapes day-to-day work in 56 otherwise
/// identical Laravel apps), *type* is what the project fundamentally is, and
/// *tag* is the one thing detection can never know: what the site is for.
/// A fleet of a hundred sites needs all four; none subsumes the others.
enum SiteGrouping: String, CaseIterable {
    case none
    case framework
    case stack
    case kind
    case tag

    var label: String {
        switch self {
        case .none: return "No grouping"
        case .framework: return "Framework"
        case .stack: return "Stack"
        case .kind: return "Type"
        case .tag: return "Tag"
        }
    }

    /// The bucket used for sites with no value on this axis. Kept out of the
    /// sort order's normal ranking so it can be pinned last.
    var catchAll: String {
        switch self {
        case .tag: return "Untagged"
        default: return "Other"
        }
    }

    /// Which buckets a site belongs to. Usually one — but a site carries several
    /// tags, and it should appear under each of them.
    func keys(for row: Row) -> [String] {
        switch self {
        case .none:
            return []
        case .framework:
            // Group by the framework's *name*, not its version: "Laravel (^12.0)"
            // and "Laravel (^13.0)" are one group to a human, and splitting them
            // would produce a section per site.
            let name = frameworkName(row.cell(["framework"]))
            return [name.isEmpty ? catchAll : name]
        case .stack:
            // Plain Laravel apps land in the catch-all rather than vanishing —
            // "which of these have no admin panel" is a real question.
            let name = frameworkName(row.cell(["stack"]))
            return [name.isEmpty ? catchAll : name]
        case .kind:
            let kind = row.cell(["kind"])
            return [kind.isEmpty || kind == "unknown" ? catchAll : kind.capitalized]
        case .tag:
            let tags = Store.tags(of: row)
            return tags.isEmpty ? [catchAll] : tags
        }
    }

    /// `"Laravel (^13.0)"` → `"Laravel"`. Proxies have no framework at all.
    private func frameworkName(_ label: String) -> String {
        guard let paren = label.firstIndex(of: "(") else { return label }
        return String(label[..<paren]).trimmingCharacters(in: .whitespaces)
    }
}
