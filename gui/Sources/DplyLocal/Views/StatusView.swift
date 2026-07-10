import SwiftUI

/// System cockpit: `dpl doctor` health plus the one-time/lifecycle actions that
/// were previously CLI-only — trusted `.test` setup, autostart, restart.
struct StatusView: View {
    @EnvironmentObject var store: Store
    @State private var report: DoctorReport?
    @State private var loading = true
    @State private var autostart = false
    @State private var busy = false
    @State private var resolutionMode = "resolver"

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                HStack(spacing: 12) {
                    GradientTile(systemImage: "waveform.path.ecg", size: 40)
                    Text("Status").font(.title2.weight(.bold))
                    Spacer()
                    Button("Setup Wizard…") { store.showOnboarding = true }
                    Button { Task { await reload() } } label: { Image(systemName: "arrow.clockwise") }
                        .disabled(loading)
                }

                DetailSection(title: "Health") {
                    if loading {
                        ProgressView().controlSize(.small)
                    } else if let report {
                        healthSummary(report)
                    } else {
                        Text("Couldn't run the checks — is the `dpl` binary on your PATH?")
                            .font(.callout).foregroundStyle(.secondary)
                    }
                }

                DetailSection(title: "System load") {
                    VStack(alignment: .leading, spacing: 6) {
                        HStack(spacing: 8) {
                            let sev = store.loadSeverity
                            let color: Color = sev == 2 ? .red : (sev == 1 ? .orange : .green)
                            Image(systemName: "gauge.with.dots.needle.67percent").foregroundStyle(color)
                            Text(String(format: "%.1f", store.loadAvg)).font(.title3.weight(.semibold).monospacedDigit())
                            Text("across \(store.cpuCount) cores").font(.caption).foregroundStyle(.secondary)
                            Spacer()
                            Text(sev == 2 ? "Saturated" : (sev == 1 ? "Busy" : "OK"))
                                .font(.caption2.weight(.medium))
                                .padding(.horizontal, 8).padding(.vertical, 3)
                                .background(color.opacity(0.15), in: Capsule()).foregroundStyle(color)
                        }
                        if store.loadSeverity == 2 {
                            Text("The CPU is oversubscribed, so php-fpm can't spawn workers fast enough and sites may hang or time out. This isn't dpl — reduce background load (e.g. pause idle queue workers). “Repair backends” below clears stale state but can't beat a saturated CPU.")
                                .font(.caption).foregroundStyle(.secondary)
                        }
                    }
                }

                if portConflict {
                    DetailSection(title: "⚠︎ Another server owns ports 80/443") {
                        VStack(alignment: .leading, spacing: 8) {
                            Text("Valet's nginx / Apache is holding :80 and :443, so the browser hits it (e.g. the “It works!” page) instead of dpl. Take over the ports to serve your sites with dpl.")
                                .font(.caption).foregroundStyle(.secondary)
                            Button {
                                store.runTakeoverInTerminal()
                            } label: {
                                Label("Take over ports (stop Valet & Apache)…", systemImage: "bolt.shield")
                            }
                            .buttonStyle(.borderedProminent).tint(Theme.violet)
                            .help("Opens Terminal to run `dpl takeover` (needs your password)")
                            Text("This stops Valet and Apache; dpl then serves everything. Re-run the health check afterward.")
                                .font(.caption2).foregroundStyle(.secondary)
                        }
                    }
                }

                DetailSection(title: "Clean .test on :80 + trusted HTTPS") {
                    VStack(alignment: .leading, spacing: 8) {
                        Text("One-time privileged setup: routes .test, hands :80/:443 to the daemon, and trusts the local CA so sites are at http://name.test (no port) with a padlock.")
                            .font(.caption).foregroundStyle(.secondary)
                        Button {
                            Task { await store.runSetup() }
                        } label: {
                            Label("Run setup…", systemImage: "lock.shield")
                        }
                        .help("macOS will ask for your password")
                    }
                }

                DetailSection(title: "DNS & iCloud Private Relay") {
                    VStack(alignment: .leading, spacing: 8) {
                        Text("Current mode: \(resolutionMode == "hosts" ? "hosts-file (Private Relay stays on)" : "wildcard DNS resolver")")
                            .font(.callout.weight(.medium))
                        Text("The DNS resolver breaks iCloud Private Relay (any .test dev tool does). Hosts-file mode maps each site in /etc/hosts instead — Private Relay keeps working. dpl keeps /etc/hosts in sync automatically.")
                            .font(.caption).foregroundStyle(.secondary)
                        HStack {
                            if resolutionMode == "hosts" {
                                Button {
                                    store.runResolutionInTerminal("resolver")
                                } label: { Label("Switch to wildcard DNS…", systemImage: "network") }
                            } else {
                                Button {
                                    store.runResolutionInTerminal("hosts")
                                } label: { Label("Use hosts-file (keep Private Relay)…", systemImage: "checkmark.shield") }
                                .buttonStyle(.borderedProminent).tint(Theme.violet)
                            }
                        }
                        Text("Switching installs/removes a one-time sudoers rule (needs your password).")
                            .font(.caption2).foregroundStyle(.secondary)
                    }
                }

                DetailSection(title: "Switch back to Valet") {
                    VStack(alignment: .leading, spacing: 8) {
                        Text("If you took over the ports, this restores Valet: re-enables its nginx/dnsmasq and removes dpl's :80/:443 redirect. dpl keeps working on :8080/:8443.")
                            .font(.caption).foregroundStyle(.secondary)
                        Button {
                            store.runUntakeoverInTerminal()
                        } label: {
                            Label("Restore Valet in Terminal…", systemImage: "arrow.uturn.backward")
                        }
                        .help("Opens Terminal to run `dpl untakeover` (needs your password)")
                    }
                }

                DetailSection(title: "Background service") {
                    Toggle(isOn: $autostart) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text("Start on login (autostart)")
                            Text("Installs a LaunchAgent so the daemon is always running.")
                                .font(.caption).foregroundStyle(.secondary)
                        }
                    }
                    .disabled(busy)
                    .onChange(of: autostart) { _, on in
                        Task { busy = true; await store.daemonAction(on ? "install" : "uninstall"); busy = false }
                    }
                    HStack(spacing: 8) {
                        Button("Start") { Task { busy = true; await store.startDaemon(); busy = false; await reload() } }
                        Button("Stop") { Task { busy = true; await store.stopDaemon(); busy = false; await reload() } }
                        Button("Reload") { Task { busy = true; await store.reloadDaemon(); busy = false; await reload() } }
                            .help("Re-read config + reconcile without restarting the daemon")
                        Button("Restart") { Task { busy = true; await store.restartDaemon(); busy = false; await reload() } }
                        if busy { ProgressView().controlSize(.small) }
                    }
                    .disabled(busy)
                    Button {
                        Task { busy = true; await store.repairBackends(); busy = false; await reload() }
                    } label: {
                        Label("Repair backends", systemImage: "wrench.and.screwdriver")
                    }
                    .disabled(busy)
                    .help("Kill stale php-fpm / Octane servers and rebuild — use when sites hang or the machine got churned")
                }
            }
            .padding(18)
        }
        .task { await reload() }
    }

    /// The doctor found another web server on :80. Keyed off the check's stable
    /// id, not its prose — `doctor.rs` owns the wording.
    private var portConflict: Bool {
        report?.checks.first { $0.id == "net.port_80" }?.status == .fail
    }

    /// A verdict plus what's wrong; the Doctor page has the full breakdown.
    @ViewBuilder
    private func healthSummary(_ report: DoctorReport) -> some View {
        let status: DoctorStatus = report.summary.fail > 0 ? .fail : (report.summary.warn > 0 ? .warn : .pass)
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Image(systemName: status.systemImage).foregroundStyle(status.color)
                Text(report.summary.fail > 0
                     ? "\(report.summary.fail) problem(s), \(report.summary.warn) warning(s)"
                     : (report.summary.warn > 0 ? "\(report.summary.warn) warning(s)" : "All \(report.summary.pass) checks passed"))
                    .font(.callout.weight(.medium))
                Spacer()
                Button("Open Doctor") { store.section = .doctor }
            }
            ForEach(report.problems.prefix(4)) { check in
                HStack(alignment: .top, spacing: 8) {
                    Image(systemName: check.status.systemImage)
                        .font(.caption).foregroundStyle(check.status.color).frame(width: 14)
                    Text("\(check.title): \(check.detail)")
                        .font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
                    Spacer(minLength: 0)
                }
            }
        }
    }

    private func reload() async {
        loading = true
        report = await store.doctorReport()
        autostart = await store.daemonStatus()
        resolutionMode = await store.loadResolutionMode()
        loading = false
    }
}
