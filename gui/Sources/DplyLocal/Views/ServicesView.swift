import SwiftUI

/// Middle column for Services: managed instances (start/stop/delete) plus any
/// externally-running engines (DBngin/Postgres.app), with a ➕ to create a new
/// multi-version instance.
struct ServicesListView: View {
    @EnvironmentObject var store: Store
    @Binding var selection: String?
    @State private var showCreate = false
    @State private var showInstall = false

    private var instances: [Row] { store.dbServices.filter { $0.dig("external") != .bool(true) } }
    private var externals: [Row] { store.dbServices.filter { $0.dig("external") == .bool(true) } }

    var body: some View {
        List(selection: $selection) {
            Section("Instances") {
                if instances.isEmpty {
                    Text("No instances yet — ➕ to create one.")
                        .font(.callout).foregroundStyle(.secondary)
                }
                ForEach(instances) { row in
                    instanceRow(row).tag(row.cell(["name"]))
                }
            }
            if !externals.isEmpty {
                Section("Detected (DBngin / external)") {
                    ForEach(externals) { row in
                        instanceRow(row).tag(row.cell(["name"]))
                    }
                }
            }
        }
        .overlay { if store.isLoading { ProgressView().controlSize(.small) } }
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Menu {
                    Button("New instance…") { showCreate = true }
                    Button("Install an engine…") { showInstall = true }
                } label: { Image(systemName: "plus") }
                .help("Create an instance or install an engine")
            }
        }
        .sheet(isPresented: $showCreate) { CreateInstanceSheet().environmentObject(store) }
        .sheet(isPresented: $showInstall) { InstallEngineSheet().environmentObject(store) }
    }

    @ViewBuilder
    private func instanceRow(_ row: Row) -> some View {
        let name = row.cell(["name"])
        let engine = row.cell(["engine"])
        let running = row.dig("running") == .bool(true)
        let external = row.dig("external") == .bool(true)
        let version = row.cell(["version"])
        HStack(spacing: 11) {
            GradientTile(systemImage: icon(engine), size: 30, active: running)
            VStack(alignment: .leading, spacing: 2) {
                Text(name).font(.body.weight(.semibold))
                Text("\(engine)\(version.isEmpty ? "" : " \(version)") · port \(row.cell(["port"]))\(external ? " · DBngin" : "")")
                    .font(.system(.caption, design: .monospaced))
                    .foregroundStyle(.secondary).lineLimit(1)
            }
            Spacer()
            if external {
                StatusBadge(status: "running")
            } else {
                Button(running ? "Stop" : "Start") {
                    Task { await store.serviceAction(running ? "stop" : "start", name: name) }
                }
                .buttonStyle(.bordered).controlSize(.small)
                .tint(running ? .red : Theme.violet)
            }
        }
        .padding(.vertical, 3)
        .contextMenu {
            if !external {
                Button("Restart") { Task { await store.serviceAction("restart", name: name) } }
                Button("Delete", role: .destructive) { Task { await store.deleteInstance(name: name) } }
            }
        }
    }

    private func icon(_ engine: String) -> String {
        switch engine {
        case "redis": return "bolt.fill"
        case "mysql": return "cylinder.split.1x2.fill"
        case "postgres": return "cylinder.fill"
        default: return "cylinder"
        }
    }
}

/// Sheet to create a new managed instance: engine + version + name + port.
struct CreateInstanceSheet: View {
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss

    @State private var engine = "postgres"
    @State private var version = ""
    @State private var name = ""
    @State private var port = ""

    private let engines = ["postgres", "mysql", "redis"]

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("New database instance").font(.headline)

            Form {
                TextField("Name", text: $name, prompt: Text("e.g. app-pg17"))
                Picker("Engine", selection: $engine) {
                    ForEach(engines, id: \.self) { Text($0.capitalized).tag($0) }
                }
                .onChange(of: engine) { version = store.versions(engine: engine).first ?? "" }
                Picker("Version", selection: $version) {
                    Text("newest").tag("")
                    ForEach(store.versions(engine: engine), id: \.self) { Text($0).tag($0) }
                }
                TextField("Port", text: $port, prompt: Text("auto (first free)"))
            }
            .formStyle(.columns)

            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                Button("Create") {
                    Task {
                        await store.createInstance(name: name, engine: engine,
                            version: version.isEmpty ? nil : version,
                            port: port.isEmpty ? nil : port)
                        dismiss()
                    }
                }
                .buttonStyle(.borderedProminent).tint(Theme.violet)
                .disabled(name.isEmpty)
            }
        }
        .padding(18)
        .frame(width: 380)
        .task { await store.loadVersions() }
    }
}

/// Detail pane: list + create/drop databases on a running engine/instance.
struct DatabasesView: View {
    @EnvironmentObject var store: Store
    /// The selected service row (carries engine + port + running state).
    let service: Row

    @State private var databases: [String] = []
    @State private var newName = ""
    @State private var loading = true

    private var engine: String { service.cell(["engine"]) }
    private var port: Int? { Int(service.cell(["port"])) }
    private var isRunning: Bool {
        service.dig("running") == .bool(true) || service.dig("external") == .bool(true)
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                HStack(spacing: 12) {
                    GradientTile(systemImage: "cylinder.fill", size: 40, active: isRunning)
                    VStack(alignment: .leading, spacing: 2) {
                        Text("\(service.cell(["name"])) databases").font(.title2.weight(.bold))
                        Text("\(engine) · port \(service.cell(["port"]))")
                            .font(.system(.caption, design: .monospaced)).foregroundStyle(.secondary)
                    }
                    Spacer()
                    StatusBadge(status: isRunning ? "running" : "stopped")
                }

                if isRunning, let p = port {
                    let conn = store.connectionStrings(engine: engine, port: p)
                    DetailSection(title: "Connection") {
                        VStack(alignment: .leading, spacing: 8) {
                            copyRow(label: "URL", value: conn.url)
                            if !conn.cmd.isEmpty { copyRow(label: "Connect", value: conn.cmd) }
                            copyRow(label: "Host", value: "127.0.0.1")
                            copyRow(label: "Port", value: String(p))
                            if engine != "redis" {
                                copyRow(label: "User", value: engine == "postgres" ? "postgres" : "root")
                            }
                        }
                    }
                }

                if engine == "redis" {
                    Text("Redis has no SQL databases — connect with the command above.")
                        .foregroundStyle(.secondary)
                } else if !isRunning {
                    ContentUnavailableView {
                        Label("Not running", systemImage: "cylinder")
                    } description: {
                        Text("Start this instance to manage databases.")
                    } actions: {
                        Button("Start") {
                            Task { await store.serviceAction("start", name: service.cell(["name"])); await load() }
                        }
                        .buttonStyle(.borderedProminent).tint(Theme.violet)
                    }
                } else {
                    DetailSection(title: "Create database") {
                        HStack {
                            TextField("name", text: $newName)
                                .textFieldStyle(.roundedBorder)
                                .onSubmit { create() }
                            Button("Create") { create() }
                                .buttonStyle(.borderedProminent).tint(Theme.violet)
                                .disabled(newName.isEmpty)
                            Spacer()
                            Button { restoreFromFile() } label: {
                                Label("Restore…", systemImage: "square.and.arrow.down")
                            }
                            .help("Restore a .sql dump into a database")
                        }
                    }
                    DetailSection(title: "Databases") {
                        if loading {
                            ProgressView().controlSize(.small)
                        } else if databases.isEmpty {
                            Text("No databases yet.").foregroundStyle(.secondary)
                        } else {
                            ForEach(databases, id: \.self) { db in
                                HStack {
                                    Image(systemName: "cylinder").foregroundStyle(.secondary)
                                    Text(db).font(.callout)
                                    Spacer()
                                    Button { backup(db) } label: { Image(systemName: "square.and.arrow.up") }
                                        .buttonStyle(.borderless).help("Back up to a .sql file")
                                    Button(role: .destructive) { drop(db) } label: {
                                        Image(systemName: "trash")
                                    }
                                    .buttonStyle(.borderless)
                                }
                                .padding(.vertical, 2)
                            }
                        }
                    }
                }
            }
            .padding(18)
        }
        .task(id: service.cell(["name"])) { await load() }
    }

    private func load() async {
        guard isRunning, engine != "redis" else { loading = false; return }
        loading = true
        databases = await store.dbList(engine: engine, port: port) ?? []
        loading = false
    }

    private func create() {
        let name = newName
        guard !name.isEmpty else { return }
        newName = ""
        Task { await store.dbAction("create", engine: engine, name: name, port: port); await load() }
    }

    private func drop(_ db: String) {
        Task { await store.dbAction("drop", engine: engine, name: db, port: port); await load() }
    }

    /// A label + selectable value with a copy button.
    private func copyRow(label: String, value: String) -> some View {
        HStack(spacing: 10) {
            Text(label).font(.callout).foregroundStyle(.secondary).frame(width: 70, alignment: .leading)
            Text(value).font(.system(.callout, design: .monospaced)).textSelection(.enabled)
                .lineLimit(1).truncationMode(.middle)
            Spacer(minLength: 4)
            Button { store.copyToPasteboard(value) } label: { Image(systemName: "doc.on.doc") }
                .buttonStyle(.borderless).help("Copy")
        }
    }

    /// Back up a database to a file chosen via a save panel.
    private func backup(_ db: String) {
        let panel = NSSavePanel()
        panel.nameFieldStringValue = "\(db).sql"
        panel.allowedContentTypes = []
        if panel.runModal() == .OK, let url = panel.url {
            Task { await store.dbBackup(engine: engine, name: db, port: port, file: url.path); await load() }
        }
    }

    /// Restore a .sql dump into a database named after the file.
    private func restoreFromFile() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false
        if panel.runModal() == .OK, let url = panel.url {
            let db = url.deletingPathExtension().lastPathComponent
                .replacingOccurrences(of: "-", with: "_")
            Task { await store.dbRestore(engine: engine, name: db, port: port, file: url.path); await load() }
        }
    }
}
