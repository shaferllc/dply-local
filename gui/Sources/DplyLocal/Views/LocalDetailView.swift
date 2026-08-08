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
    private var isLaravel: Bool { site.cell(["framework"]).localizedCaseInsensitiveContains("laravel") }
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

    // MARK: Branch-aware database

    @State private var dbBusy = false
    @State private var dbBranches: [Store.DbBranch] = []
    private var dbName: String? { site.first(["database"]) }
    private var dbBranch: String? { site.first(["db_branch"]) }

    /// Branch-aware databases: one Postgres DB per git branch, switched
    /// automatically on checkout. Linked sites only (like PHP pins and HTTPS).
    @ViewBuilder
    private var databaseSection: some View {
        DetailSection(title: "Database") {
            VStack(alignment: .leading, spacing: 10) {
                if !isLinked {
                    Text("Link this site to give each git branch its own database.")
                        .font(.caption).foregroundStyle(.secondary)
                } else if let db = dbName {
                    HStack(spacing: 8) {
                        Text(db).font(.system(.callout, design: .monospaced).weight(.medium))
                        if let branch = dbBranch {
                            Text(branch)
                                .font(.caption.weight(.medium))
                                .padding(.horizontal, 7).padding(.vertical, 2)
                                .background(Theme.violet.opacity(0.18), in: Capsule())
                                .foregroundStyle(Theme.violet)
                        }
                        if dbBusy { ProgressView().controlSize(.small) }
                        Spacer()
                        Button(role: .destructive) {
                            Task { dbBusy = true; await store.detachBranchDb(name: name); dbBusy = false }
                        } label: { Text("Detach") }
                        .buttonStyle(.bordered).controlSize(.small)
                    }
                    if dbBranches.count > 1 || dbBranches.contains(where: { !$0.live }) {
                        VStack(alignment: .leading, spacing: 4) {
                            ForEach(dbBranches) { b in
                                HStack(spacing: 8) {
                                    Image(systemName: b.live ? "circle.fill" : "circle")
                                        .font(.system(size: 7))
                                        .foregroundStyle(b.live ? Theme.live : .secondary)
                                    Text(b.branch).font(.system(.caption, design: .monospaced))
                                    Text(b.size).font(.caption2).foregroundStyle(.tertiary)
                                    if b.live {
                                        Text("live").font(.caption2).foregroundStyle(Theme.live)
                                    }
                                    Spacer()
                                    if !b.live {
                                        Button {
                                            Task {
                                                dbBusy = true
                                                await store.dropDbBranch(name: name, branch: b.branch)
                                                dbBranches = await store.dbBranches(name: name)
                                                dbBusy = false
                                            }
                                        } label: { Image(systemName: "trash").font(.caption2) }
                                        .buttonStyle(.borderless).foregroundStyle(.tertiary)
                                        .help("Drop this branch's parked database")
                                    }
                                }
                            }
                        }
                    }
                    Text("Each git branch keeps its own copy — `git checkout` switches the database automatically.")
                        .font(.caption).foregroundStyle(.secondary)
                } else {
                    HStack(spacing: 10) {
                        Button {
                            Task { dbBusy = true; await store.attachBranchDb(name: name); dbBusy = false }
                        } label: { Label("Attach", systemImage: "cylinder.split.1x2") }
                        .buttonStyle(.bordered)
                        .disabled(dbBusy)
                        if dbBusy { ProgressView().controlSize(.small) }
                        Spacer()
                    }
                    Text("Give each git branch its own database (from this project's `.env` DB_DATABASE). Checkouts then switch it automatically — migrations on a branch never bleed into another.")
                        .font(.caption).foregroundStyle(.secondary)
                }
            }
        }
        .task(id: "\(name)-\(dbName ?? "")-\(dbBranch ?? "")") {
            dbBranches = dbName == nil ? [] : await store.dbBranches(name: name)
        }
    }

    // MARK: Tags

    @State private var tagDraft = ""
    @State private var tagsBusy = false
    private var tags: [String] { Store.tags(of: site) }

    /// Free-form labels for this site. The one grouping axis detection can't
    /// reach — which client it's for, whether it's archived, what's mid-rewrite.
    @ViewBuilder
    private var tagsSection: some View {
        DetailSection(title: "Tags") {
            VStack(alignment: .leading, spacing: 8) {
                if tags.isEmpty {
                    Text("No tags. Group this site with others by giving it one.")
                        .font(.caption).foregroundStyle(.secondary)
                } else {
                    // A wrapping row, so twenty tags don't run off the pane.
                    FlowRow(spacing: 5) {
                        ForEach(tags, id: \.self) { tag in
                            HStack(spacing: 3) {
                                Text(tag).font(.system(size: 10, weight: .medium))
                                Button {
                                    Task { await save(tags.filter { $0 != tag }) }
                                } label: {
                                    Image(systemName: "xmark").font(.system(size: 7, weight: .bold))
                                }
                                .buttonStyle(.borderless)
                            }
                            .padding(.horizontal, 6).padding(.vertical, 2)
                            .background(Theme.violet.opacity(0.16), in: Capsule())
                            .foregroundStyle(Theme.violet)
                        }
                    }
                }
                HStack(spacing: 6) {
                    TextField("Add a tag", text: $tagDraft, prompt: Text("client-x"))
                        .textFieldStyle(.roundedBorder).frame(width: 160)
                        .onSubmit { Task { await addTag() } }
                    Button("Add") { Task { await addTag() } }
                        .buttonStyle(.bordered)
                        .disabled(tagDraft.trimmingCharacters(in: .whitespaces).isEmpty)
                    if tagsBusy { ProgressView().controlSize(.small) }
                    Spacer()
                }
                .disabled(tagsBusy)
                Text("Lowercased and hyphenated on save, so one idea stays one tag. Group the sidebar by tag to use them.")
                    .font(.caption2).foregroundStyle(.secondary)
            }
        }
    }

    private func addTag() async {
        let new = tagDraft.trimmingCharacters(in: .whitespaces)
        guard !new.isEmpty else { return }
        tagDraft = ""
        await save(tags + [new])
    }

    private func save(_ next: [String]) async {
        tagsBusy = true
        await store.setTags(site: name, tags: next)
        tagsBusy = false
    }

    // MARK: Node

    @State private var nodeBusy = false
    @State private var editingNode = false
    @State private var nodeDraft = ""
    @State private var scripts: [String] = []
    @State private var nodeRun: NodeRunRequest?
    private var nodeVersion: String? { site.first(["node"]) }
    private var nodeSource: String? { site.first(["node_source"]) }
    private let nodePicks = ["22", "20", "18"]

    /// The package manager this repo calls for. `nil` means no package.json, so
    /// there are no dependencies to install and no scripts to run.
    private var nodeAgent: String? { site.first(["node_agent"]) }
    private var nodeAgentSource: String? { site.first(["node_agent_source"]) }

    // MARK: Dev server

    @State private var devBusy = false
    /// The script the daemon supervises for this site, if any.
    private var devScript: String? { site.first(["dev"]) }
    private var devRunning: Bool { site.dig("dev_running") == .bool(true) }
    private var devPort: String? { site.first(["dev_port"]) }
    /// What to start when the toggle goes on: the conventional `dev`, else
    /// whatever the project actually defines.
    private var defaultDevScript: String? {
        scripts.contains("dev") ? "dev" : scripts.first
    }

    /// The supervised dev server. Unlike the Run menu above, this is a *setting*
    /// — the daemon keeps the process alive across app restarts, so the control
    /// is a toggle rather than a button that opens a window you mustn't close.
    @ViewBuilder
    private var devServerRow: some View {
        HStack(spacing: 8) {
            Toggle("Dev server", isOn: Binding(
                get: { devScript != nil },
                set: { on in
                    Task {
                        devBusy = true
                        await store.setDevServer(site: name, script: on ? defaultDevScript : nil)
                        devBusy = false
                    }
                }
            ))
            .toggleStyle(.switch)
            .controlSize(.mini)
            .disabled(devBusy || (devScript == nil && defaultDevScript == nil))

            if let script = devScript {
                Menu(script) {
                    ForEach(scripts, id: \.self) { candidate in
                        Button(candidate) {
                            Task {
                                devBusy = true
                                await store.setDevServer(site: name, script: candidate)
                                devBusy = false
                            }
                        }
                    }
                }
                .menuStyle(.borderlessButton)
                .frame(width: 74)
                .disabled(devBusy)

                if devRunning {
                    if let port = devPort, !port.isEmpty {
                        Button("localhost:\(port)") { store.openURL("http://localhost:\(port)") }
                            .buttonStyle(.link)
                            .help("Open the dev server")
                    } else {
                        // Watchers and type-checkers never listen on anything;
                        // that's normal, not a fault worth flagging.
                        Label("running", systemImage: "circle.fill")
                            .font(.caption2).foregroundStyle(Theme.live).labelStyle(.titleAndIcon)
                    }
                } else {
                    Label("stopped", systemImage: "circle.fill")
                        .font(.caption2).foregroundStyle(.orange)
                }

                Button {
                    Task { devBusy = true; await store.restartDevServer(site: name); devBusy = false }
                } label: { Image(systemName: "arrow.clockwise") }
                    .buttonStyle(.borderless).disabled(devBusy)
                    .help("Restart the dev server")

                Button {
                    // `--follow`: the sheet already streams a long-running
                    // process and kills it on close, so a live tail costs
                    // nothing extra and is the right shape for a dev server.
                    nodeRun = NodeRunRequest(
                        title: "Dev server logs",
                        scope: name,
                        args: ["dev", "logs", name, "--follow"]
                    )
                } label: { Image(systemName: "doc.plaintext") }
                    .buttonStyle(.borderless)
                    .help("What the dev server has printed")
            } else if defaultDevScript == nil {
                Text("no scripts to run").font(.caption2).foregroundStyle(.secondary)
            }
            if devBusy { ProgressView().controlSize(.small) }
            Spacer()
        }
    }

    /// Dependency install / script running for this one site. Everything here
    /// shells out to `dpl node …`, so the buttons do exactly what the documented
    /// commands do — including picking the right agent per repo.
    @ViewBuilder
    private var nodeActions: some View {
        if let agent = nodeAgent {
            Divider().padding(.vertical, 2)
            devServerRow
            Text("The daemon keeps this running and restarts it if it dies — it isn't tied to this window. PHP still serves the site; the dev server is a side-car on its own port.")
                .font(.caption2).foregroundStyle(.secondary)
            Divider().padding(.vertical, 2)
            HStack(spacing: 6) {
                Button {
                    nodeRun = NodeRunRequest(
                        title: "Install dependencies",
                        scope: name,
                        args: ["node", "deps", "--site", name]
                    )
                } label: { Label("Install", systemImage: "arrow.down.circle") }
                    .buttonStyle(.bordered)
                    .help("\(agent) install in this site, under its Node pin")

                Menu {
                    if scripts.isEmpty {
                        Text("No scripts in package.json")
                    } else {
                        ForEach(scripts, id: \.self) { script in
                            Button(script) {
                                nodeRun = NodeRunRequest(
                                    title: "Run \(script)",
                                    scope: name,
                                    args: ["node", "run", "--site", name, script]
                                )
                            }
                        }
                    }
                } label: { Label("Run", systemImage: "play") }
                    .menuStyle(.borderlessButton)
                    .frame(width: 78)
                    .disabled(scripts.isEmpty)

                Spacer()
                Text("\(agent)\(nodeAgentSource.map { " · \($0)" } ?? "")")
                    .font(.caption2).foregroundStyle(.secondary)
                    .help("Detected from this repo — override per run with `dpl node deps --agent`")
            }
            .task(id: name) { scripts = await store.nodeScripts(site: name) }
        }
    }

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
                nodeActions
            }
        }
        .task { if store.nodeManager == nil { await store.loadNodeManager() } }
        .sheet(item: $nodeRun) { run in
            NodeRunSheet(title: run.title, scope: run.scope, args: run.args)
                .environmentObject(store)
        }
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

    // MARK: Site path

    private var sitePath: String { site.cell(["path"]) }

    /// A project that has been moved or deleted out from under dply. The daemon
    /// still has the old path, so the site is listed but can never serve.
    private var pathMissing: Bool {
        !sitePath.isEmpty && !FileManager.default.fileExists(atPath: sitePath)
    }

    /// The path under the site's name. For a linked site it's a button: projects
    /// get moved, and re-linking by hand costs you every setting on the site.
    /// Parked sites take their path from the parked folder, so it's just text.
    @ViewBuilder
    private var pathLine: some View {
        let label = HStack(spacing: 4) {
            if pathMissing {
                Image(systemName: "exclamationmark.triangle.fill").font(.caption2)
            }
            Text(sitePath)
                .font(.system(.caption, design: .monospaced))
                .lineLimit(1).truncationMode(.middle)
            if isLinked {
                Image(systemName: "pencil").font(.caption2)
            }
        }
        .foregroundStyle(pathMissing ? AnyShapeStyle(Color.orange) : AnyShapeStyle(.secondary))

        if isLinked {
            Button { choosePath() } label: { label }
                .buttonStyle(.plain)
                .help(pathMissing
                      ? "This folder no longer exists — choose where the project moved to"
                      : "Change this site's folder")
        } else {
            label.help("Parked sites follow their parked folder")
        }
    }

    private func choosePath() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.prompt = "Use Folder"
        panel.message = "Choose the folder \(site.cell(["host"])) should serve."
        // Open where the project used to be, so a move within the same parent is
        // one click. If that's gone, fall back to the nearest surviving parent.
        var start = URL(fileURLWithPath: sitePath)
        let fm = FileManager.default
        while start.pathComponents.count > 1, !fm.fileExists(atPath: start.path) {
            start = start.deletingLastPathComponent()
        }
        panel.directoryURL = start
        guard panel.runModal() == .OK, let url = panel.url, url.path != sitePath else { return }
        Task { await store.relinkLocal(name: name, path: url.path) }
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
                        pathLine
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

                        // Tinker only exists for Laravel; opens in Terminal (a REPL).
                        if isLaravel {
                            Button { store.openTinker(name) } label: {
                                Label("Tinker", systemImage: "terminal")
                            }
                            .fixedSize()
                            .help("Open a Tinker REPL on this site's PHP")
                        }

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

                SiteProjectSection(projectPath: site.cell(["path"])).environmentObject(store)

                if isLinked { tagsSection }

                if !isProxy {
                    xdebugSection
                    profilerSection
                    databaseSection
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

    /// Whether this site runs on Octane rather than php-fpm.
    private var isOctane: Bool {
        let runtime = site.cell(["runtime"])
        return !runtime.isEmpty && runtime != "fpm"
    }

    /// Whether saving a file reloads this site's Octane workers.
    private var isWatching: Bool { site.dig("watch") == .bool(true) }

    /// The application-server runtime picker: php-fpm (default) or Laravel
    /// Octane on Swoole / RoadRunner / FrankenPHP.
    ///
    /// An Octane site gets the two actions php-fpm never needs: reload, because
    /// the workers are holding your application in memory, and the watch toggle
    /// that does it for you on every save.
    private var runtimeMenu: some View {
        let current = site.cell(["runtime"])
        let label = current.isEmpty || current == "fpm" ? "php-fpm" : current.replacingOccurrences(of: "octane-", with: "Octane ")
        return Menu {
            if isOctane {
                Section("This Octane server") {
                    Button("Reload workers") { Task { await store.reloadOctane(site: name) } }
                    Button("Restart server") { Task { await store.restartOctane(site: name) } }
                    Toggle("Reload when I save", isOn: Binding(
                        get: { isWatching },
                        set: { on in Task { await store.setOctaneWatch(site: name, on: on) } }
                    ))
                }
            }
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
        .help(isOctane
            ? "Reload this site's Octane workers, or move it back to php-fpm"
            : "Run this site on php-fpm or a Laravel Octane server")
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

/// A per-site scratchpad: what the site is about, its production and testing
/// URLs, and a to-do checklist. The notes are stored inside the project folder
/// (`<project>/.dpl/project.json`) so they travel with the repo — loaded from
/// and saved to the Store's `SiteProject` on every edit.
struct SiteProjectSection: View {
    @EnvironmentObject var store: Store
    /// The site's project directory — where the notes file lives.
    let projectPath: String

    @State private var project = SiteProject()
    @State private var newTodo = ""
    /// Guards the save-on-change from firing during the initial load.
    @State private var loaded = false

    var body: some View {
        DetailSection(title: "Project") {
            VStack(alignment: .leading, spacing: 14) {
                summaryField
                HStack(alignment: .top, spacing: 12) {
                    urlField(title: "Production", text: $project.productionURL,
                             placeholder: "https://example.com")
                    urlField(title: "Testing", text: $project.testingURL,
                             placeholder: "https://staging.example.com")
                }
                relatedSites
                relatedPackages
                todoList
            }
        }
        // Reload when navigating between sites (the view is reused).
        .task(id: projectPath) {
            loaded = false
            project = store.project(atPath: projectPath)
            loaded = true
        }
        .onChange(of: project) { _, updated in
            guard loaded else { return }
            store.setProject(updated, atPath: projectPath)
        }
    }

    // MARK: About

    private var summaryField: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("About").font(.caption).foregroundStyle(.secondary)
            TextEditor(text: $project.summary)
                .font(.callout)
                .scrollContentBackground(.hidden)
                .frame(minHeight: 52)
                .padding(6)
                .background(RoundedRectangle(cornerRadius: 6).fill(Color.primary.opacity(0.04)))
                .overlay(alignment: .topLeading) {
                    if project.summary.isEmpty {
                        Text("What is this site about?")
                            .font(.callout).foregroundStyle(.tertiary)
                            .padding(.horizontal, 11).padding(.vertical, 14)
                            .allowsHitTesting(false)
                    }
                }
        }
    }

    // MARK: URLs

    private func urlField(title: String, text: Binding<String>, placeholder: String) -> some View {
        let trimmed = text.wrappedValue.trimmingCharacters(in: .whitespaces)
        return VStack(alignment: .leading, spacing: 4) {
            Text(title).font(.caption).foregroundStyle(.secondary)
            HStack(spacing: 6) {
                TextField(placeholder, text: text)
                    .textFieldStyle(.roundedBorder)
                    .font(.callout)
                Button {
                    store.openURL(normalizedURL(trimmed))
                } label: {
                    Image(systemName: "arrow.up.forward.app")
                }
                .buttonStyle(.borderless)
                .disabled(trimmed.isEmpty)
                .help("Open \(title.lowercased()) URL")
            }
        }
    }

    /// Let the user type `example.com` and still get a working link.
    private func normalizedURL(_ raw: String) -> String {
        guard !raw.isEmpty else { return raw }
        if raw.contains("://") { return raw }
        return "https://" + raw
    }

    // MARK: Related sites

    /// Other local sites that belong to the same product — the API next to the
    /// app, the admin next to the storefront. Chips jump to that site's detail.
    private var relatedSites: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Text("Related sites").font(.caption).foregroundStyle(.secondary)
                addSiteMenu
                Spacer()
            }
            if !project.relatedSites.isEmpty {
                // Wrapping chips: a simple flow via LazyVGrid-free HStacks is
                // overkill at the handful-of-sites scale; one wrapping HStack.
                HStack(spacing: 6) {
                    ForEach(project.relatedSites, id: \.self) { name in
                        HStack(spacing: 4) {
                            Button(name) { store.siteJump = name }
                                .buttonStyle(.plain)
                                .font(.caption.weight(.medium))
                                .foregroundStyle(Theme.violet)
                            Button {
                                project.relatedSites.removeAll { $0 == name }
                            } label: { Image(systemName: "xmark").font(.system(size: 8)) }
                            .buttonStyle(.plain).foregroundStyle(.tertiary)
                        }
                        .padding(.horizontal, 8).padding(.vertical, 3)
                        .background(Theme.violet.opacity(0.12), in: Capsule())
                    }
                    Spacer()
                }
            }
        }
    }

    /// "+" menu of every other local site not already referenced.
    private var addSiteMenu: some View {
        Menu {
            ForEach(candidateRelatedSites, id: \.self) { name in
                Button(name) { project.relatedSites.append(name) }
            }
        } label: {
            Image(systemName: "plus.circle").font(.caption)
        }
        .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
        .disabled(candidateRelatedSites.isEmpty)
        .help("Reference another local site that's part of this project")
    }

    private var candidateRelatedSites: [String] {
        store.localSites
            // Exclude this site (matched by project path — the section doesn't
            // know its own site name) and anything already referenced.
            .filter { $0.cell(["path"]) != projectPath }
            .map { $0.cell(["name"]) }
            .filter { !$0.isEmpty && !project.relatedSites.contains($0) }
    }

    // MARK: Related packages

    /// Non-site code that belongs to this project — SDKs, composer packages,
    /// shared libraries. Rows open in the configured editor or in Finder.
    private var relatedPackages: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Text("Related packages").font(.caption).foregroundStyle(.secondary)
                Button { addPackage() } label: { Image(systemName: "plus.circle").font(.caption) }
                    .buttonStyle(.borderless)
                    .help("Reference a folder of code that's part of this project")
                Spacer()
            }
            ForEach(project.relatedPackages) { pkg in
                HStack(spacing: 8) {
                    Image(systemName: "shippingbox").font(.caption).foregroundStyle(.secondary)
                    Text(pkg.name).font(.callout.weight(.medium))
                    Text(pkg.path)
                        .font(.system(.caption, design: .monospaced)).foregroundStyle(.tertiary)
                        .lineLimit(1).truncationMode(.middle)
                    Spacer()
                    Button { store.openFolderInEditor(pkg.path) } label: {
                        Image(systemName: "chevron.left.forwardslash.chevron.right").font(.caption2)
                    }
                    .buttonStyle(.borderless).help("Open in editor")
                    Button {
                        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: pkg.path)])
                    } label: { Image(systemName: "folder").font(.caption2) }
                    .buttonStyle(.borderless).help("Reveal in Finder")
                    Button {
                        project.relatedPackages.removeAll { $0.id == pkg.id }
                    } label: { Image(systemName: "xmark").font(.caption2) }
                    .buttonStyle(.borderless).foregroundStyle(.tertiary).help("Remove")
                }
            }
        }
    }

    private func addPackage() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = true
        panel.prompt = "Add"
        panel.message = "Choose folders of code that are part of this project"
        guard panel.runModal() == .OK else { return }
        for url in panel.urls {
            let path = url.path
            guard !project.relatedPackages.contains(where: { $0.path == path }) else { continue }
            project.relatedPackages.append(
                SiteProject.RelatedPackage(name: url.lastPathComponent, path: path))
        }
    }

    // MARK: To-dos

    private var todoList: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Text("To do").font(.caption).foregroundStyle(.secondary)
                if project.openTodos > 0 {
                    Text("\(project.openTodos)")
                        .font(.caption2.weight(.bold))
                        .foregroundStyle(.white)
                        .padding(.horizontal, 6).padding(.vertical, 1)
                        .background(Capsule().fill(Theme.violet))
                }
            }

            ForEach($project.todos) { $todo in
                HStack(spacing: 8) {
                    Button {
                        todo.done.toggle()
                    } label: {
                        Image(systemName: todo.done ? "checkmark.circle.fill" : "circle")
                            .foregroundStyle(todo.done ? Theme.live : .secondary)
                    }
                    .buttonStyle(.borderless)

                    TextField("", text: $todo.text)
                        .textFieldStyle(.plain)
                        .font(.callout)
                        .foregroundStyle(todo.done ? .secondary : .primary)
                        .strikethrough(todo.done, color: .secondary)

                    Button {
                        project.todos.removeAll { $0.id == todo.id }
                    } label: {
                        Image(systemName: "xmark").font(.caption2)
                    }
                    .buttonStyle(.borderless)
                    .foregroundStyle(.tertiary)
                    .help("Remove")
                }
            }

            HStack(spacing: 8) {
                Image(systemName: "plus.circle").foregroundStyle(.secondary)
                TextField("Add a task…", text: $newTodo)
                    .textFieldStyle(.plain)
                    .font(.callout)
                    .onSubmit(addTodo)
            }
        }
    }

    private func addTodo() {
        let text = newTodo.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        project.todos.append(SiteProject.Todo(text: text))
        newTodo = ""
    }
}
