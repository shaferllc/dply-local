import SwiftUI

/// Deploy parity: what differs between this local site and the dply site it
/// ships to. Renders the same check report the Doctor page does, because
/// `dpl parity --json` and `dpl doctor --json` share a wire format.
///
/// Environment variables are compared by **key name only** — no value is read
/// from your `.env` or from the server, so a parity report is safe to share.
struct ParitySheet: View {
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss

    /// The local site being compared.
    let site: String

    @State private var remote: String = ""
    @State private var report: DoctorReport?
    @State private var running = false
    @State private var copied = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Divider()
            content
            Divider()
            footer
        }
        .frame(width: 720, height: 620)
        .task {
            await store.loadRemoteSites()
            if remote.isEmpty { remote = rememberedRemote ?? bestRemoteMatch() ?? "" }
            // Only compare once we know which remote site to compare against —
            // guessing produces a 404 the moment the names differ.
            if !remote.isEmpty { await compare() }
        }
    }

    /// The remote this local site was last compared against.
    private var rememberedRemote: String? {
        UserDefaults.standard.string(forKey: "parityRemote.\(site)")
            .flatMap { remoteNames.contains($0) ? $0 : nil }
    }

    /// Same name if it exists, else the one that contains it (`lookout` →
    /// `uselookout`), else nothing — deploy names rarely match the local folder.
    private func bestRemoteMatch() -> String? {
        if remoteNames.contains(site) { return site }
        let lower = site.lowercased()
        return remoteNames.first {
            let n = $0.lowercased()
            return n.contains(lower) || lower.contains(n)
        }
    }

    // MARK: Header

    private var header: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 12) {
                GradientTile(systemImage: "arrow.left.arrow.right", size: 36)
                VStack(alignment: .leading, spacing: 2) {
                    Text("Deploy parity").font(.title3.weight(.bold))
                    Text(report?.subtitle ?? "Compare \(site).test with the dply site it deploys to")
                        .font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
            }
            HStack(spacing: 8) {
                Text("Compare with").font(.caption).foregroundStyle(.secondary)
                Picker("", selection: $remote) {
                    Text("Choose a dply site…").tag("")
                    ForEach(remoteNames, id: \.self) { Text($0).tag($0) }
                }
                .labelsHidden()
                .frame(maxWidth: 260)
                Button("Compare") { Task { await compare() } }
                    .buttonStyle(.borderedProminent).tint(Theme.violet)
                    .disabled(running || remote.isEmpty)
                if running { ProgressView().controlSize(.small) }
                Spacer()
                if let report { verdictPill(report) }
            }
        }
        .padding(16)
    }

    private func verdictPill(_ report: DoctorReport) -> some View {
        let status: DoctorStatus = report.summary.fail > 0 ? .fail : (report.summary.warn > 0 ? .warn : .pass)
        let text = report.summary.fail > 0
            ? "\(report.summary.fail) will bite you"
            : (report.summary.warn > 0 ? "\(report.summary.warn) worth knowing" : "In sync")
        return Label(text, systemImage: status.systemImage)
            .font(.caption.weight(.medium))
            .padding(.horizontal, 9).padding(.vertical, 4)
            .background(status.color.opacity(0.15), in: Capsule())
            .foregroundStyle(status.color)
    }

    // MARK: Body

    @ViewBuilder
    private var content: some View {
        if let report {
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    ForEach(report.categories, id: \.self) { category in
                        let checks = report.checks(in: category)
                        if !checks.isEmpty {
                            VStack(alignment: .leading, spacing: 10) {
                                HStack(spacing: 7) {
                                    Image(systemName: doctorCategoryIcon(category))
                                        .foregroundStyle(report.severity(of: category).color)
                                    Text(category.uppercased())
                                        .font(.caption.weight(.bold)).foregroundStyle(.secondary).kerning(0.5)
                                    Spacer()
                                }
                                ForEach(checks) { check in
                                    CheckRow(check: check, onFixed: { await compare() })
                                    if check.id != checks.last?.id { Divider().opacity(0.5) }
                                }
                            }
                            .cardSurface()
                        }
                    }
                    Text("Environment variables are compared by name. No value is ever read from your .env or from the server.")
                        .font(.caption2).foregroundStyle(.tertiary)
                }
                .padding(16)
            }
        } else if running {
            VStack { Spacer(); ProgressView("Comparing…").controlSize(.small); Spacer() }
                .frame(maxWidth: .infinity)
        } else if remote.isEmpty {
            ContentUnavailableView(
                "Which site does this deploy to?",
                systemImage: "arrow.left.arrow.right",
                description: Text("\(site) doesn't match a dply site by name. Pick its counterpart above, then Compare — the choice is remembered.")
            )
        } else {
            ContentUnavailableView(
                "Couldn't compare",
                systemImage: "exclamationmark.triangle",
                description: Text("Check that `\(remote)` exists on dply and that you're logged in (Account).")
            )
        }
    }

    // MARK: Footer

    private var footer: some View {
        HStack {
            if let report {
                Button {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(report.plainText(), forType: .string)
                    copied = true
                    Task { try? await Task.sleep(for: .seconds(2)); copied = false }
                } label: {
                    Label(copied ? "Copied" : "Copy report", systemImage: copied ? "checkmark" : "doc.on.doc")
                }
                .help("Safe to paste — it contains key names, never values")
            }
            Spacer()
            Button("Done") { dismiss() }.keyboardShortcut(.defaultAction)
        }
        .padding(12)
    }

    private var remoteNames: [String] {
        store.sites.map { $0.cell(["name"]) }.filter { !$0.isEmpty }.sorted()
    }

    private func compare() async {
        guard !remote.isEmpty else { return }
        running = true
        report = await store.parity(site: site, remote: remote)
        // Remember the pairing so this site opens on the right remote next time.
        if report != nil {
            UserDefaults.standard.set(remote, forKey: "parityRemote.\(site)")
        }
        running = false
    }
}
