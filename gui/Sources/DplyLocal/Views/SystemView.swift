import SwiftUI

/// The System surface: every dpl subsystem at a glance — the daemon, the
/// HTTP/HTTPS proxy, the DNS responder, the mail sink, the dumps receiver, TLS,
/// and PHP — each with its live status and detail.
///
/// The data is the same the Doctor page probes (`dpl doctor`); here it's framed
/// as "what's running" rather than "what's wrong", so a green board means every
/// piece of the local stack is up.
struct SystemView: View {
    @EnvironmentObject var store: Store
    @State private var refreshing = false

    /// The runtime subsystems, in the order they matter. Info-only rows (setup
    /// hints, resolution-mode notes) are dropped — this is about services, not
    /// configuration advice.
    private let groups: [(title: String, category: String)] = [
        ("Daemon", "Daemon"),
        ("Networking", "Networking"),
        ("TLS", "TLS"),
        ("PHP", "PHP"),
    ]

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                header
                if let report = store.doctorHealth {
                    ForEach(groups, id: \.category) { group in
                        let rows = report.checks(in: group.category)
                        if !rows.isEmpty {
                            serviceGroup(group.title, rows: rows)
                        }
                    }
                } else {
                    ProgressView("Checking subsystems…")
                        .frame(maxWidth: .infinity, alignment: .center)
                        .padding(.top, 40)
                }
            }
            .padding(20)
        }
        .task { if store.doctorHealth == nil { await store.refreshDoctor() } }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            Task { await store.refreshDoctor() }
        }
    }

    private var header: some View {
        HStack(alignment: .firstTextBaseline) {
            VStack(alignment: .leading, spacing: 2) {
                Text("System").font(.title2.weight(.bold))
                if let summary = store.doctorHealth?.summary {
                    Text(summary.fail > 0
                        ? "\(summary.fail) subsystem\(summary.fail == 1 ? "" : "s") down"
                        : (summary.warn > 0 ? "\(summary.warn) need attention" : "All subsystems running"))
                        .font(.caption)
                        .foregroundStyle(summary.fail > 0 ? .red : (summary.warn > 0 ? .orange : Theme.live))
                }
            }
            Spacer()
            Button {
                Task { refreshing = true; await store.refreshDoctor(); refreshing = false }
            } label: {
                if refreshing { ProgressView().controlSize(.small) }
                else { Label("Refresh", systemImage: "arrow.clockwise") }
            }
            .disabled(refreshing)
        }
    }

    private func serviceGroup(_ title: String, rows: [DoctorCheck]) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(title.uppercased())
                .font(.caption.weight(.semibold))
                .foregroundStyle(.secondary)
                .padding(.bottom, 8)
            VStack(spacing: 0) {
                ForEach(Array(rows.enumerated()), id: \.element.id) { index, check in
                    serviceRow(check)
                    if index < rows.count - 1 {
                        Divider().background(Theme.hairline)
                    }
                }
            }
            .background(RoundedRectangle(cornerRadius: Theme.cardRadius).fill(Theme.card))
        }
    }

    private func serviceRow(_ check: DoctorCheck) -> some View {
        HStack(spacing: 12) {
            Circle()
                .fill(check.status.color)
                .frame(width: 9, height: 9)
            VStack(alignment: .leading, spacing: 1) {
                Text(check.title).font(.callout)
                Text(check.detail).font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
            // A one-click fix for anything that isn't running, when the check
            // offers one (e.g. "sudo dpl setup" for the port redirect).
            if check.status != .pass, check.status != .info, let fix = check.fix, !fix.command.isEmpty {
                DoctorFixButton(fix: fix)
            }
        }
        .padding(.vertical, 10)
        .padding(.horizontal, 14)
    }
}
