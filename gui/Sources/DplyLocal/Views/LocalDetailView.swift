import SwiftUI

/// Detail pane for a local `.test` site: overview + open / secure / unlink.
/// The data comes straight from `dpl sites --json` (a `SiteInfo`).
struct LocalDetailView: View {
    @EnvironmentObject var store: Store
    let site: Row
    @State private var showLogs = false
    @State private var showShare = false
    @State private var showParity = false
    @State private var showAddTld = false
    @State private var newTld = ""
    @State private var xdebugBusy = false

    private var name: String { site.cell(["name"]) }
    private var url: String { site.cell(["url"]) }
    private var isSecure: Bool { site.dig("secure") == .bool(true) }
    private var isServing: Bool { site.dig("serving") == .bool(true) }
    private var isLinked: Bool { site.cell(["source"]) == "linked" }
    private var isProxy: Bool { site.cell(["source"]) == "proxy" }
    private var phpLabel: String {
        let v = site.cell(["php"])
        return v.isEmpty ? "default" : v
    }
    private var requiresPhp: String { site.cell(["requires_php"]) }
    /// The PHP version this site actually runs on (its pin, else the default).
    private var effectivePhp: String {
        let pinned = site.cell(["php"])
        return pinned.isEmpty ? (store.defaultPhp ?? "") : pinned
    }
    /// Does the effective PHP satisfy the site's `require.php` constraint?
    private var phpCompatible: Bool? {
        guard !requiresPhp.isEmpty, !effectivePhp.isEmpty else { return nil }
        return phpSatisfies(requiresPhp, effectivePhp)
    }

    // MARK: Xdebug

    /// This site's Xdebug state, from the daemon. Falls back to `off` while the
    /// first `dpl xdebug` call is still in flight.
    private var xdebug: XdebugSite {
        store.xdebug.site(name)
            ?? XdebugSite(name: name, php: nil, mode: "off", installed: false)
    }

    /// The single mode the segmented control shows. Xdebug modes compose, but
    /// the picker offers one at a time; `debug` wins when several are set so a
    /// site mid-debug never looks like it's only profiling.
    private var xdebugChoice: String {
        for m in ["debug", "profile", "trace", "develop"] where xdebug.has(m) { return m }
        return "off"
    }

    /// Per-site Xdebug. Modes are stored per *linked* site, exactly as PHP
    /// version pinning and HTTPS are, so parked sites can only follow the
    /// machine-wide default.
    @ViewBuilder
    private var xdebugSection: some View {
        DetailSection(title: "Xdebug") {
            VStack(alignment: .leading, spacing: 10) {
                if !isLinked {
                    Text("Parked sites follow the default Xdebug mode (currently **\(store.xdebug.sites.first { $0.name == name }?.mode ?? "off")**). Link this site to give it a mode of its own.")
                        .font(.caption).foregroundStyle(.secondary)
                } else if !xdebug.installed {
                    Label("Xdebug isn't installed for PHP \(effectivePhp.isEmpty ? "this version" : effectivePhp). Install it from the Extensions page.",
                          systemImage: "exclamationmark.triangle.fill")
                        .font(.caption).foregroundStyle(.orange)
                }

                Picker("", selection: Binding(
                    get: { xdebugChoice },
                    set: { mode in
                        Task {
                            xdebugBusy = true
                            await store.setXdebug(mode: mode, site: name)
                            xdebugBusy = false
                        }
                    })) {
                    Text("Off").tag("off")
                    Text("Step Debug").tag("debug")
                    Text("Develop").tag("develop")
                    Text("Profile").tag("profile")
                    Text("Trace").tag("trace")
                }
                .pickerStyle(.segmented).labelsHidden()
                .disabled(xdebugBusy || !isLinked || !xdebug.installed)

                Group {
                    switch xdebugChoice {
                    case "debug":
                        Text("Listen on **127.0.0.1:\(store.xdebug.clientPort)** (IDE key `\(store.xdebug.ideKey)`), set a breakpoint, reload the site.")
                    case "develop":
                        Text("Better error messages and `var_dump` output. No breakpoints.")
                    case "profile":
                        Text("Writes a cachegrind profile per request to `~/.dpl/xdebug`. Open it with QCachegrind or PhpStorm.")
                    case "trace":
                        Text("Writes a full function trace per request to `~/.dpl/xdebug`. Verbose — turn it off when you're done.")
                    default:
                        Text("Off — this site runs on the shared pool with no Xdebug loaded, at full speed.")
                    }
                }
                .font(.caption).foregroundStyle(.secondary)

                if xdebugChoice != "off" {
                    Text("Only this site is affected; the others keep running without Xdebug.")
                        .font(.caption2).foregroundStyle(Theme.violet)
                }
            }
        }
    }

    // MARK: Profiler

    @State private var profileBusy = false
    private var isProfiling: Bool { site.dig("profile") == .bool(true) }
    private var profileInstalled: Bool { site.dig("profile_installed") == .bool(true) }

    /// Per-site SPX profiler: one toggle, plus a link to the same-origin flame
    /// graphs. Turning it on installs SPX for this PHP if needed.
    @ViewBuilder
    private var profilerSection: some View {
        DetailSection(title: "Profiler") {
            VStack(alignment: .leading, spacing: 10) {
                if !isLinked {
                    Text("Link this site to profile it on a pool of its own.")
                        .font(.caption).foregroundStyle(.secondary)
                }
                HStack(spacing: 10) {
                    Toggle(isOn: Binding(
                        get: { isProfiling },
                        set: { on in
                            Task {
                                profileBusy = true
                                await store.setProfile(name: name, on: on)
                                profileBusy = false
                            }
                        })) {
                        Text(isProfiling ? "On — every request is captured" : "Off")
                    }
                    .toggleStyle(.switch).tint(Theme.violet)
                    .disabled(profileBusy || !isLinked)
                    if profileBusy { ProgressView().controlSize(.small) }
                    Spacer()
                    if isProfiling {
                        Button {
                            if let u = URL(string: "\(url.trimmingCharacters(in: CharacterSet(charactersIn: "/")))/?SPX_UI_URI=/&SPX_KEY=dpl-local") {
                                NSWorkspace.shared.open(u)
                            }
                        } label: { Label("Flame graphs", systemImage: "flame") }
                    }
                }
                Text(isProfiling
                    ? "SPX auto-profiles every request into a flame graph at this site's own origin."
                    : "Off — this site runs on the shared pool with no SPX, at full speed.")
                    .font(.caption).foregroundStyle(.secondary)
                if !profileInstalled {
                    Text("SPX will be installed for PHP \(effectivePhp.isEmpty ? "this version" : effectivePhp) when you turn it on.")
                        .font(.caption2).foregroundStyle(.secondary)
                }
            }
        }
    }

    // MARK: Node

    @State private var nodeBusy = false
    @State private var editingNode = false
    @State private var nodeDraft = ""
    private var nodeVersion: String? { site.first(["node"]) }
    private var nodeSource: String? { site.first(["node_source"]) }
    private let nodePicks = ["22", "20", "18"]

    /// Per-site Node version — writes the repo's `.nvmrc`; fnm/nvm switch on `cd`.
    @ViewBuilder
    private var nodeSection: some View {
        DetailSection(title: "Node") {
            VStack(alignment: .leading, spacing: 10) {
                HStack(spacing: 8) {
                    if let v = nodeVersion {
                        Text("Node \(v)").font(.callout.weight(.medium))
                        Text("· \(nodeSource ?? ".nvmrc")").font(.caption).foregroundStyle(.secondary)
                    } else {
                        Text("unpinned").font(.callout).foregroundStyle(.secondary)
                    }
                    if nodeBusy { ProgressView().controlSize(.small) }
                    Spacer()
                }
                if editingNode {
                    HStack(spacing: 6) {
                        TextField("e.g. 20.11.0", text: $nodeDraft)
                            .textFieldStyle(.roundedBorder).frame(width: 130)
                            .onSubmit { Task { await pinNode(nodeDraft) } }
                        Button("Pin") { Task { await pinNode(nodeDraft) } }
                            .buttonStyle(.borderedProminent).tint(Theme.violet)
                            .disabled(nodeDraft.trimmingCharacters(in: .whitespaces).isEmpty)
                        Button("Cancel") { editingNode = false }.buttonStyle(.bordered)
                    }
                } else {
                    HStack(spacing: 6) {
                        ForEach(nodePicks, id: \.self) { v in
                            Button(v) { Task { await pinNode(v) } }
                                .buttonStyle(.bordered)
                                .tint(nodeVersion == v ? Theme.violet : nil)
                        }
                        // Only offer Detect when package.json actually has an
                        // engines.node to read — otherwise it can only fail.
                        if nodeSource == "package.json" {
                            Button {
                                Task { nodeBusy = true; await store.detectNodeVersion(name: name); nodeBusy = false }
                            } label: { Label("Detect", systemImage: "wand.and.stars") }
                                .buttonStyle(.bordered)
                                .help("Pin the version from this repo's package.json engines.node")
                        }
                        Button { nodeDraft = nodeVersion ?? ""; editingNode = true } label: {
                            Image(systemName: "pencil")
                        }.buttonStyle(.bordered)
                        Spacer()
                    }
                    .disabled(nodeBusy)
                }
                Text(store.nodeManager.map { "\($0) switches to this version when you `cd` in." }
                    ?? "Install fnm or nvm to auto-switch when you `cd` in.")
                    .font(.caption2).foregroundStyle(.secondary)
            }
        }
        .task { if store.nodeManager == nil { await store.loadNodeManager() } }
    }

    private func pinNode(_ version: String) async {
        let v = version.trimmingCharacters(in: .whitespaces)
        guard !v.isEmpty else { return }
        editingNode = false
        nodeBusy = true
        await store.setNodeVersion(name: name, version: v)
        nodeBusy = false
    }

    /// Minimal PHP version-constraint check (`^8.3`, `~8.1`, `>=8.1`, `8.2.*`).
    private func phpSatisfies(_ constraint: String, _ version: String) -> Bool {
        func parse(_ s: String) -> (Int, Int)? {
            let nums = s.split(whereSeparator: { !"0123456789.".contains($0) }).first.map(String.init) ?? s
            let parts = nums.split(separator: ".").compactMap { Int($0) }
            guard parts.count >= 2 else { return parts.first.map { ($0, 0) } }
            return (parts[0], parts[1])
        }
        guard let (vMaj, vMin) = parse(version), let (cMaj, cMin) = parse(constraint) else { return true }
        if constraint.contains("^") { return vMaj == cMaj && vMin >= cMin }
        if constraint.contains("~") { return vMaj == cMaj && vMin >= cMin }
        if constraint.contains(">=") { return vMaj > cMaj || (vMaj == cMaj && vMin >= cMin) }
        return vMaj == cMaj && vMin == cMin
    }

    var body: some View {
        if isProxy { proxyDetail } else { siteDetail }
    }

    // MARK: Proxy detail

    private var proxyDetail: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                HStack(spacing: 12) {
                    GradientTile(systemImage: "arrow.triangle.branch", size: 44)
                    VStack(alignment: .leading, spacing: 3) {
                        Text(site.cell(["host"])).font(.title2.weight(.bold))
                        Text("proxy").font(.caption).foregroundStyle(.secondary)
                    }
                    Spacer()
                    StatusBadge(status: "serving")
                }
                HStack(spacing: 10) {
                    Button { store.openURL(url) } label: { Label("Open", systemImage: "safari") }
                        .buttonStyle(.borderedProminent).tint(Theme.violet)
                    Button(role: .destructive) {
                        Task { await store.unproxyLocal(name: name) }
                    } label: { Label("Remove proxy", systemImage: "trash") }
                    Spacer()
                }
                DetailSection(title: "Proxy") {
                    KeyValueView(row: site, fields: [
                        ("Host", ["host"]), ("URL", ["url"]), ("Target", ["path"]),
                    ])
                }
                Text("Requests to \(site.cell(["host"])) are forwarded to the target service.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            .padding(18)
        }
    }

    // MARK: Site detail

    private var siteDetail: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                HStack(spacing: 12) {
                    GradientTile(systemImage: "globe", size: 44, active: isServing)
                    VStack(alignment: .leading, spacing: 3) {
                        Text(site.cell(["host"]))
                            .font(.title2.weight(.bold))
                        Text(site.cell(["path"]))
                            .font(.system(.caption, design: .monospaced))
                            .foregroundStyle(.secondary)
                            .lineLimit(1).truncationMode(.middle)
                    }
                    Spacer()
                    StatusBadge(status: isServing ? "serving" : "stopped")
                }

                VStack(alignment: .leading, spacing: 10) {
                    // Primary row: open + the two menus (domain, PHP).
                    HStack(spacing: 10) {
                        Button {
                            store.openURL(url)
                        } label: {
                            Label("Open", systemImage: "safari")
                        }
                        .buttonStyle(.borderedProminent)
                        .tint(Theme.violet)
                        .disabled(!isServing)
                        .fixedSize()

                        domainMenu
                        if isLinked { phpMenu }
                        if isLinked { runtimeMenu }
                        Spacer()
                    }

                    // Secondary row: secure / logs / share / unlink.
                    HStack(spacing: 10) {
                        Button {
                            Task { await store.setLocalSecure(name: name, secure: !isSecure) }
                        } label: {
                            Label(isSecure ? "Disable HTTPS" : "Secure",
                                  systemImage: isSecure ? "lock.open" : "lock")
                        }
                        .disabled(!isLinked)
                        .help(isLinked ? "Trust HTTPS for this site" : "HTTPS applies to linked sites")
                        .fixedSize()

                        Button { showLogs = true } label: { Label("Logs", systemImage: "text.alignleft") }
                            .disabled(!isServing).fixedSize()
                        Button { showShare = true } label: { Label("Share", systemImage: "antenna.radiowaves.left.and.right") }
                            .disabled(!isServing).fixedSize()

                        // Parity only means something for a linked project that
                        // has a dply site to deploy to.
                        if isLinked && store.dplyEnabled {
                            Button { showParity = true } label: {
                                Label("Deploy parity", systemImage: "arrow.left.arrow.right")
                            }
                            .fixedSize()
                            .help("Compare this project with the dply site it deploys to")
                        }

                        if isLinked {
                            Button(role: .destructive) {
                                Task { await store.unlinkLocal(name: name) }
                            } label: {
                                Label("Unlink", systemImage: "trash")
                            }
                            .fixedSize()
                        }
                        Spacer()
                    }
                }
                .controlSize(.regular)

                DetailSection(title: "Overview") {
                    KeyValueView(row: site, fields: [
                        ("Host", ["host"]),
                        ("URL", ["url"]),
                        ("Project type", ["framework"]),
                        ("Requires PHP", ["requires_php"]),
                        ("Serving", ["serving"]),
                        ("HTTPS", ["secure"]),
                        ("PHP", ["php"]),
                        ("Runtime", ["runtime"]),
                        ("Project", ["path"]),
                    ])
                }

                if !isProxy {
                    xdebugSection
                    profilerSection
                    nodeSection
                }

                if !requiresPhp.isEmpty {
                    DetailSection(title: "PHP compatibility") {
                        VStack(alignment: .leading, spacing: 6) {
                            Text("Requires **PHP \(requiresPhp)** · running **PHP \(effectivePhp.isEmpty ? "?" : effectivePhp)**")
                                .font(.callout)
                            switch phpCompatible {
                            case .some(true):
                                Label("The active PHP version is ideal for this site.", systemImage: "checkmark.circle.fill")
                                    .foregroundStyle(.green).font(.caption)
                            case .some(false):
                                Label("This site wants PHP \(requiresPhp). Isolate it to a matching version with the PHP menu above.", systemImage: "exclamationmark.triangle.fill")
                                    .foregroundStyle(.orange).font(.caption)
                            case .none:
                                EmptyView()
                            }
                        }
                    }
                }

                if !isServing {
                    Text("This site isn't being served — check the daemon log (the folder may be missing or PHP failed to start).")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }
            }
            .padding(18)
        }
        .sheet(isPresented: $showLogs) { SiteLogsSheet(site: name).environmentObject(store) }
        .sheet(isPresented: $showParity) { ParitySheet(site: name).environmentObject(store) }
        .sheet(isPresented: $showShare) { ShareSheet(site: name).environmentObject(store) }
        .sheet(isPresented: $showAddTld) { addTldSheet }
        .task { await store.loadTlds() }
    }

    // MARK: Action-bar menus

    /// The primary domain (TLD) picker. TLDs are global — every site answers on
    /// all of them; this sets which one builds the canonical URL.
    private var domainMenu: some View {
        let primary = store.tlds.first ?? "test"
        return Menu {
            Section("Primary domain (all sites)") {
                ForEach(store.tlds, id: \.self) { t in
                    Button {
                        Task { await store.setPrimaryTld(t) }
                    } label: {
                        if t == primary { Label(".\(t)", systemImage: "checkmark") }
                        else { Text(".\(t)") }
                    }
                }
            }
            Divider()
            Button("Add a TLD…") { showAddTld = true }
        } label: {
            Label(".\(primary)", systemImage: "globe")
        }
        .menuStyle(.borderlessButton)
        .fixedSize()
        .help("Change the domain your sites use")
    }

    private var phpMenu: some View {
        Menu {
            ForEach(store.availablePhpVersions, id: \.self) { v in
                Button("PHP \(v)") { Task { await store.usePhp(version: v, site: name) } }
            }
        } label: {
            Label("PHP \(phpLabel)", systemImage: "chevron.left.forwardslash.chevron.right")
        }
        .menuStyle(.borderlessButton)
        .fixedSize()
    }

    /// The application-server runtime picker: php-fpm (default) or Laravel
    /// Octane on Swoole / RoadRunner / FrankenPHP.
    private var runtimeMenu: some View {
        let current = site.cell(["runtime"])
        let label = current.isEmpty || current == "fpm" ? "php-fpm" : current.replacingOccurrences(of: "octane-", with: "Octane ")
        return Menu {
            Button("php-fpm (default)") { Task { await store.setRuntime(site: name, runtime: "fpm") } }
            Section("Laravel Octane (installs into project)") {
                Button("Swoole…") { store.runOctaneSetupInTerminal(site: name, server: "swoole") }
                Button("RoadRunner…") { store.runOctaneSetupInTerminal(site: name, server: "roadrunner") }
                Button("FrankenPHP…") { store.runOctaneSetupInTerminal(site: name, server: "frankenphp") }
            }
        } label: {
            Label(label, systemImage: "bolt.horizontal.circle")
        }
        .menuStyle(.borderlessButton)
        .fixedSize()
        .help("Run this site on php-fpm or a Laravel Octane server")
    }

    private var addTldSheet: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Add a TLD").font(.headline)
            Text("Your sites will also answer on <name>.<tld>. Run `dpl setup` (Status → Run setup) afterward so the new TLD resolves.")
                .font(.caption).foregroundStyle(.secondary)
            TextField("tld, e.g. localhost", text: $newTld)
                .textFieldStyle(.roundedBorder)
                .onSubmit(addTld)
            HStack {
                Spacer()
                Button("Cancel") { showAddTld = false; newTld = "" }
                Button("Add", action: addTld)
                    .buttonStyle(.borderedProminent).tint(Theme.violet)
                    .disabled(newTld.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        }
        .padding(18)
        .frame(width: 400)
    }

    private func addTld() {
        let t = newTld.trimmingCharacters(in: .whitespaces).trimmingCharacters(in: CharacterSet(charactersIn: "."))
        guard !t.isEmpty else { return }
        Task { await store.addTld(t) }
        showAddTld = false
        newTld = ""
    }
}
