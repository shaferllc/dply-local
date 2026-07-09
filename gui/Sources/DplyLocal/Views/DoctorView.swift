import SwiftUI

/// The Doctor: every probe `dpl doctor` runs, on one page. A verdict up top,
/// everything that needs attention (each with the command that fixes it), then
/// the full breakdown by category.
struct DoctorPage: View {
    @EnvironmentObject var store: Store
    @State private var problemsOnly = false
    @State private var query = ""
    @State private var copied = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                header
                if let report = store.doctorHealth {
                    verdictCard(report)
                    if !report.problems.isEmpty {
                        DetailSection(title: "Needs attention") {
                            VStack(alignment: .leading, spacing: 12) {
                                ForEach(report.problems) { problem in
                                    problemRow(problem)
                                    if problem.id != report.problems.last?.id { Divider() }
                                }
                            }
                        }
                    }
                    categories(report)
                } else if store.doctorRunning {
                    HStack(spacing: 8) {
                        ProgressView().controlSize(.small)
                        Text("Running checks…").foregroundStyle(.secondary)
                    }
                    .padding(.top, 40)
                    .frame(maxWidth: .infinity)
                } else {
                    ContentUnavailableView(
                        "Couldn't run the checks",
                        systemImage: "stethoscope",
                        description: Text("The `dpl` binary couldn't be reached. Check its path in Settings.")
                    )
                }
            }
            .padding(18)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .task { if store.doctorHealth == nil { await store.refreshDoctor() } }
    }

    // MARK: Header

    private var header: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 12) {
                GradientTile(systemImage: "stethoscope", size: 40)
                VStack(alignment: .leading, spacing: 2) {
                    Text("Doctor").font(.title2.weight(.bold))
                    Text(verdict).font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                if let report = store.doctorHealth {
                    countPill(report.summary.fail, .fail)
                    countPill(report.summary.warn, .warn)
                    countPill(report.summary.pass, .pass)
                }
                Button { Task { await store.refreshDoctor() } } label: {
                    Image(systemName: "arrow.clockwise")
                }
                .help("Re-run every check")
                .disabled(store.doctorRunning)
            }
            if store.doctorHealth != nil {
                HStack(spacing: 10) {
                    TextField("Filter checks", text: $query)
                        .textFieldStyle(.roundedBorder)
                        .frame(maxWidth: 260)
                    Toggle("Problems only", isOn: $problemsOnly)
                        .toggleStyle(.switch).controlSize(.small)
                    Spacer()
                    copyButton
                    Button("Setup Wizard…") { store.showOnboarding = true }
                    if store.doctorRunning { ProgressView().controlSize(.small) }
                }
            }
        }
    }

    private var verdict: String {
        guard let report = store.doctorHealth else {
            return store.doctorRunning ? "Running checks…" : "Not run yet"
        }
        if report.summary.fail > 0 {
            return "\(report.summary.fail) problem\(report.summary.fail == 1 ? "" : "s") found"
        }
        if report.summary.warn > 0 {
            return "\(report.summary.warn) warning\(report.summary.warn == 1 ? "" : "s")"
        }
        return "Everything checks out"
    }

    @ViewBuilder
    private var copyButton: some View {
        if let report = store.doctorHealth {
            Button {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(report.plainText(), forType: .string)
                copied = true
                Task { try? await Task.sleep(for: .seconds(2)); copied = false }
            } label: {
                Label(copied ? "Copied" : "Copy report", systemImage: copied ? "checkmark" : "doc.on.doc")
            }
            .help("Copy the full report — handy for a bug report")
        }
    }

    private func countPill(_ count: Int, _ status: DoctorStatus) -> some View {
        HStack(spacing: 4) {
            Image(systemName: status.systemImage).font(.system(size: 9))
            Text("\(count)").font(.caption.weight(.semibold).monospacedDigit())
        }
        .foregroundStyle(count == 0 ? Color.secondary : status.color)
        .padding(.horizontal, 7).padding(.vertical, 3)
        .background((count == 0 ? Color.secondary : status.color).opacity(0.12), in: Capsule())
        .help("\(count) \(status.label.lowercased())")
    }

    // MARK: Verdict

    private func verdictCard(_ report: DoctorReport) -> some View {
        let status: DoctorStatus = report.summary.fail > 0 ? .fail : (report.summary.warn > 0 ? .warn : .pass)
        let title = report.summary.fail > 0
            ? "\(report.summary.fail) problem\(report.summary.fail == 1 ? "" : "s") to fix"
            : (report.summary.warn > 0
                ? "Working, with \(report.summary.warn) warning\(report.summary.warn == 1 ? "" : "s")"
                : "Everything checks out")
        let subtitle = report.summary.fail > 0
            ? "Your sites won't work correctly until these are resolved."
            : (report.summary.warn > 0
                ? "Nothing is broken — these are optional or transient."
                : "\(report.summary.pass) checks passed across \(report.categories.count) categories.")

        return HStack(spacing: 14) {
            Image(systemName: status.systemImage)
                .font(.system(size: 30))
                .foregroundStyle(status.color)
            VStack(alignment: .leading, spacing: 3) {
                Text(title).font(.title3.weight(.semibold))
                Text(subtitle).font(.callout).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 0)
        }
        .cardSurface()
    }

    // MARK: Categories

    /// Category cards, two-up on a wide window and one-up when narrow.
    private func categories(_ report: DoctorReport) -> some View {
        let visible = report.categories.filter { !checks(report, $0).isEmpty }
        return Group {
            if visible.isEmpty {
                ContentUnavailableView(
                    problemsOnly ? "Nothing needs attention" : "No matching checks",
                    systemImage: problemsOnly ? "checkmark.seal" : "magnifyingglass",
                    description: Text(problemsOnly
                        ? "Every check passed. Turn off “Problems only” to see them all."
                        : "No check matches “\(query)”.")
                )
            } else {
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 380), spacing: 16, alignment: .top)], spacing: 16) {
                    ForEach(visible, id: \.self) { category in
                        categoryCard(category, checks: checks(report, category), severity: report.severity(of: category))
                    }
                }
            }
        }
    }

    private func categoryCard(_ category: String, checks: [DoctorCheck], severity: DoctorStatus) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 7) {
                Image(systemName: doctorCategoryIcon(category))
                    .foregroundStyle(severity.color)
                Text(category.uppercased())
                    .font(.caption.weight(.bold)).foregroundStyle(.secondary).kerning(0.5)
                Spacer()
                Text("\(checks.count)")
                    .font(.caption2.monospacedDigit()).foregroundStyle(.tertiary)
            }
            VStack(alignment: .leading, spacing: 10) {
                ForEach(checks) { check in
                    CheckRow(check: check)
                    if check.id != checks.last?.id { Divider().opacity(0.5) }
                }
            }
        }
        .cardSurface()
    }

    private func problemRow(_ check: DoctorCheck) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: check.status.systemImage)
                .foregroundStyle(check.status.color).padding(.top, 2)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(check.title).font(.callout.weight(.medium))
                    Text(check.category)
                        .font(.caption2).foregroundStyle(.secondary)
                        .padding(.horizontal, 5).padding(.vertical, 1)
                        .background(Color.secondary.opacity(0.12), in: Capsule())
                }
                Text(check.detail).font(.caption).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                if let hint = check.hint {
                    Text(hint).font(.caption2).foregroundStyle(.tertiary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            Spacer(minLength: 8)
            if let fix = check.fix, !fix.command.isEmpty {
                DoctorFixButton(fix: fix, prominent: true)
            }
        }
    }

    /// A category's checks after the "problems only" toggle and the filter box.
    private func checks(_ report: DoctorReport, _ category: String) -> [DoctorCheck] {
        report.checks(in: category).filter { check in
            if problemsOnly && check.status != .fail && check.status != .warn { return false }
            guard !query.isEmpty else { return true }
            return check.title.localizedCaseInsensitiveContains(query)
                || check.detail.localizedCaseInsensitiveContains(query)
                || check.id.localizedCaseInsensitiveContains(query)
        }
    }
}

/// One check: status, what was found, why it matters, and its fix. Shared by
/// the Doctor page and the deploy-parity sheet — both render the same report.
struct CheckRow: View {
    let check: DoctorCheck
    /// Re-run after an in-process fix. Doctor re-runs the whole report itself.
    var onFixed: (() async -> Void)?

    var body: some View {
        HStack(alignment: .top, spacing: 9) {
            Image(systemName: check.status.systemImage)
                .font(.caption).foregroundStyle(check.status.color)
                .frame(width: 14).padding(.top, 2)
            VStack(alignment: .leading, spacing: 2) {
                Text(check.title).font(.callout.weight(.medium))
                Text(check.detail)
                    .font(.caption).foregroundStyle(.secondary)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                // The "why it matters" line only earns its space when something
                // is actually wrong.
                if let hint = check.hint, check.status != .pass {
                    Text(hint)
                        .font(.caption2).foregroundStyle(.tertiary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            Spacer(minLength: 6)
            if let fix = check.fix, !fix.command.isEmpty, check.status != .pass {
                DoctorFixButton(fix: fix, onFixed: onFixed)
            }
        }
    }
}

/// Runs a check's fix, then re-runs the checks so the row updates in place.
struct DoctorFixButton: View {
    @EnvironmentObject var store: Store
    let fix: DoctorFix
    var prominent = false
    /// What to re-run after an in-process fix; defaults to the Doctor report.
    var onFixed: (() async -> Void)?
    @State private var busy = false
    @State private var copied = false

    var body: some View {
        Button {
            Task {
                busy = true
                await store.runFix(fix)
                if fix.needsEditing {
                    copied = true
                    try? await Task.sleep(for: .seconds(2))
                    copied = false
                } else if !fix.sudo {
                    // A sudo fix runs in Terminal and finishes on its own
                    // schedule, so only re-run for the in-process ones.
                    if let onFixed { await onFixed() } else { await store.refreshDoctor() }
                }
                busy = false
            }
        } label: {
            if busy && !copied {
                ProgressView().controlSize(.small)
            } else {
                Label(copied ? "Copied" : fix.label, systemImage: icon).font(.caption)
            }
        }
        .buttonStyle(.bordered)
        .tint(prominent ? Theme.violet : nil)
        .disabled(busy && !copied)
        .help(helpText)
    }

    private var icon: String {
        if copied { return "checkmark" }
        if fix.needsEditing { return "doc.on.doc" }
        return fix.sudo ? "lock.shield" : "wrench.and.screwdriver"
    }

    private var helpText: String {
        if fix.needsEditing { return "Copies (it needs a value you must fill in): \(fix.command)" }
        return fix.sudo ? "Opens Terminal: \(fix.command)" : "Runs: \(fix.command)"
    }
}
