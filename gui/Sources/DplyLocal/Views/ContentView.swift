import SwiftUI

/// Three-column layout: a branded, grouped sidebar · the item list · the detail
/// pane. The middle and detail columns swap content based on the selected
/// section.
struct ContentView: View {
    @EnvironmentObject var store: Store
    @State private var selection: String?
    @State private var showAddProxy = false
    @State private var showValetImport = false
    @State private var showCleanup = false
    @State private var showLinkSheet = false
    @State private var pendingLinkPath = ""
    @State private var siteSearch = ""
    /// `siteSearch` debounced — what `filteredLocalSites` actually filters on, so
    /// a burst of keystrokes doesn't re-filter ~120 sites on each one.
    @State private var debouncedSiteSearch = ""
    @AppStorage("didOnboard") private var didOnboard = false

    var body: some View {
        layout
            .tint(Theme.violet)
        .preferredColorScheme(.dark)
        .toolbar { toolbarContent }
        .sheet(isPresented: $showAddProxy) { AddProxySheet().environmentObject(store) }
        .sheet(isPresented: $showValetImport) { ValetImportSheet().environmentObject(store) }
        .sheet(isPresented: $showCleanup) { SiteCleanupSheet().environmentObject(store) }
        .sheet(isPresented: $showLinkSheet) { LinkFolderSheet(path: pendingLinkPath).environmentObject(store) }
        .sheet(isPresented: $store.showOnboarding) {
            OnboardingView(isPresented: $store.showOnboarding).environmentObject(store)
        }
        .onAppear {
            if !didOnboard { store.showOnboarding = true }
            store.startLoadMonitor()
        }
        .onChange(of: store.section) { selection = nil; Task { await store.refreshCurrentSection() } }
        // A "related site" chip asked to select a site. Switch to Local Sites
        // first (its onChange nils the selection), then select on the next
        // runloop turn so the clear can't stomp the jump.
        .onChange(of: store.siteJump) { _, requested in
            guard let requested else { return }
            store.section = .local
            DispatchQueue.main.async {
                selection = requested
                store.siteJump = nil
            }
        }
        .overlay(alignment: .top) {
            VStack(spacing: 6) {
                if let paused = store.pausedDump {
                    PauseBanner(dump: paused) { action in Task { await store.resumeDump(action) } }
                        .transition(.move(edge: .top).combined(with: .opacity))
                }
                if let error = store.lastError {
                    ErrorBanner(message: error) { store.lastError = nil }
                        .transition(.move(edge: .top).combined(with: .opacity))
                }
            }
        }
        .animation(.easeInOut(duration: 0.2), value: store.lastError)
        .animation(.easeInOut(duration: 0.2), value: store.pausedDump)
    }

    /// Three columns for the list-shaped surfaces (sidebar · items · detail);
    /// two for the single-page ones, so Status/Doctor/Settings fill the window.
    @ViewBuilder
    private var layout: some View {
        if store.section.isFullWidth {
            NavigationSplitView {
                sidebar
                    .navigationSplitViewColumnWidth(min: 180, ideal: 210, max: 340)
            } detail: {
                fullWidthPage
            }
        } else {
            NavigationSplitView {
                sidebar
                    .navigationSplitViewColumnWidth(min: 180, ideal: 210, max: 340)
            } content: {
                listColumn
                    .navigationSplitViewColumnWidth(min: 260, ideal: 320, max: 460)
            } detail: {
                detailColumn
            }
        }
    }

    /// The single-page surfaces, rendered across the whole detail area.
    @ViewBuilder
    private var fullWidthPage: some View {
        switch store.section {
        case .dashboard: DashboardView()
        case .php: PhpPage()
        case .extensions: ExtensionsPage()
        case .status: StatusView()
        case .system: SystemView()
        case .doctor: DoctorPage()
        case .settings: SettingsPage()
        default: EmptyView()
        }
    }

    // MARK: Sidebar

    private var sidebar: some View {
        List(selection: $store.section) {
            Section("Local") {
                sidebarRow(.dashboard)
                sidebarRow(.local)
                sidebarRow(.services)
                sidebarRow(.mail)
                sidebarRow(.dumps)
                sidebarRow(.php)
                sidebarRow(.extensions)
            }
            // Opt-in: the dply platform only appears once it's enabled in Settings.
            if store.dplyEnabled {
                Section("dply") {
                    sidebarRow(.edgeSites)
                    sidebarRow(.sites)
                    sidebarRow(.servers)
                    sidebarRow(.account)
                }
            }
            if store.keelEnabled {
                Section("keel cloud") {
                    sidebarRow(.keelSites)
                    sidebarRow(.keelAccount)
                }
            }
            Section("System") {
                sidebarRow(.status)
                sidebarRow(.system)
                sidebarRow(.doctor)
                sidebarRow(.settings)
            }
        }
        .safeAreaInset(edge: .top) { brandHeader }
        .safeAreaInset(edge: .bottom) { hostFooter }
    }

    private func sidebarRow(_ section: Surface) -> some View {
        Label {
            HStack(spacing: 6) {
                Text(section.rawValue)
                if section == .doctor, let badge = doctorBadge {
                    Spacer(minLength: 0)
                    Text("\(badge.count)")
                        .font(.caption2.weight(.semibold).monospacedDigit())
                        .padding(.horizontal, 5).padding(.vertical, 1)
                        .background(badge.color.opacity(0.18), in: Capsule())
                        .foregroundStyle(badge.color)
                }
            }
        } icon: {
            Image(systemName: section.systemImage)
                .foregroundStyle(store.section == section ? AnyShapeStyle(Theme.brand) : AnyShapeStyle(.secondary))
        }
        .tag(section)
    }

    /// How many checks need attention, colored by the worst of them. `nil` when
    /// the report is clean or hasn't run.
    private var doctorBadge: (count: Int, color: Color)? {
        guard let summary = store.doctorHealth?.summary else { return nil }
        let count = summary.fail + summary.warn
        guard count > 0 else { return nil }
        return (count, summary.fail > 0 ? .red : .orange)
    }

    private var brandHeader: some View {
        HStack(spacing: 9) {
            GradientTile(systemImage: "bolt.horizontal.fill", size: 30)
            VStack(alignment: .leading, spacing: 0) {
                Text("Dply Local").font(.headline)
                Text("local dev + dply").font(.caption2).foregroundStyle(.secondary)
            }
            Spacer()
        }
        .padding(.horizontal, 12)
        .padding(.top, 12)
        .padding(.bottom, 6)
        .background(.bar)
    }

    // MARK: Middle column

    @ViewBuilder
    private var listColumn: some View {
        switch store.section {
        case .local:
            List(selection: $selection) {
                if store.localSites.isEmpty && !store.isLoading {
                    ContentUnavailableView("No local sites", systemImage: "house",
                        description: Text("Use ➕ to link a project or park a folder. Each becomes <name>.test."))
                }
                ForEach(filteredLocalSites) { row in
                    DomainRow(site: row).tag(row.id)
                        .contextMenu { siteContextMenu(row) }
                }
            }
            .searchable(text: $siteSearch, placement: .automatic, prompt: "Filter domains")
            .task(id: siteSearch) {
                // Debounce: clearing is instant; typing settles after a short
                // pause. `.task(id:)` cancels + restarts on each change, so only
                // the last keystroke's task survives the sleep to commit.
                if siteSearch.isEmpty { debouncedSiteSearch = ""; return }
                try? await Task.sleep(nanoseconds: 200_000_000)
                guard !Task.isCancelled else { return }
                debouncedSiteSearch = siteSearch
            }
            .overlay { if store.isLoading { ProgressView().controlSize(.small) } }
        case .services:
            ServicesListView(selection: $selection)
        case .mail:
            MailListView(selection: $selection)
        case .dumps:
            DumpsListView(selection: $selection)
        case .dashboard, .php, .extensions, .status, .system, .doctor, .settings:
            EmptyView() // full-width surfaces — no middle column
        case .edgeSites:
            siteList(
                store.edgeSites, systemImage: "bolt.horizontal.circle.fill",
                emptyTitle: "No edge sites", emptyIcon: "bolt.horizontal.circle",
                emptyHint: dplyHint,
                title: ["name"], subtitle: ["build.framework", "framework", "live_url"],
                statusFor: { $0.cell(["status"]) }, activeFor: { _ in true }
            )
        case .sites:
            siteList(
                store.sites, systemImage: "server.rack",
                emptyTitle: "No server sites", emptyIcon: "server.rack",
                emptyHint: dplyHint,
                title: ["name"], subtitle: ["server.name", "server_name", "runtime"],
                statusFor: { $0.cell(["status"]) }, activeFor: { _ in true }
            )
        case .servers:
            siteList(
                store.servers, systemImage: "externaldrive.fill",
                emptyTitle: "No servers", emptyIcon: "externaldrive",
                emptyHint: dplyHint,
                title: ["name"], subtitle: ["provider", "region", "ip_address", "ip"],
                statusFor: { $0.cell(["status"]) }, activeFor: { _ in true }
            )
        case .account:
            AccountView()
        case .keelSites:
            siteList(
                store.keelSites, systemImage: "sailboat",
                emptyTitle: "No Keel Cloud sites", emptyIcon: "sailboat",
                emptyHint: "Log in under Keel Cloud, then refresh.",
                title: ["name"], subtitle: ["custom_hostname", "prod_hostname"],
                statusFor: { $0.cell(["status"]) }, activeFor: { $0.cell(["status"]) == "live" }
            )
        case .keelAccount:
            KeelAccountView()
        }
    }

    private var dplyHint: String {
        "Go to Account to log in to dply, then refresh."
    }

    /// Local sites filtered by the search box (name / host / project type).
    private var filteredLocalSites: [Row] {
        guard !debouncedSiteSearch.isEmpty else { return store.localSites }
        return store.localSites.filter {
            $0.cell(["name"]).localizedCaseInsensitiveContains(debouncedSiteSearch)
                || $0.cell(["host"]).localizedCaseInsensitiveContains(debouncedSiteSearch)
                || $0.cell(["framework"]).localizedCaseInsensitiveContains(debouncedSiteSearch)
        }
    }

    private func siteList(
        _ rows: [Row],
        systemImage: String,
        emptyTitle: String, emptyIcon: String, emptyHint: String,
        title: [String], subtitle: [String],
        statusFor: @escaping (Row) -> String,
        activeFor: @escaping (Row) -> Bool
    ) -> some View {
        List(selection: $selection) {
            if rows.isEmpty && !store.isLoading {
                ContentUnavailableView(emptyTitle, systemImage: emptyIcon, description: Text(emptyHint))
            }
            ForEach(rows) { row in
                ItemRow(
                    title: row.cell(title),
                    subtitle: row.cell(subtitle),
                    status: statusFor(row),
                    systemImage: systemImage,
                    active: activeFor(row)
                )
                .tag(row.id)
            }
        }
        .overlay { if store.isLoading { ProgressView().controlSize(.small) } }
    }

    // MARK: Detail column

    @ViewBuilder
    private var detailColumn: some View {
        switch store.section {
        case .local:
            if let id = selection, let row = store.localSites.first(where: { $0.id == id }) {
                LocalDetailView(site: row).id(id)
            } else { placeholder("Select a site", "house") }
        case .services:
            if let id = selection, let row = store.dbServices.first(where: { $0.cell(["name"]) == id }) {
                DatabasesView(service: row).id(id)
            } else {
                placeholder("Select an instance to manage databases", "cylinder.split.1x2")
            }
        case .mail:
            if let id = selection {
                MailDetailView(id: id).id(id)
            } else { placeholder("Select a message", "envelope") }
        case .dumps:
            if let id = selection, let n = Int(id), let dump = store.dumps.first(where: { $0.id == n }) {
                DumpDetailView(dump: dump).id(id)
            } else { placeholder("Select a dump", "ladybug") }
        case .dashboard, .php, .extensions, .status, .system, .doctor, .settings:
            EmptyView() // rendered by `fullWidthPage`
        case .edgeSites:
            if let id = selection { EdgeDetailView(siteID: id).id(id) } else { placeholder("Select an edge site", "bolt.horizontal.circle") }
        case .sites:
            if let id = selection { SiteDetailView(siteID: id).id(id) } else { placeholder("Select a site", "server.rack") }
        case .servers:
            if let id = selection, let row = store.servers.first(where: { $0.id == id }) {
                ServerDetailView(server: row).id(id)
            } else { placeholder("Select a server", "externaldrive") }
        case .account:
            AccountDetailView()
        case .keelSites:
            if let id = selection, let row = store.keelSites.first(where: { $0.id == id }) {
                KeelSiteDetailView(site: row).id(id)
            } else { placeholder("Select a site", "sailboat") }
        case .keelAccount:
            placeholder("Keel Cloud", "cloud")
        }
    }

    private func placeholder(_ title: String, _ icon: String) -> some View {
        ContentUnavailableView(title, systemImage: icon)
    }

    // MARK: Footer + toolbar

    private var hostFooter: some View {
        VStack(spacing: 0) {
            Divider()
            HStack(spacing: 7) {
                // The host/login indicator is meaningless without dply enabled.
                if store.dplyEnabled {
                    Circle()
                        .fill(store.isAuthenticated ? Theme.live : Color.secondary)
                        .frame(width: 7, height: 7)
                    Text(store.activeHost)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                Spacer()
                loadPill
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 7)
        }
        .background(.bar)
    }

    /// Right-click actions for a site in the domains list.
    @ViewBuilder
    private func siteContextMenu(_ row: Row) -> some View {
        let name = row.cell(["name"])
        let isProxy = row.cell(["source"]) == "proxy"
        Button("Open in Browser") { store.openURL(row.cell(["url"])) }
        if !isProxy {
            Button("Open Folder") { store.openFolder(row.cell(["path"])) }
            Button("Open in Editor") { store.openFolderInEditor(row.cell(["path"])) }
            Divider()
            let php = row.cell(["php"]).isEmpty ? (store.defaultPhp ?? "") : row.cell(["php"])
            Menu("Switch PHP Version") {
                ForEach(store.availablePhpVersions, id: \.self) { v in
                    Button("Always use PHP \(v)") { Task { await store.usePhp(version: v, site: name) } }
                }
            }
            Button("Manage Extensions (PHP \(php))…") {
                store.extensionsVersion = php.isEmpty ? nil : php
                store.section = .extensions
            }
            let secure = row.dig("secure") == .bool(true)
            Button(secure ? "Disable HTTPS" : "Secure (HTTPS)") {
                Task { await store.setLocalSecure(name: name, secure: !secure) }
            }
            Divider()
            Button("Unlink", role: .destructive) { Task { await store.unlinkLocal(name: name) } }
        } else {
            Button("Remove proxy", role: .destructive) { Task { await store.unproxyLocal(name: name) } }
        }
    }

    /// System-load chip: green when idle, orange when busy, red when the CPU is
    /// saturated enough to slow site serving.
    private var loadPill: some View {
        let sev = store.loadSeverity
        let color: Color = sev == 2 ? .red : (sev == 1 ? .orange : .green)
        return HStack(spacing: 4) {
            Image(systemName: sev == 2 ? "gauge.high" : (sev == 1 ? "gauge.medium" : "gauge.low"))
                .font(.caption2)
            Text(String(format: "%.1f", store.loadAvg)).font(.caption2.monospacedDigit())
        }
        .foregroundStyle(color)
        .padding(.horizontal, 6).padding(.vertical, 2)
        .background(color.opacity(0.12), in: Capsule())
        .help(sev == 2
              ? "CPU saturated (load \(String(format: "%.1f", store.loadAvg)) on \(store.cpuCount) cores) — sites may hang. Reduce background load."
              : "System load \(String(format: "%.1f", store.loadAvg)) across \(store.cpuCount) cores")
    }

    @ToolbarContentBuilder
    private var toolbarContent: some ToolbarContent {
        if store.section == .local {
            ToolbarItem(placement: .primaryAction) {
                Menu {
                    Button("Link a project…") { pickFolder(park: false) }
                    Button("Park a folder…") { pickFolder(park: true) }
                    Divider()
                    Button("Add a proxy…") { showAddProxy = true }
                    Button("Import from Valet…") { showValetImport = true }
                    Button("Clean up duplicates…") { showCleanup = true }
                } label: {
                    Image(systemName: "plus")
                }
                .help("Add a local site or proxy")
            }
        }
        if store.section == .mail && !store.mailMessages.isEmpty {
            ToolbarItem(placement: .primaryAction) {
                Button {
                    Task { await store.clearMail() }
                } label: {
                    Image(systemName: "trash")
                }
                .help(store.mailboxFilter.map { "Clear the \(Store.mailboxLabel($0)) mailbox" }
                    ?? "Clear all captured mail")
            }
        }
        if store.section == .dumps && !store.dumps.isEmpty {
            ToolbarItem(placement: .primaryAction) {
                Button {
                    Task { await store.clearDumps() }
                } label: {
                    Image(systemName: "trash")
                }
                .help("Clear all dumps")
            }
        }
        ToolbarItem(placement: .primaryAction) {
            Button {
                Task { await store.refreshAll() }
            } label: {
                Image(systemName: "arrow.clockwise")
            }
            .help("Refresh")
            .disabled(store.isLoading)
        }
    }

    /// Prompt for a directory and either link it as one site or park it so its
    /// subfolders each become sites.
    private func pickFolder(park: Bool) {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.prompt = park ? "Park" : "Link"
        panel.message = park
            ? "Choose a folder whose subdirectories are your projects."
            : "Choose a single project directory to serve as <name>.test."
        if panel.runModal() == .OK, let url = panel.url {
            if park {
                Task { await store.parkLocal(path: url.path) }
            } else {
                // Show the Link-a-Folder dialog to customize name + secure.
                pendingLinkPath = url.path
                showLinkSheet = true
            }
        }
    }
}
