import SwiftUI

/// The **Extensions** page: OPcache tuning, Xdebug, and per-extension toggles
/// for one installed PHP version — picked in the header rather than from a
/// duplicate sidebar list.
struct ExtensionsPage: View {
    @EnvironmentObject var store: Store

    @State private var version = ""
    @State private var exts: [Store.PhpExt] = []
    @State private var available: [String] = []
    @State private var opcache = OpcacheConfig()
    @State private var draft = OpcacheConfig()
    @State private var xdebug = XdebugConfig()
    @State private var search = ""
    @State private var loading = true
    @State private var loadingAvailable = false
    @State private var busy = false
    @State private var showAdvanced = false
    @State private var showAvailable = false
    @State private var installExt: PhpAction?

    /// The version being edited: the picked one, else the default, else the first.
    private var install: PhpInstall? {
        store.installedPhp.first { $0.version == version } ?? store.installedPhp.first
    }

    private var filtered: [Store.PhpExt] {
        search.isEmpty ? exts : exts.filter { $0.name.localizedCaseInsensitiveContains(search) }
    }
    private var filteredAvailable: [String] {
        search.isEmpty ? available : available.filter { $0.localizedCaseInsensitiveContains(search) }
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                header
                if store.installedPhp.isEmpty {
                    ContentUnavailableView("No PHP versions", systemImage: "puzzlepiece.extension",
                        description: Text("Install one from the PHP page."))
                } else if loading {
                    HStack(spacing: 8) {
                        ProgressView().controlSize(.small)
                        Text("Reading PHP \(install?.version ?? "")…").foregroundStyle(.secondary)
                    }
                    .padding(.top, 30).frame(maxWidth: .infinity)
                } else if let install {
                    opcachePanel(install)
                    xdebugPanel(install)
                    extensionsPanel(install)
                    availablePanel(install)
                }
            }
            .padding(18)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .defaultScrollAnchor(.top)
        .task {
            if store.phpVersions.isEmpty { await store.loadPhpVersions() }
            if version.isEmpty {
                // A site's "Manage Extensions (PHP 8.3)…" names the version it
                // wants; otherwise open on the default.
                version = store.extensionsVersion ?? store.defaultPhp ?? store.installedPhp.first?.version ?? ""
                store.extensionsVersion = nil
            }
        }
        .onChange(of: store.extensionsVersion) { _, requested in
            if let requested, !requested.isEmpty { version = requested; store.extensionsVersion = nil }
        }
        .task(id: install?.version) { await reload(); await loadAvailable() }
        .sheet(item: $installExt, onDismiss: { Task { await reload(); await loadAvailable() } }) { action in
            PhpManagerSheet(action: action).environmentObject(store)
        }
        .overlay(alignment: .top) {
            if busy {
                HStack(spacing: 6) { ProgressView().controlSize(.small); Text("Applying & restarting php-fpm…").font(.caption) }
                    .padding(8).background(.regularMaterial, in: Capsule()).padding(.top, 6)
            }
        }
    }

    // MARK: Header

    private var header: some View {
        HStack(spacing: 12) {
            GradientTile(systemImage: "puzzlepiece.extension", size: 40)
            VStack(alignment: .leading, spacing: 2) {
                Text("Extensions").font(.title2.weight(.bold))
                Text(install.map { "OPcache, Xdebug, and module toggles for \($0.binary)" }
                     ?? "OPcache tuning and per-extension toggles.")
                    .font(.caption).foregroundStyle(.secondary)
                    .lineLimit(1).truncationMode(.middle)
            }
            Spacer()
            // Which version you're editing is the page's key choice — a picker,
            // not a whole column of near-identical rows.
            if !store.installedPhp.isEmpty {
                Picker("", selection: $version) {
                    ForEach(store.installedPhp) { php in
                        Text("PHP \(php.version)").tag(php.version)
                    }
                }
                .labelsHidden().fixedSize()
                .disabled(busy)
            }
            Button { Task { await reload() } } label: { Image(systemName: "arrow.clockwise") }.disabled(busy)
        }
    }

    // MARK: OPcache

    @ViewBuilder
    private func opcachePanel(_ install: PhpInstall) -> some View {
        DetailSection(title: "OPcache") {
            VStack(alignment: .leading, spacing: 12) {
                if !opcache.loaded {
                    Text("The Zend OPcache extension isn't loaded — enable it in the list below first.")
                        .font(.caption).foregroundStyle(.orange)
                }

                // The three presets are the whole decision for most people, so
                // they read as one control instead of three buttons + two pills.
                Picker("", selection: Binding(get: { opcache.mode }, set: { apply(OpcacheConfig.preset($0)) })) {
                    Text("Development").tag("dev")
                    Text("Production").tag("prod")
                    Text("Off").tag("off")
                }
                .pickerStyle(.segmented).labelsHidden().disabled(busy)

                Text(modeBlurb)
                    .font(.caption).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)

                DisclosureGroup("Advanced tuning", isExpanded: $showAdvanced) {
                    VStack(alignment: .leading, spacing: 10) {
                        Toggle("OPcache enabled", isOn: $draft.enabled)
                        Toggle("Revalidate timestamps (pick up code changes)", isOn: $draft.validateTimestamps)
                            .disabled(!draft.enabled)
                        Stepper("Memory: \(draft.memoryMB) MB", value: $draft.memoryMB, in: 32...1024, step: 32)
                            .disabled(!draft.enabled)
                        Stepper("Max cached files: \(draft.maxFiles)", value: $draft.maxFiles, in: 1000...100000, step: 1000)
                            .disabled(!draft.enabled)
                        Toggle("JIT", isOn: $draft.jit).disabled(!draft.enabled)
                        if draft.jit {
                            Stepper("JIT buffer: \(draft.jitBufferMB) MB", value: $draft.jitBufferMB, in: 0...512, step: 32)
                                .disabled(!draft.enabled)
                        }
                        HStack {
                            if draft != opcache { Text("Unsaved changes").font(.caption).foregroundStyle(.orange) }
                            Spacer()
                            Button("Revert") { draft = opcache }.disabled(draft == opcache || busy)
                            Button("Apply") { apply(draft) }
                                .buttonStyle(.borderedProminent).tint(Theme.violet)
                                .disabled(draft == opcache || busy)
                        }
                    }
                    .padding(.top, 8)
                }
                .font(.callout)
            }
            .font(.callout)
        }
    }

    private var modeBlurb: String {
        switch opcache.mode {
        case "dev": return "Cached, but revalidated every request — your code changes show up immediately."
        case "prod": return "Trusts the cache: fastest, but you must restart php-fpm to see code changes."
        default: return "Every request recompiles from source. Slower, and nothing to invalidate."
        }
    }

    // MARK: Xdebug

    @ViewBuilder
    private func xdebugPanel(_ install: PhpInstall) -> some View {
        DetailSection(title: "Xdebug") {
            if !xdebug.loaded {
                VStack(alignment: .leading, spacing: 8) {
                    if xdebug.disabledButInstalled {
                        Text("Installed for PHP \(install.version) but not enabled (its load line is commented out).")
                            .font(.callout).foregroundStyle(.secondary)
                        Button { apply(mode: "debug", ideKey: xdebug.ideKey, port: xdebug.clientPort, install: install) } label: {
                            Label("Enable Xdebug", systemImage: "power")
                        }
                        .buttonStyle(.borderedProminent).tint(Theme.violet).disabled(busy)
                    } else {
                        Text("Not installed for PHP \(install.version).")
                            .font(.callout).foregroundStyle(.secondary)
                        Button { installExt = .extInstall(install.version, "xdebug") } label: {
                            Label("Install Xdebug", systemImage: "arrow.down.circle")
                        }
                        .buttonStyle(.borderedProminent).tint(Theme.violet).disabled(busy)
                    }
                }
            } else {
                VStack(alignment: .leading, spacing: 12) {
                    Picker("", selection: Binding(get: { xdebugMode }, set: { apply(mode: $0, ideKey: xdebug.ideKey, port: xdebug.clientPort, install: install) })) {
                        Text("Off").tag("off")
                        Text("Step Debug").tag("debug")
                        Text("Develop").tag("develop")
                    }
                    .pickerStyle(.segmented).labelsHidden().disabled(busy)

                    if xdebug.stepDebug {
                        Text("Set your IDE to listen on **127.0.0.1:\(xdebug.clientPort)** (IDE key `\(xdebug.ideKey)`), set a breakpoint, and reload the site.")
                            .font(.caption).foregroundStyle(.secondary)
                    } else if xdebug.mode == "off" {
                        Text("Off — no overhead. Switch to Step Debug when you need breakpoints.")
                            .font(.caption).foregroundStyle(.secondary)
                    } else {
                        Text("Develop mode: better error messages and var_dump output, no step debugging.")
                            .font(.caption).foregroundStyle(.secondary)
                    }

                    HStack(spacing: 16) {
                        Picker("IDE", selection: Binding(
                            get: { xdebug.ideKey },
                            set: { apply(mode: xdebug.mode == "off" ? "debug" : xdebug.mode, ideKey: $0, port: xdebug.clientPort, install: install) })) {
                            Text("PhpStorm").tag("PHPSTORM")
                            Text("VS Code").tag("VSCODE")
                        }
                        .fixedSize().disabled(busy)
                        Text("Port \(xdebug.clientPort)").font(.caption).foregroundStyle(.secondary)
                        Spacer()
                    }
                    .font(.callout)
                }
            }
        }
    }

    /// The segmented control's selection, derived from the loaded config.
    private var xdebugMode: String {
        if xdebug.stepDebug { return "debug" }
        if xdebug.develop { return "develop" }
        return "off"
    }

    // MARK: Installed extensions

    @ViewBuilder
    private func extensionsPanel(_ install: PhpInstall) -> some View {
        DetailSection(title: "Loaded · \(exts.count)") {
            VStack(spacing: 0) {
                HStack(spacing: 8) {
                    Image(systemName: "magnifyingglass").font(.caption).foregroundStyle(.tertiary)
                    TextField("Filter extensions…", text: $search).textFieldStyle(.plain)
                }
                .padding(.bottom, 6)

                if filtered.isEmpty {
                    emptyRow(exts.isEmpty ? "No configurable extensions found." : "No match.")
                }
                ForEach(Array(filtered.enumerated()), id: \.element.id) { i, ext in
                    if i > 0 { Divider().opacity(0.5) }
                    HStack(spacing: 10) {
                        // The toggle already says on/off — a status dot beside it
                        // would say it twice.
                        Text(ext.name)
                            .font(.callout.weight(.medium))
                            .foregroundStyle(ext.enabled ? .primary : .secondary)
                        Spacer()
                        // A switch reads as on/off; a checkbox reads as "selected".
                        Toggle("", isOn: Binding(get: { ext.enabled }, set: { on in toggle(ext, on) }))
                            .toggleStyle(.switch)
                            .labelsHidden().controlSize(.mini).disabled(busy)
                        Menu {
                            Button(ext.enabled ? "Disable" : "Enable") { toggle(ext, !ext.enabled) }
                            Divider()
                            Button("Uninstall…", role: .destructive) { uninstall(ext, install: install) }
                        } label: { Image(systemName: "ellipsis") }
                        .menuStyle(.borderlessButton).frame(width: 20).disabled(busy)
                    }
                    .padding(.vertical, 7)
                }
            }
        }
    }

    // MARK: Available

    /// Folded away by default — the catalog is long and rarely the reason
    /// someone opened this page.
    @ViewBuilder
    private func availablePanel(_ install: PhpInstall) -> some View {
        DetailSection(title: "Available") {
            VStack(alignment: .leading, spacing: 0) {
                if loadingAvailable {
                    HStack(spacing: 6) {
                        ProgressView().controlSize(.small)
                        Text("Searching the Homebrew extension catalog…").font(.caption).foregroundStyle(.secondary)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading).padding(.vertical, 8)
                } else if available.isEmpty {
                    emptyRow("Everything in the catalog is already installed (or the tap isn't available).")
                } else {
                    Button {
                        withAnimation(.easeInOut(duration: 0.15)) { showAvailable.toggle() }
                    } label: {
                        HStack(spacing: 6) {
                            Image(systemName: "chevron.right")
                                .font(.caption2.weight(.semibold))
                                .rotationEffect(.degrees(showAvailable ? 90 : 0))
                            Text("\(available.count) more extension\(available.count == 1 ? "" : "s") to install")
                                .font(.callout)
                            Spacer()
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)

                    if showAvailable {
                        if filteredAvailable.isEmpty {
                            emptyRow("No match for “\(search)”.")
                        }
                        ForEach(Array(filteredAvailable.enumerated()), id: \.element) { i, name in
                            if i > 0 { Divider().opacity(0.5) }
                            HStack(spacing: 10) {
                                Text(name).font(.callout).foregroundStyle(.secondary)
                                Spacer()
                                Button("Install") { installExt = .extInstall(install.version, name) }
                                    .buttonStyle(.bordered).controlSize(.small).disabled(busy)
                            }
                            .padding(.vertical, 6)
                        }
                        .padding(.top, 4)
                    }
                }
            }
        }
    }

    private func emptyRow(_ text: String) -> some View {
        Text(text).font(.caption).foregroundStyle(.secondary)
            .frame(maxWidth: .infinity, alignment: .leading).padding(.vertical, 8)
    }

    // MARK: Actions

    private func reload() async {
        guard let install else { loading = false; return }
        loading = exts.isEmpty
        exts = await store.extensions(forBinary: install.binary)
        opcache = await store.opcacheConfig(forBinary: install.binary)
        draft = opcache
        xdebug = await store.xdebugConfig(forBinary: install.binary)
        loading = false
    }

    private func loadAvailable() async {
        guard let install else { return }
        loadingAvailable = true
        available = await store.availableExtensions(forVersion: install.version)
        loadingAvailable = false
    }

    private func apply(_ cfg: OpcacheConfig) {
        guard let install else { return }
        busy = true
        Task {
            await store.applyOpcache(cfg, binary: install.binary)
            await reload()
            busy = false
        }
    }

    private func apply(mode: String, ideKey: String, port: Int, install: PhpInstall) {
        busy = true
        Task {
            await store.applyXdebug(mode: mode, ideKey: ideKey, port: port, binary: install.binary)
            await reload()
            busy = false
        }
    }

    private func toggle(_ ext: Store.PhpExt, _ on: Bool) {
        busy = true
        Task {
            await store.setExtension(ext, enabled: on)
            await reload()
            busy = false
        }
    }

    private func uninstall(_ ext: Store.PhpExt, install: PhpInstall) {
        busy = true
        Task {
            await store.uninstallExtension(ext.name, version: install.version)
            await reload()
            await loadAvailable()
            busy = false
        }
    }
}
