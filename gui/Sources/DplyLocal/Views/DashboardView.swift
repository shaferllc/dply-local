import SwiftUI

/// The home dashboard — a beautiful at-a-glance overview: sites, services,
/// system health, and live resource usage, in a responsive card grid.
struct DashboardView: View {
    @EnvironmentObject var store: Store
    @State private var health: [String] = []
    @State private var top: [(name: String, mb: Int, cpu: Double)] = []
    @State private var workers: [WorkerGroup] = []
    @State private var devServers: [DevServer] = []
    /// Sites whose dev server is mid-action, so the row can disable itself.
    @State private var devBusy: Set<String> = []

    private var sitesRunning: Int { store.localSites.filter { $0.dig("serving") == .bool(true) }.count }
    private var servicesActive: Int { store.dbServices.filter { $0.dig("running") == .bool(true) }.count }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                header
                LazyVGrid(columns: [
                    GridItem(.flexible(), spacing: 16, alignment: .top),
                    GridItem(.flexible(), spacing: 16, alignment: .top),
                ], spacing: 16) {
                    sitesCard
                    servicesCard
                    workersCard
                    healthCard
                    resourcesCard
                }
            }
            .padding(20)
        }
        // Cards grow as their data arrives; without this the grid drifts out
        // from under the title bar on first load.
        .defaultScrollAnchor(.top)
        .task { await refresh() }
    }

    // MARK: Header

    private var header: some View {
        HStack(spacing: 12) {
            GradientTile(systemImage: "bolt.horizontal.fill", size: 44)
            VStack(alignment: .leading, spacing: 3) {
                Text("Dashboard").font(.largeTitle.weight(.bold))
                HStack(spacing: 6) {
                    Circle().fill(Theme.live).frame(width: 7, height: 7)
                    Text("Everything's running · \(sitesRunning)/\(store.localSites.count) sites · \(servicesActive) services active")
                        .font(.callout).foregroundStyle(.secondary)
                }
            }
            Spacer()
            Button { Task { await refresh(force: true) } } label: { Image(systemName: "arrow.clockwise") }
        }
    }

    // MARK: Cards

    private var sitesCard: some View {
        card(title: "Sites", badge: "\(sitesRunning)/\(store.localSites.count) running", badgeColor: .green) {
            VStack(spacing: 0) {
                ForEach(store.localSites.prefix(6)) { s in
                    HStack(spacing: 8) {
                        Circle().fill(s.dig("serving") == .bool(true) ? Color.green : .secondary.opacity(0.4)).frame(width: 7, height: 7)
                        Text(s.cell(["host"])).font(.callout.weight(.medium)).lineLimit(1)
                        Spacer()
                        if !s.cell(["framework"]).isEmpty {
                            Text(s.cell(["framework"])).font(.caption2).foregroundStyle(Theme.violet).lineLimit(1)
                        }
                    }
                    .padding(.vertical, 5)
                    Divider()
                }
                if store.localSites.isEmpty {
                    Text("No sites yet.").font(.caption).foregroundStyle(.secondary).frame(maxWidth: .infinity, alignment: .leading).padding(.vertical, 8)
                }
                Spacer(minLength: 8) // keep the actions on the card's bottom edge
                HStack {
                    Button { store.section = .local } label: { Label("Link site", systemImage: "plus") }
                        .buttonStyle(.borderedProminent).tint(Theme.violet).controlSize(.small)
                    Spacer()
                    Button("Open sites →") { store.section = .local }.buttonStyle(.plain).font(.caption).foregroundStyle(Theme.violet)
                }
                .padding(.top, 8)
            }
        }
    }

    private var servicesCard: some View {
        card(title: "Services", badge: "\(servicesActive) active", badgeColor: servicesActive > 0 ? .green : .secondary) {
            VStack(spacing: 0) {
                ForEach(store.dbServices.prefix(6)) { s in
                    HStack(spacing: 8) {
                        Circle().fill(s.dig("running") == .bool(true) ? Color.green : .secondary.opacity(0.4)).frame(width: 7, height: 7)
                        Text(s.cell(["name"]).capitalized).font(.callout.weight(.medium))
                        Spacer()
                        Text(":\(s.cell(["port"]))").font(.system(.caption2, design: .monospaced)).foregroundStyle(.secondary)
                    }
                    .padding(.vertical, 5)
                    Divider()
                }
                if store.dbServices.isEmpty {
                    Text("No services running.").font(.caption).foregroundStyle(.secondary).frame(maxWidth: .infinity, alignment: .leading).padding(.vertical, 8)
                }
                Spacer(minLength: 8)
                HStack {
                    Button { store.section = .services } label: { Label("Add", systemImage: "plus") }
                        .buttonStyle(.borderedProminent).tint(Theme.violet).controlSize(.small)
                    Spacer()
                    Button("Open services →") { store.section = .services }.buttonStyle(.plain).font(.caption).foregroundStyle(Theme.violet)
                }
                .padding(.top, 8)
            }
        }
    }

    private var workersCard: some View {
        let observed = workers.reduce(0) { $0 + $1.items.count }
        let total = observed + devServers.filter(\.running).count
        return card(title: "Workers", badge: total > 0 ? "\(total) running" : "none",
                    badgeColor: total > 0 ? .green : .secondary) {
            VStack(alignment: .leading, spacing: 8) {
                if workers.isEmpty && devServers.isEmpty {
                    Text("No dev servers, Horizon, queue, Reverb or Vite workers detected.")
                        .font(.caption).foregroundStyle(.secondary).frame(maxWidth: .infinity, alignment: .leading).padding(.vertical, 6)
                }
                // Supervised first: these are the ones you can act on.
                if !devServers.isEmpty {
                    HStack {
                        Text("DEV SERVERS").font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
                        Spacer()
                        Text("\(devServers.filter(\.running).count)/\(devServers.count)")
                            .font(.caption2.monospacedDigit()).foregroundStyle(.green)
                    }
                    ForEach(devServers) { dev in devServerRow(dev) }
                }
                ForEach(workers) { group in
                    // Collapse the many processes per project into one row + count.
                    let byProject = Dictionary(grouping: group.items, by: \.name)
                        .map { (name: $0.key, count: $0.value.count) }
                        .sorted { $0.name < $1.name }
                    HStack {
                        Text(group.kind.uppercased()).font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
                        Spacer()
                        Text("\(group.items.count)").font(.caption2.monospacedDigit()).foregroundStyle(.green)
                    }
                    ForEach(byProject.prefix(6), id: \.name) { proj in
                        HStack(spacing: 8) {
                            Circle().fill(Color.green).frame(width: 6, height: 6)
                            Text(proj.name).font(.callout).foregroundStyle(Theme.violet).lineLimit(1)
                            if proj.count > 1 { Text("×\(proj.count)").font(.caption2.monospacedDigit()).foregroundStyle(.secondary) }
                            Spacer()
                        }
                        .padding(.leading, 4)
                    }
                }
                Spacer(minLength: 2)
                Text(workers.isEmpty && devServers.isEmpty ? "" : hint)
                    .font(.caption2).foregroundStyle(.secondary).padding(.top, 2)
            }
        }
    }

    /// The observed processes can't be controlled from here — say so once,
    /// rather than leaving the reader to wonder why only some rows have buttons.
    private var hint: String {
        if devServers.isEmpty { return "Started outside dpl — restart them where you started them." }
        if workers.isEmpty { return "Supervised by dpl: these restart themselves if they die." }
        return "Dev servers are supervised; the rest were started outside dpl."
    }

    /// One supervised dev server: state, where it's listening, stop/restart.
    private func devServerRow(_ dev: DevServer) -> some View {
        HStack(spacing: 8) {
            Circle().fill(dev.running ? Color.green : Color.orange).frame(width: 6, height: 6)
            Text(dev.site).font(.callout).foregroundStyle(Theme.violet).lineLimit(1)
            Text(dev.script).font(.caption2).foregroundStyle(.secondary)
            if let port = dev.port, dev.running {
                Button("localhost:\(port)") { store.openURL("http://localhost:\(port)") }
                    .buttonStyle(.link).font(.caption2)
            } else if !dev.running, let detail = dev.detail {
                Text(detail).font(.caption2).foregroundStyle(.orange).lineLimit(1)
            }
            Spacer()
            if devBusy.contains(dev.site) {
                ProgressView().controlSize(.small)
            } else {
                Button {
                    Task { await act(dev.site) { await store.restartDevServer(site: dev.site) } }
                } label: { Image(systemName: "arrow.clockwise") }
                    .buttonStyle(.borderless).help("Restart")
                Button {
                    Task { await act(dev.site) { await store.setDevServer(site: dev.site, script: nil) } }
                } label: { Image(systemName: "stop.fill") }
                    .buttonStyle(.borderless).help("Stop and turn off")
            }
        }
        .padding(.leading, 4)
    }

    private func act(_ site: String, _ body: () async -> Void) async {
        devBusy.insert(site)
        await body()
        await refresh()
        devBusy.remove(site)
    }

    private var healthCard: some View {
        let bad = health.contains { $0.hasPrefix("✗") }
        return card(title: "System health", badge: bad ? "Needs attention" : "Healthy", badgeColor: bad ? .orange : .green) {
            VStack(alignment: .leading, spacing: 5) {
                if health.isEmpty { ProgressView().controlSize(.small) }
                ForEach(Array(health.prefix(8).enumerated()), id: \.offset) { _, line in
                    let mark = line.first.map(String.init) ?? ""
                    let color: Color = mark == "✓" ? .green : (mark == "✗" ? .red : .orange)
                    HStack(spacing: 8) {
                        Text(mark).foregroundStyle(color).frame(width: 12)
                        Text(line.dropFirst(mark.isEmpty ? 0 : 1).trimmingCharacters(in: .whitespaces))
                            .font(.caption).lineLimit(1)
                        Spacer(minLength: 0)
                    }
                }
                Spacer(minLength: 4)
                Button("Open system →") { store.section = .status }.buttonStyle(.plain).font(.caption).foregroundStyle(Theme.violet).padding(.top, 4)
            }
        }
    }

    private var resourcesCard: some View {
        card(title: "Resources", badge: String(format: "load %.1f", store.loadAvg),
             badgeColor: store.loadSeverity == 2 ? .red : (store.loadSeverity == 1 ? .orange : .green)) {
            VStack(alignment: .leading, spacing: 8) {
                HStack {
                    VStack(alignment: .leading, spacing: 1) {
                        Text("CPU load").font(.caption2).foregroundStyle(.secondary)
                        Text(String(format: "%.1f", store.loadAvg)).font(.title3.weight(.semibold).monospacedDigit())
                    }
                    Spacer()
                    VStack(alignment: .trailing, spacing: 1) {
                        Text("Memory").font(.caption2).foregroundStyle(.secondary)
                        Text(String(format: "%.0f GB total", store.totalMemoryGB)).font(.title3.weight(.semibold).monospacedDigit())
                    }
                }
                Divider()
                Text("TOP CONSUMERS").font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
                ForEach(Array(top.prefix(6).enumerated()), id: \.offset) { _, p in
                    HStack {
                        Text(p.name).font(.system(.caption, design: .monospaced)).lineLimit(1)
                        Spacer()
                        Text("\(p.mb) MB").font(.system(.caption2, design: .monospaced)).foregroundStyle(.secondary)
                        Text(String(format: "%.1f%%", p.cpu)).font(.system(.caption2, design: .monospaced)).foregroundStyle(.tertiary).frame(width: 44, alignment: .trailing)
                    }
                }
            }
        }
    }

    // MARK: Card chrome

    private func card<C: View>(title: String, badge: String, badgeColor: Color, @ViewBuilder content: () -> C) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text(title).font(.headline)
                Spacer()
                Text(badge).font(.caption2.weight(.medium))
                    .padding(.horizontal, 8).padding(.vertical, 3)
                    .background(badgeColor.opacity(0.15), in: Capsule()).foregroundStyle(badgeColor)
            }
            content()
        }
        .padding(16)
        // Fill the grid row's height so cards sitting side by side match, with
        // their content pinned to the top rather than floating in the middle.
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 14))
        .overlay(RoundedRectangle(cornerRadius: 14).stroke(.separator.opacity(0.5), lineWidth: 1))
    }

    /// Load everything the dashboard shows. Sites + services go through the
    /// shared section refresh (guarded, so this doesn't re-fetch what the
    /// navigation load just fetched); the health/process/worker cards are the
    /// dashboard's own. The toolbar button forces a full re-fetch.
    private func refresh(force: Bool = false) async {
        await store.refreshCurrentSection(force: force)
        health = await store.doctor()
        top = await store.topProcesses()
        devServers = await store.devServers()
        // Hand the supervised process groups to the ps scan so a dev server dpld
        // owns isn't also listed as an untouchable observed process.
        let supervised = Set(devServers.compactMap(\.pgid))
        workers = await store.detectWorkers(excluding: supervised)
    }
}
