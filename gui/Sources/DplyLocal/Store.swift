import Foundation
import SwiftUI

/// Sidebar sections. Local `.test` sites will join this list once the daemon
/// phases land; for now the app manages the dply surfaces the CLI already
/// exposes.
enum Surface: String, CaseIterable, Identifiable {
    case local = "Local Sites"
    case services = "Services"
    case mail = "Mail"
    case dumps = "Dumps"
    case php = "PHP"
    case edgeSites = "Edge Sites"
    case sites = "Server Sites"
    case servers = "Servers"
    case account = "Account"

    var id: String { rawValue }

    /// The local-tooling surfaces (no dply login required).
    var isLocal: Bool {
        switch self {
        case .local, .services, .mail, .dumps, .php: return true
        default: return false
        }
    }
    var isDply: Bool { !isLocal }

    var systemImage: String {
        switch self {
        case .local: return "house"
        case .services: return "cylinder.split.1x2"
        case .mail: return "envelope"
        case .dumps: return "ladybug"
        case .php: return "chevron.left.forwardslash.chevron.right"
        case .edgeSites: return "bolt.horizontal.circle"
        case .sites: return "server.rack"
        case .servers: return "externaldrive.connected.to.line.below"
        case .account: return "person.crop.circle"
        }
    }
}

/// Observable application state. Every mutating action shells out to `dpl` and
/// refreshes the affected list. All published mutations happen on the main
/// actor so SwiftUI stays consistent.
@MainActor
final class Store: ObservableObject {
    // Persisted settings.
    @AppStorage("dplyHost") var host: String = ""
    @AppStorage("dplyBinaryPath") var binaryPath: String = ""

    // Navigation. Local sites lead — that's the primary use; dply is secondary.
    @Published var section: Surface = .local

    // Data.
    @Published var localSites: [Row] = []
    @Published var dbServices: [Row] = []
    @Published var mailMessages: [Row] = []
    @Published var phpVersions: [Row] = []
    @Published var tlds: [String] = []
    @Published var edgeSites: [Row] = []
    @Published var sites: [Row] = []
    @Published var servers: [Row] = []
    @Published var account: Row?

    // Status.
    @Published var isLoading = false
    @Published var lastError: String?

    /// A CLI configured with the current settings.
    var cli: DplyCLI {
        DplyCLI(
            overridePath: binaryPath.isEmpty ? nil : binaryPath,
            host: host.isEmpty ? nil : host
        )
    }

    var isAuthenticated: Bool {
        account?.dig("authenticated") == .bool(true)
    }

    var activeHost: String {
        account?.first(["host"]) ?? (host.isEmpty ? "default" : host)
    }

    // MARK: Loading

    func refreshAll() async {
        // Only touch dply (which needs a login) when a dply surface is active.
        if section.isDply { await refreshAccount() }
        // PHP versions + TLDs are cheap and used across panes (site picker,
        // settings), so keep them warm.
        await loadPhpVersions()
        await loadTlds()
        // Keep the dumps stream live from launch so dumps accumulate even
        // before the panel is opened.
        startDumpsStream()
        await refreshCurrentSection()
    }

    func loadPhpVersions() async {
        let cli = self.cli
        if let r = await background({ try cli.rows(["php"]) }) { phpVersions = r }
    }

    func refreshCurrentSection() async {
        let cli = self.cli
        isLoading = true
        defer { isLoading = false }
        switch section {
        case .local:
            if let r = await background({ try cli.rows(["sites"]) }) { localSites = r }
        case .services:
            if let r = await background({ try cli.rows(["services"]) }) { dbServices = r }
            await loadVersions()
        case .mail:
            if let r = await background({ try cli.rows(["mail", "list"]) }) { mailMessages = r }
        case .dumps:
            break // dumps arrive via the live stream, not a refresh
        case .php:
            if let r = await background({ try cli.rows(["php"]) }) { phpVersions = r }
        case .edgeSites:
            if let r = await background({ try cli.rows(["dply", "edge:sites"]) }) { edgeSites = r }
        case .sites:
            if let r = await background({ try cli.rows(["dply", "sites:list"]) }) { sites = r }
        case .servers:
            if let r = await background({ try cli.rows(["dply", "servers:list"]) }) { servers = r }
        case .account:
            await refreshAccount()
        }
    }

    // MARK: Local site actions

    /// Link a project directory as a `.test` site, then refresh.
    func linkLocal(path: String) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["link", path]) }
        await loadLocal()
    }

    /// Park a directory (its subfolders each become sites), then refresh.
    func parkLocal(path: String) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["park", path]) }
        await loadLocal()
    }

    func unlinkLocal(name: String) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["unlink", name]) }
        await loadLocal()
    }

    func setLocalSecure(name: String, secure: Bool) async {
        let cli = self.cli
        _ = await background { try cli.runRaw([secure ? "secure" : "unsecure", name]) }
        await loadLocal()
    }

    func loadLocal() async {
        let cli = self.cli
        if let r = await background({ try cli.rows(["sites"]) }) { localSites = r }
    }

    // MARK: Services / PHP / Mail / TLD actions

    // Installed engine versions for the instance picker.
    @Published var availableVersions: [Row] = []

    func serviceAction(_ action: String, name: String) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["service", action, name]) }
        await loadServices()
    }

    func loadServices() async {
        let cli = self.cli
        if let r = await background({ try cli.rows(["services"]) }) { dbServices = r }
    }

    func loadVersions() async {
        let cli = self.cli
        if let r = await background({ try cli.rows(["service", "versions"]) }) { availableVersions = r }
    }

    /// Versions available for a given engine (for the create form).
    func versions(engine: String) -> [String] {
        availableVersions.filter { $0.first(["engine"]) == engine }
            .compactMap { $0.first(["version"]) }
    }

    /// Create a managed instance.
    func createInstance(name: String, engine: String, version: String?, port: String?) async {
        let cli = self.cli
        var args = ["service", "create", name, "--engine", engine]
        if let v = version, !v.isEmpty { args += ["--version", v] }
        if let p = port, !p.isEmpty { args += ["--port", p] }
        _ = await background { try cli.runRaw(args) }
        await loadServices()
    }

    func deleteInstance(name: String) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["service", "delete", name]) }
        await loadServices()
    }

    /// db operations targeting a specific port (an instance) or the default.
    func dbList(engine: String, port: Int?) async -> [String]? {
        let cli = self.cli
        var args = ["db", "list", "--engine", engine]
        if let p = port { args += ["--port", String(p)] }
        guard let out = await backgroundQuiet({
            String(decoding: try cli.runRaw(args), as: UTF8.self)
        }) else { return nil }
        return out.split(separator: "\n").map(String.init).filter { !$0.isEmpty && $0 != "(none)" && $0 != "(no databases)" }
    }

    func dbAction(_ action: String, engine: String, name: String, port: Int?) async {
        let cli = self.cli
        var args = ["db", action, name, "--engine", engine]
        if let p = port { args += ["--port", String(p)] }
        _ = await background { try cli.runRaw(args) }
    }

    /// Back up a database to `file`.
    func dbBackup(engine: String, name: String, port: Int?, file: String) async {
        let cli = self.cli
        var args = ["db", "backup", name, "--engine", engine, "--file", file]
        if let p = port { args += ["--port", String(p)] }
        _ = await background { try cli.runRaw(args) }
    }

    /// Restore a database from `file`.
    func dbRestore(engine: String, name: String, port: Int?, file: String) async {
        let cli = self.cli
        var args = ["db", "restore", name, "--engine", engine, "--file", file]
        if let p = port { args += ["--port", String(p)] }
        _ = await background { try cli.runRaw(args) }
    }

    /// (connection URL, client command) for an engine on a port.
    func connectionStrings(engine: String, port: Int) -> (url: String, cmd: String) {
        switch engine {
        case "postgres":
            return ("postgresql://postgres@127.0.0.1:\(port)/postgres", "psql -h 127.0.0.1 -p \(port) -U postgres")
        case "mysql", "mariadb":
            return ("mysql://root@127.0.0.1:\(port)", "mysql -h 127.0.0.1 -P \(port) -u root")
        case "redis":
            return ("redis://127.0.0.1:\(port)", "redis-cli -p \(port)")
        default:
            return ("127.0.0.1:\(port)", "")
        }
    }

    func copyToPasteboard(_ s: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(s, forType: .string)
    }

    // MARK: Dumps (LaraDumps-style debugger)

    @Published var dumps: [DumpEntry] = []
    @Published var dumpSiteFilter: String? = nil
    @Published var dumpScreenFilter: String? = nil
    @Published var dumpTypeFilter: String? = nil // nil=all; else category: dump/query/log/mail/job/http
    @Published var dumpSearch: String = ""
    /// Editor for jump-to-source: "code" | "cursor" | "phpstorm" | "subl" | "system".
    @AppStorage("dumpsEditor") var dumpsEditor: String = "code"

    private var dumpStreamTask: Task<Void, Never>?
    private let dumpsPort = 9912
    private let maxDumps = 2000

    /// Start (idempotently) the persistent connection to dpld's dump stream.
    func startDumpsStream() {
        guard dumpStreamTask == nil else { return }
        dumpStreamTask = Task { [weak self] in
            guard let self else { return }
            let url = URL(string: "http://127.0.0.1:\(self.dumpsPort)/dumps/stream")!
            while !Task.isCancelled {
                do {
                    var req = URLRequest(url: url)
                    req.timeoutInterval = .infinity
                    let (bytes, _) = try await URLSession.shared.bytes(for: req)
                    for try await line in bytes.lines {
                        guard let data = line.data(using: .utf8),
                              let entry = try? JSONDecoder().decode(DumpEntry.self, from: data)
                        else { continue }
                        await self.appendDump(entry)
                    }
                } catch {
                    // Daemon down / connection dropped — back off and retry.
                }
                if Task.isCancelled { break }
                try? await Task.sleep(nanoseconds: 1_000_000_000)
            }
        }
    }

    /// A paused breakpoint waiting for Continue/Stop (blocks the PHP request).
    @Published var pausedDump: DumpEntry?

    private func appendDump(_ entry: DumpEntry) {
        // Backfill can resend ids we already have; de-dupe.
        if dumps.last?.id == entry.id || dumps.contains(where: { $0.id == entry.id }) { return }
        dumps.append(entry)
        if dumps.count > maxDumps { dumps.removeFirst(dumps.count - maxDumps) }
        if entry.pause == true, entry.token != nil { pausedDump = entry }
    }

    /// Resume a paused breakpoint: "continue" lets the request proceed, "stop"
    /// terminates it.
    func resumeDump(_ action: String) async {
        guard let token = pausedDump?.token else { return }
        pausedDump = nil
        var req = URLRequest(url: URL(string: "http://127.0.0.1:\(dumpsPort)/dumps/resume/\(token)?action=\(action)")!)
        req.httpMethod = "POST"
        _ = try? await URLSession.shared.data(for: req)
    }

    /// Clear both the daemon ring buffer and the local list.
    func clearDumps() async {
        dumps.removeAll()
        var req = URLRequest(url: URL(string: "http://127.0.0.1:\(dumpsPort)/dumps/clear")!)
        req.httpMethod = "POST"
        _ = try? await URLSession.shared.data(for: req)
    }

    var dumpSites: [String] {
        Array(Set(dumps.compactMap { $0.site })).sorted()
    }
    var dumpScreens: [String] {
        Array(Set(dumps.compactMap { $0.screen })).sorted()
    }

    var filteredDumps: [DumpEntry] {
        dumps.filter { d in
            (dumpSiteFilter == nil || d.site == dumpSiteFilter)
                && (dumpScreenFilter == nil || d.screen == dumpScreenFilter)
                && (dumpTypeFilter == nil || d.category == dumpTypeFilter)
                && (dumpSearch.isEmpty || matches(d, dumpSearch))
        }
    }

    private func matches(_ d: DumpEntry, _ q: String) -> Bool {
        let hay = [d.label, d.location, d.preview, d.site].compactMap { $0 }.joined(separator: " ").lowercased()
        return hay.contains(q.lowercased())
    }

    /// Open a dump's source file at its line in the configured editor.
    func openInEditor(file: String, line: Int?) {
        let ln = line ?? 1
        switch dumpsEditor {
        case "code", "cursor", "subl":
            let cmd = dumpsEditor == "subl" ? "subl" : dumpsEditor
            runEditor(cmd, ["-g", "\(file):\(ln)"])
        case "phpstorm":
            if let url = URL(string: "phpstorm://open?file=\(file.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? file)&line=\(ln)") {
                NSWorkspace.shared.open(url)
            }
        default:
            NSWorkspace.shared.open(URL(fileURLWithPath: file))
        }
    }

    private func runEditor(_ program: String, _ args: [String]) {
        // Editors' CLIs live in /usr/local/bin, /opt/homebrew/bin, or PATH.
        let candidates = ["/opt/homebrew/bin/\(program)", "/usr/local/bin/\(program)", program]
        for path in candidates {
            let proc = Process()
            if path.hasPrefix("/") {
                guard FileManager.default.isExecutableFile(atPath: path) else { continue }
                proc.executableURL = URL(fileURLWithPath: path)
                proc.arguments = args
            } else {
                proc.executableURL = URL(fileURLWithPath: "/usr/bin/env")
                proc.arguments = [program] + args
            }
            if (try? proc.run()) != nil { return }
        }
    }

    /// Set a site's PHP version (nil site → global default).
    func usePhp(version: String, site: String?) async {
        let cli = self.cli
        let args = site.map { ["use", version, $0] } ?? ["use", version, "--default"]
        _ = await background { try cli.runRaw(args) }
        await loadLocal()
    }

    func clearMail() async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["mail", "clear"]) }
        if let r = await background({ try cli.rows(["mail", "list"]) }) { mailMessages = r }
    }

    /// Raw body of one captured message (for the reader pane).
    func mailBody(_ id: String) async -> String {
        let cli = self.cli
        let data = await background { try cli.runRaw(["mail", "show", id]) }
        return data.map { String(decoding: $0, as: UTF8.self) } ?? ""
    }

    func loadTlds() async {
        let cli = self.cli
        if let v = await background({ try cli.json(["tld"]) }) {
            tlds = v.arrayValue?.compactMap { $0.stringValue } ?? []
        }
    }

    func addTld(_ name: String) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["tld", "add", name]) }
        await loadTlds()
    }

    func removeTld(_ name: String) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["tld", "remove", name]) }
        await loadTlds()
    }

    /// Distinct PHP versions available (for the per-site picker).
    var availablePhpVersions: [String] {
        phpVersions.compactMap { $0.first(["version"]) }
    }

    func refreshAccount() async {
        let cli = self.cli
        // whoami never errors on "not logged in" (it returns authenticated:false),
        // so a nil result here means a real failure worth surfacing.
        if let obj = await background({ try cli.object(["dply", "whoami"]) }) {
            account = obj
        }
    }

    // MARK: Detail fetches

    func edgeSite(_ id: String) throws -> Row {
        try cli.object(["dply", "edge:show", id])
    }

    func edgeDeployments(_ id: String) throws -> [Row] {
        try cli.rows(["dply", "edge:deployments", id])
    }

    func edgeEnv(_ id: String) throws -> [Row] {
        try cli.rows(["dply", "edge:env", id])
    }

    func edgeDomains(_ id: String) throws -> [Row] {
        try cli.rows(["dply", "edge:domains", id])
    }

    func serverSite(_ id: String) throws -> Row {
        try cli.object(["dply", "sites:show", id])
    }

    func serverSiteDeployments(_ id: String) throws -> [Row] {
        try cli.rows(["dply", "sites:deployments", id])
    }

    // MARK: Actions

    /// Deploy an edge site, then refresh the list.
    func deployEdge(_ id: String) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["dply", "edge:deploy", id]) }
        await refreshCurrentSection()
    }

    /// Deploy a server-hosted site.
    func deploySite(_ id: String) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["dply", "sites:deploy", id]) }
        await refreshCurrentSection()
    }

    /// Purge an edge site's cache.
    func purgeEdge(_ id: String) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["dply", "edge:purge", id]) }
    }

    /// Fetch a page of edge logs as formatted text (for the logs sheet).
    func edgeLogsText(_ id: String, limit: Int = 100) throws -> String {
        let data = try cli.runRaw(["dply", "edge:logs", id, "--limit", String(limit)])
        return String(decoding: data, as: UTF8.self)
    }

    func openURL(_ string: String) {
        guard let url = URL(string: string) else { return }
        NSWorkspace.shared.open(url)
    }

    /// Run a throwing CLI call off the main actor, returning its result or nil
    /// (recording the error). Detail views use this to load without blocking
    /// the UI. Pass a captured `cli` value so nothing on the main actor is
    /// touched off-thread.
    func background<T>(_ work: @escaping () throws -> T) async -> T? {
        let result: Result<T, Error> = await Task.detached {
            do { return .success(try work()) } catch { return .failure(error) }
        }.value
        switch result {
        case .success(let value):
            return value
        case .failure(let error):
            lastError = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
            return nil
        }
    }

    /// Like `background`, but swallows errors silently (returns nil) instead of
    /// raising the global banner — for expected failures a view handles itself
    /// (e.g. listing databases on a stopped service).
    func backgroundQuiet<T>(_ work: @escaping () throws -> T) async -> T? {
        let result: Result<T, Error> = await Task.detached {
            do { return .success(try work()) } catch { return .failure(error) }
        }.value
        if case .success(let value) = result { return value }
        return nil
    }

    /// Is a given database engine currently running?
    func isServiceRunning(_ engine: String) -> Bool {
        dbServices.first { $0.first(["name"]) == engine }?.dig("running") == .bool(true)
    }
}
