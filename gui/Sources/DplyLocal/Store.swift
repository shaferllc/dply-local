import Foundation
import SwiftUI
import Darwin

/// An installed PHP version and its binary — for the per-version extension /
/// OPcache manager.
struct PhpInstall: Identifiable, Hashable {
    let version: String
    let binary: String
    var id: String { version }
}

/// OPcache settings for one PHP version (read from its merged config, written
/// via a dpl-managed conf.d file).
struct OpcacheConfig: Equatable {
    var loaded = false
    var enabled = false
    var memoryMB = 128
    var maxFiles = 10000
    var validateTimestamps = true
    var revalidateFreq = 2
    var jit = false
    var jitBufferMB = 0

    /// dev = revalidates on every request; prod = trusts cache; off = disabled.
    var mode: String {
        if !enabled { return "off" }
        return validateTimestamps ? "dev" : "prod"
    }

    static func preset(_ name: String) -> OpcacheConfig {
        switch name {
        case "dev":
            return OpcacheConfig(loaded: true, enabled: true, memoryMB: 256, maxFiles: 20000,
                                 validateTimestamps: true, revalidateFreq: 0, jit: true, jitBufferMB: 100)
        case "prod":
            return OpcacheConfig(loaded: true, enabled: true, memoryMB: 256, maxFiles: 20000,
                                 validateTimestamps: false, revalidateFreq: 0, jit: true, jitBufferMB: 256)
        default:
            return OpcacheConfig(loaded: true, enabled: false)
        }
    }
}

/// One site's Xdebug state, as reported by `dpl xdebug --json`.
struct XdebugSite: Identifiable, Equatable {
    let name: String
    /// Pinned PHP version, or `nil` for the default.
    let php: String?
    /// Canonical mode, e.g. `off`, `debug`, `debug,develop`.
    let mode: String
    /// Whether Xdebug exists for this site's PHP version at all.
    let installed: Bool

    var id: String { name }
    var stepDebug: Bool { has("debug") }
    var develop: Bool { has("develop") }
    var isOff: Bool { mode == "off" }
    /// The site wants Xdebug but its PHP version doesn't have it.
    var missing: Bool { !isOff && !installed }

    /// Whole-part match — `mode.contains("debug")` would also match `gcstats`
    /// free spellings like `xdebug`, and would call `develop` a step-debug mode.
    func has(_ part: String) -> Bool {
        mode.split(separator: ",").contains { $0 == Substring(part) }
    }
}

/// Xdebug state for the whole machine: the shared IDE settings plus one entry
/// per site. The daemon owns this; the GUI never reads it back out of PHP.
///
/// Reading it from PHP is not merely redundant, it is wrong: dpl sets the mode
/// through the `XDEBUG_MODE` environment variable, and Xdebug deliberately does
/// not write that back into the `xdebug.mode` setting. `ini_get("xdebug.mode")`
/// therefore reports `""` no matter which mode is actually live.
struct XdebugState: Equatable {
    var clientPort = 9003
    var ideKey = "PHPSTORM"
    var sites: [XdebugSite] = []

    func site(_ name: String) -> XdebugSite? { sites.first { $0.name == name } }
    /// Sites currently running with Xdebug on — drives the menu-bar indicator.
    var active: [XdebugSite] { sites.filter { !$0.isOff } }
}

/// One attachment or inline part of a captured message.
struct MailAttachment: Identifiable, Hashable {
    let index: Int
    let name: String
    let mime: String
    let size: Int
    /// Referenced from the HTML body by `cid:`, not offered as a download.
    let inline: Bool

    var id: Int { index }

    var humanSize: String {
        let kb = 1024, mb = kb * 1024
        switch size {
        case mb...: return String(format: "%.1f MB", Double(size) / Double(mb))
        case kb...: return String(format: "%.1f KB", Double(size) / Double(kb))
        default: return "\(size) B"
        }
    }
}

/// A fully parsed message, from `dpl mail show <id> --json`.
struct MailMessage {
    let id: String
    let mailbox: String
    let from: String
    let to: String
    let cc: String
    let subject: String
    let date: String?
    let size: Int
    let text: String?
    /// HTML with `cid:` images already inlined as `data:` URIs, so it renders
    /// fully with the network blocked.
    let html: String?
    /// How many http(s) resources the HTML would fetch if we let it.
    let remoteResources: Int
    let links: [String]
    let attachments: [MailAttachment]
    let headers: [(name: String, value: String)]

    /// Attachments worth showing as downloads — inline parts belong to the body.
    var downloads: [MailAttachment] { attachments.filter { !$0.inline } }

    init?(_ value: JSONValue) {
        guard let o = value.objectValue, let id = o["id"]?.stringValue else { return nil }
        self.id = id
        mailbox = o["mailbox"]?.stringValue ?? "-"
        from = o["from"]?.stringValue ?? ""
        to = o["to"]?.stringValue ?? ""
        cc = o["cc"]?.stringValue ?? ""
        subject = o["subject"]?.stringValue ?? ""
        date = o["date"]?.stringValue
        size = o["size"]?.intValue ?? 0
        text = o["text"]?.stringValue
        html = o["html"]?.stringValue
        remoteResources = o["remote_resources"]?.intValue ?? 0
        links = (o["links"]?.arrayValue ?? []).compactMap(\.stringValue)
        attachments = (o["attachments"]?.arrayValue ?? []).compactMap { item -> MailAttachment? in
            guard let a = item.objectValue, let index = a["index"]?.intValue else { return nil }
            return MailAttachment(
                index: index,
                name: a["name"]?.stringValue ?? "(unnamed)",
                mime: a["mime"]?.stringValue ?? "application/octet-stream",
                size: a["size"]?.intValue ?? 0,
                inline: a["inline"]?.boolValue ?? false
            )
        }
        // Serialized as a list of [name, value] pairs.
        headers = (o["headers"]?.arrayValue ?? []).compactMap { pair in
            guard let p = pair.arrayValue, p.count == 2,
                  let name = p[0].stringValue, let value = p[1].stringValue else { return nil }
            return (name, value)
        }
    }
}

/// A `valet link`ed site discovered during import (name may differ from folder).
struct ValetSite: Identifiable, Hashable {
    let name: String
    let path: String
    let exists: Bool
    var id: String { name }
}

/// What an existing Laravel Valet install has configured.
struct ValetSnapshot {
    var installed = false
    var tld = "test"
    var parked: [String] = []
    var linked: [ValetSite] = []
}

/// User-authored notes pinned to a site: what it is, what's left to do, where
/// it lives beyond this machine, and the other code that belongs to the same
/// project. Persisted in the project itself (`.dpl/project.json`) so it
/// travels with the repo.
struct SiteProject: Codable, Equatable {
    /// One checklist item.
    struct Todo: Codable, Equatable, Identifiable {
        var id = UUID()
        var text: String
        var done = false
    }

    /// Another area of code that's part of this project but isn't a site —
    /// an SDK, a composer package, a shared library checkout.
    struct RelatedPackage: Codable, Equatable, Identifiable {
        var id = UUID()
        var name: String
        var path: String
    }

    /// What the site is about — a short free-text description.
    var summary = ""
    /// Where this project lives in production, if anywhere.
    var productionURL = ""
    /// A staging / testing URL, if the project has one.
    var testingURL = ""
    var todos: [Todo] = []
    /// Names of other local sites that are part of the same project
    /// (the API next to the app, the admin next to the storefront).
    var relatedSites: [String] = []
    /// Non-site code areas that belong to this project.
    var relatedPackages: [RelatedPackage] = []

    /// Nothing worth surfacing yet — used to keep the section quiet until the
    /// user has actually written something.
    var isEmpty: Bool {
        summary.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && productionURL.isEmpty && testingURL.isEmpty && todos.isEmpty
            && relatedSites.isEmpty && relatedPackages.isEmpty
    }

    /// Open to-do count, for a badge on the section header.
    var openTodos: Int { todos.filter { !$0.done }.count }

    init() {}

    /// Hand-rolled so project.json files written before the related-sites/
    /// packages fields existed still decode (synthesized Decodable treats a
    /// missing key as an error, defaults notwithstanding).
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        summary = try c.decodeIfPresent(String.self, forKey: .summary) ?? ""
        productionURL = try c.decodeIfPresent(String.self, forKey: .productionURL) ?? ""
        testingURL = try c.decodeIfPresent(String.self, forKey: .testingURL) ?? ""
        todos = try c.decodeIfPresent([Todo].self, forKey: .todos) ?? []
        relatedSites = try c.decodeIfPresent([String].self, forKey: .relatedSites) ?? []
        relatedPackages = try c.decodeIfPresent([RelatedPackage].self, forKey: .relatedPackages) ?? []
    }
}

/// Run an executable and capture stdout (off the main actor). Used to query
/// the active PHP binary for its config.
func runProcess(_ exe: String, _ args: [String]) -> String {
    let proc = Process()
    proc.executableURL = URL(fileURLWithPath: exe)
    proc.arguments = args
    // Launched from Finder, we inherit launchd's bare PATH — without this, every
    // `which` probe below reports Homebrew and Composer missing on a machine that
    // has them.
    proc.environment = DplyCLI.toolEnvironment
    let out = Pipe()
    proc.standardOutput = out
    proc.standardError = Pipe()
    do { try proc.run() } catch { return "" }
    let data = out.fileHandleForReading.readDataToEndOfFile()
    proc.waitUntilExit()
    return String(decoding: data, as: UTF8.self)
}

/// List extension .ini files in a PHP conf.d directory (enabled = `.ini`,
/// disabled = `.ini.disabled`).
func scanExtensions(_ dir: String) -> [Store.PhpExt] {
    guard let files = try? FileManager.default.contentsOfDirectory(atPath: dir) else { return [] }
    var out: [Store.PhpExt] = []
    for f in files.sorted() {
        // Extensions are the numbered `NN-name.ini` files; skip brew's
        // `.ini.default` and non-extension config (error_log.ini, etc.).
        guard f.first?.isNumber == true else { continue }
        let enabled = f.hasSuffix(".ini")
        let disabled = f.hasSuffix(".ini.disabled")
        guard enabled || disabled else { continue }
        // Name: strip a leading NN- ordering prefix and the .ini(.disabled) suffix.
        var name = f
        if let range = name.range(of: ".ini") { name = String(name[..<range.lowerBound]) }
        if let dash = name.firstIndex(of: "-"), name[..<dash].allSatisfy(\.isNumber) {
            name = String(name[name.index(after: dash)...])
        }
        out.append(Store.PhpExt(name: name, enabled: enabled, path: dir + "/" + f))
    }
    return out
}

/// Find xdebug's `zend_extension` .so path from a conf.d directory — reading it
/// out of any `*xdebug*.ini` even when the line is commented (`; zend_extension=`).
/// Homebrew/shivammathur install xdebug as a keg-path zend_extension, so this is
/// how we both detect it and re-load it.
func xdebugSoPath(inDir dir: String) -> String? {
    guard let files = try? FileManager.default.contentsOfDirectory(atPath: dir) else { return nil }
    for f in files where f.lowercased().contains("xdebug") && f.contains(".ini") {
        guard let content = try? String(contentsOfFile: dir + "/" + f, encoding: .utf8) else { continue }
        for raw in content.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = raw.trimmingCharacters(in: CharacterSet(charactersIn: "; \t"))
            if line.lowercased().hasPrefix("zend_extension"), let eq = line.firstIndex(of: "=") {
                let val = line[line.index(after: eq)...].trimmingCharacters(in: CharacterSet(charactersIn: " \"'"))
                if !val.isEmpty { return val }
            }
        }
    }
    return nil
}

/// A detected background worker process.
struct WorkerItem: Identifiable { let pid: Int; let name: String; var id: Int { pid } }
/// A group of workers of one kind (Horizon, Queue, …).
struct WorkerGroup: Identifiable { let kind: String; let items: [WorkerItem]; var id: String { kind } }

/// A dev server the daemon supervises — the controllable half of the Workers
/// card, as opposed to the processes we merely observe in `ps`.
/// One supervised Laravel Octane server, as `dpl octane --json` reports it.
struct OctaneServer: Identifiable, Equatable {
    let site: String
    /// e.g. `octane-frankenphp`.
    let runtime: String
    /// The loopback port the proxy forwards the site to.
    let port: Int?
    let running: Bool
    /// Whether saving a file reloads its workers.
    let watch: Bool
    let reloads: Int
    /// Why it isn't running, when it isn't.
    let detail: String?
    var id: String { site }
    /// Just the server name: `frankenphp`, `swoole`, `roadrunner`.
    var server: String {
        runtime.hasPrefix("octane-") ? String(runtime.dropFirst("octane-".count)) : runtime
    }
}

struct DevServer: Identifiable, Equatable {
    let site: String
    let script: String
    let agent: String
    let running: Bool
    /// Process group, used to suppress the `ps`-detected duplicate.
    let pgid: Int?
    let port: Int?
    /// Why it isn't running, when it isn't.
    let detail: String?
    var id: String { site }
}

/// Working directory of each pid (via `lsof`), for labeling worker processes.
func cwds(for pids: [Int]) -> [Int: String] {
    guard !pids.isEmpty else { return [:] }
    let out = runProcess("/usr/sbin/lsof", ["-a", "-d", "cwd", "-p", pids.map(String.init).joined(separator: ","), "-Fn"])
    var map: [Int: String] = [:]
    var cur = 0
    for line in out.split(separator: "\n") {
        let s = String(line)
        if s.hasPrefix("p") { cur = Int(s.dropFirst()) ?? 0 }
        else if s.hasPrefix("n"), cur != 0 { map[cur] = String(s.dropFirst()) }
    }
    return map
}

/// Is a command on PATH?
func whichExists(_ name: String) -> Bool {
    !runProcess("/usr/bin/env", ["which", name]).trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
}

/// A prerequisite for local PHP development, for the Setup Assistant.
struct SetupRequirement: Identifiable {
    let name: String
    let detail: String
    let ok: Bool
    let installCommand: String
    var id: String { name }
}

/// Rename a file (used to enable/disable an extension .ini).
func renameFile(_ from: String, _ to: String) -> Bool {
    (try? FileManager.default.moveItem(atPath: from, toPath: to)) != nil
}

/// Sidebar sections. Local `.test` sites will join this list once the daemon
/// phases land; for now the app manages the dply surfaces the CLI already
/// exposes.
enum Surface: String, CaseIterable, Identifiable {
    case dashboard = "Dashboard"
    case local = "Local Sites"
    case services = "Services"
    case mail = "Mail"
    case dumps = "Dumps"
    case php = "PHP"
    case extensions = "Extensions"
    case status = "Status"
    case system = "System"
    case doctor = "Doctor"
    case settings = "Settings"
    case edgeSites = "Edge Sites"
    case sites = "Server Sites"
    case servers = "Servers"
    case account = "Account"
    case keelSites = "Keel Sites"
    case keelAccount = "Keel Cloud"

    var id: String { rawValue }

    /// The local-tooling surfaces (no dply login required).
    var isLocal: Bool {
        switch self {
        case .dashboard, .local, .services, .mail, .dumps, .php, .extensions,
             .status, .system, .doctor, .settings:
            return true
        default:
            return false
        }
    }
    var isDply: Bool {
        switch self {
        case .edgeSites, .sites, .servers, .account: return true
        default: return false
        }
    }
    /// The Keel Cloud surfaces (need a Keel Cloud token).
    var isKeel: Bool {
        switch self {
        case .keelSites, .keelAccount: return true
        default: return false
        }
    }

    /// Surfaces that are a single page rather than a list + detail, so they get
    /// the whole window instead of being squeezed into the middle column.
    var isFullWidth: Bool {
        switch self {
        case .dashboard, .php, .extensions, .status, .system, .doctor, .settings: return true
        default: return false
        }
    }

    var systemImage: String {
        switch self {
        case .dashboard: return "square.grid.2x2.fill"
        case .local: return "house"
        case .services: return "cylinder.split.1x2"
        case .mail: return "envelope"
        case .dumps: return "ladybug"
        case .php: return "chevron.left.forwardslash.chevron.right"
        case .extensions: return "puzzlepiece.extension"
        case .status: return "waveform.path.ecg"
        case .system: return "square.stack.3d.up.fill"
        case .doctor: return "stethoscope"
        case .settings: return "gearshape"
        case .edgeSites: return "bolt.horizontal.circle"
        case .sites: return "server.rack"
        case .servers: return "externaldrive.connected.to.line.below"
        case .account: return "person.crop.circle"
        case .keelSites: return "sailboat"
        case .keelAccount: return "cloud"
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

    /// Whether the dply platform surfaces are shown at all. Opt-in: `dpl` is a
    /// local dev environment first, and most users never log in to dply.
    /// `@Published` rather than `@AppStorage` so the sidebar reacts immediately.
    @Published var dplyEnabled: Bool = Store.initialDplyEnabled() {
        didSet {
            UserDefaults.standard.set(dplyEnabled, forKey: "dplyEnabled")
            // Don't strand the user on a surface that just disappeared.
            if !dplyEnabled && section.isDply { section = .dashboard }
        }
    }

    /// Whether the Keel Cloud surfaces are shown. Opt-in, exactly like dply.
    @Published var keelEnabled: Bool = Store.initialKeelEnabled() {
        didSet {
            UserDefaults.standard.set(keelEnabled, forKey: "keelEnabled")
            if !keelEnabled && section.isKeel { section = .dashboard }
        }
    }

    /// First launch: on iff a Keel Cloud token is already stored (CLI or MCP
    /// setup predating the GUI surface) — the same grandfathering as dply.
    private static func initialKeelEnabled() -> Bool {
        let defaults = UserDefaults.standard
        if defaults.object(forKey: "keelEnabled") != nil {
            return defaults.bool(forKey: "keelEnabled")
        }
        let enabled = hasKeelLogin()
        defaults.set(enabled, forKey: "keelEnabled")
        return enabled
    }

    /// Whether `~/.keel/cloud.json` holds a token.
    static func hasKeelLogin() -> Bool {
        let path = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".keel/cloud.json")
        guard let data = try? Data(contentsOf: path),
              let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return false }
        return !((root["token"] as? String) ?? "").isEmpty
    }

    /// Off by default — but somebody who already logged in through the CLI was
    /// using dply before it became opt-in, so honour that on the first launch.
    private static func initialDplyEnabled() -> Bool {
        let defaults = UserDefaults.standard
        if defaults.object(forKey: "dplyEnabled") != nil {
            return defaults.bool(forKey: "dplyEnabled")
        }
        let enabled = hasDplyLogin()
        defaults.set(enabled, forKey: "dplyEnabled")
        return enabled
    }

    /// Does the shared `~/.dply/config.json` hold a token for any host?
    private static func hasDplyLogin() -> Bool {
        let path = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".dply/config.json")
        guard let data = try? Data(contentsOf: path),
              let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let hosts = root["hosts"] as? [String: Any]
        else { return false }
        return hosts.values.contains { host in
            let token = (host as? [String: Any])?["token"] as? String
            return !(token ?? "").isEmpty
        }
    }

    // Navigation. Local sites lead — that's the primary use; dply is secondary.
    @Published var section: Surface = .dashboard
    /// Drives the first-run (and re-openable) onboarding wizard.
    @Published var showOnboarding = false

    // Data.
    @Published var localSites: [Row] = []
    @Published var dbServices: [Row] = []
    @Published var mailMessages: [Row] = []
    @Published var phpVersions: [Row] = []
    /// The full installable-PHP catalog (`dpl php available`): every known line
    /// with installed/active/broken status — backs the version manager.
    @Published var phpCatalog: [Row] = []
    @Published var tlds: [String] = []
    /// 1-minute system load average, polled while the app is open.
    @Published var loadAvg: Double = 0
    /// Logical CPU count, to normalize the load into a per-core ratio.
    let cpuCount = max(1, ProcessInfo.processInfo.activeProcessorCount)
    private var loadTimer: Timer?
    @Published var edgeSites: [Row] = []
    @Published var sites: [Row] = []
    @Published var servers: [Row] = []
    @Published var account: Row?
    @Published var keelSites: [Row] = []
    @Published var keelAccount: Row?

    // Status.
    @Published var isLoading = false
    @Published var lastError: String?

    /// Latest `dpl doctor` report, shared by the Doctor page's list and detail
    /// columns so a re-run updates both at once.
    @Published var doctorHealth: DoctorReport?
    @Published var doctorRunning = false

    /// Which PHP version the Extensions page should open on, when something
    /// navigated there for a specific one (e.g. a site's context menu).
    @Published var extensionsVersion: String?

    /// A site name something wants the Local Sites list to select — set by a
    /// "related site" chip; ContentView consumes and clears it (the same
    /// request/consume pattern as `extensionsVersion`).
    @Published var siteJump: String?

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
        // Only touch dply (which needs a login) when it's enabled and active.
        if dplyEnabled && section.isDply { await refreshAccount() }
        // PHP versions + TLDs are cheap and used across panes (site picker,
        // settings), so keep them warm.
        await loadPhpVersions()
        await loadPhpConfig()
        await loadTlds()
        // Keep the dumps stream live from launch so dumps accumulate even
        // before the panel is opened.
        startDumpsStream()
        // An explicit refresh (launch, ⌘R, the toolbar button) always fetches.
        await refreshCurrentSection(force: true)
    }

    func loadPhpVersions() async {
        let cli = self.cli
        if let r = await background({ try cli.rows(["php"]) }) { phpVersions = r }
    }

    /// Refresh the installable-PHP catalog for the version manager.
    func loadPhpCatalog() async {
        let cli = self.cli
        if let r = await background({ try cli.rows(["php", "available"]) }) { phpCatalog = r }
    }

    // MARK: Per-version extensions + OPcache

    /// Installed PHP versions with their binaries (for the extension manager).
    var installedPhp: [PhpInstall] {
        phpVersions.compactMap {
            let v = $0.cell(["version"]), b = $0.cell(["binary"])
            return v.isEmpty || b.isEmpty ? nil : PhpInstall(version: v, binary: b)
        }
    }

    /// The conf.d directory a PHP binary scans for extension .ini files.
    private func confDir(forBinary bin: String) async -> String? {
        let ini = await backgroundQuiet { runProcess(bin, ["-d", "display_errors=stderr", "--ini"]) } ?? ""
        return value(in: ini, after: "Scan for additional .ini files in:")
    }

    /// Extensions configured for a specific PHP version.
    func extensions(forBinary bin: String) async -> [PhpExt] {
        guard let dir = await confDir(forBinary: bin) else { return [] }
        return await backgroundQuiet { scanExtensions(dir) } ?? []
    }

    /// Enable/disable one extension for a version, then restart its fpm pool.
    func setExtension(_ ext: PhpExt, enabled: Bool) async {
        guard enabled != ext.enabled else { return }
        let from = ext.path
        let to = enabled ? String(ext.path.dropLast(".disabled".count)) : ext.path + ".disabled"
        _ = await backgroundQuiet { renameFile(from, to) }
        await restartDaemon()
    }

    /// Uninstall an extension for a version (brew remove) + restart fpm.
    func uninstallExtension(_ name: String, version: String) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["php", "ext-uninstall", version, name]) }
        await restartDaemon()
    }

    /// Extensions installable for a version (from the shivammathur tap) that
    /// aren't already present. Slow (brew search) — call off the hot path.
    func availableExtensions(forVersion v: String) async -> [String] {
        let cli = self.cli
        guard let val = await background({ try cli.json(["php", "ext-available", v]) }) else { return [] }
        return val.arrayValue?.compactMap { $0.objectValue?["name"]?.stringValue } ?? []
    }

    /// Machine-wide Xdebug state, owned by the daemon.
    @Published var xdebug = XdebugState()

    /// Refresh from `dpl xdebug --json`.
    func loadXdebug() async {
        let cli = self.cli
        guard let val = await background({ try cli.json(["xdebug"]) }),
              let obj = val.objectValue else { return }
        var next = XdebugState()
        if let p = obj["client_port"]?.intValue { next.clientPort = p }
        if let k = obj["ide_key"]?.stringValue, !k.isEmpty { next.ideKey = k }
        next.sites = (obj["sites"]?.arrayValue ?? []).compactMap { item -> XdebugSite? in
            guard let o = item.objectValue, let name = o["name"]?.stringValue else { return nil }
            return XdebugSite(
                name: name,
                php: o["php"]?.stringValue,
                mode: o["mode"]?.stringValue ?? "off",
                installed: o["installed"]?.boolValue ?? false
            )
        }
        xdebug = next
    }

    /// Set one site's mode, or the default for every site when `site` is nil.
    /// The daemon moves the site onto a php-fpm pool for that mode and restarts
    /// only what it must, so there is no `restartDaemon()` here.
    func setXdebug(mode: String, site: String? = nil) async {
        let cli = self.cli
        var args = ["xdebug", "mode", mode]
        if let site { args.append(site) }
        _ = await background { try cli.runRaw(args) }
        await loadXdebug()
        await loadLocal()
    }

    /// Toggle step debugging for one site.
    func toggleXdebug(site: String) async {
        let on = xdebug.site(site)?.stepDebug ?? false
        await setXdebug(mode: on ? "off" : "debug", site: site)
    }

    /// Toggle the machine-wide default (what the menu-bar item drives).
    func toggleDefaultXdebug() async {
        let anyOn = !xdebug.active.isEmpty
        await setXdebug(mode: anyOn ? "off" : "debug")
    }

    func setXdebugPort(_ port: Int) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["xdebug", "port", String(port)]) }
        await loadXdebug()
    }

    func setXdebugIdeKey(_ key: String) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["xdebug", "ide", key]) }
        await loadXdebug()
    }

    /// Whether Xdebug exists for a PHP binary — a filesystem check, so it stays
    /// correct for a version that has the extension installed but commented out.
    func xdebugInstalled(forBinary bin: String) async -> Bool {
        guard let dir = await confDir(forBinary: bin) else { return false }
        guard let so = await backgroundQuiet({ xdebugSoPath(inDir: dir) }) ?? nil else { return false }
        return FileManager.default.fileExists(atPath: so)
    }

    /// Read a version's OPcache configuration.
    func opcacheConfig(forBinary bin: String) async -> OpcacheConfig {
        let script = "echo json_encode(['loaded'=>extension_loaded('Zend OPcache'),"
            + "'enable'=>ini_get('opcache.enable'),'mem'=>ini_get('opcache.memory_consumption'),"
            + "'maxf'=>ini_get('opcache.max_accelerated_files'),'val'=>ini_get('opcache.validate_timestamps'),"
            + "'rev'=>ini_get('opcache.revalidate_freq'),'jit'=>ini_get('opcache.jit'),"
            + "'jitbuf'=>ini_get('opcache.jit_buffer_size')]);"
        let out = await backgroundQuiet { runProcess(bin, ["-d", "display_errors=stderr", "-r", script]) } ?? ""
        var cfg = OpcacheConfig()
        guard let data = out.data(using: .utf8),
              let o = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return cfg }
        func truthy(_ v: Any?) -> Bool {
            if let b = v as? Bool { return b }
            if let i = v as? Int { return i != 0 }
            if let s = v as? String { return s == "1" || s.lowercased() == "on" || s.lowercased() == "true" }
            return false
        }
        func intVal(_ v: Any?) -> Int? {
            if let i = v as? Int { return i }
            if let s = v as? String { return Int(s) }
            return nil
        }
        cfg.loaded = truthy(o["loaded"])
        cfg.enabled = truthy(o["enable"])
        cfg.memoryMB = intVal(o["mem"]) ?? 128
        cfg.maxFiles = intVal(o["maxf"]) ?? 10000
        cfg.validateTimestamps = truthy(o["val"])
        cfg.revalidateFreq = intVal(o["rev"]) ?? 2
        let jit = (o["jit"] as? String) ?? String(intVal(o["jit"]) ?? 0)
        cfg.jit = !jit.isEmpty && jit != "0" && jit.lowercased() != "off" && jit.lowercased() != "disable"
        if let buf = o["jitbuf"] as? String {
            // e.g. "100M", "128M", or a raw byte count.
            let digits = buf.prefix { $0.isNumber }
            let n = Int(digits) ?? 0
            cfg.jitBufferMB = buf.uppercased().contains("M") ? n : (buf.uppercased().contains("G") ? n * 1024 : n / (1024 * 1024))
        } else {
            cfg.jitBufferMB = (intVal(o["jitbuf"]) ?? 0) / (1024 * 1024)
        }
        return cfg
    }

    /// Write a version's OPcache settings (dpl-managed conf.d file) + restart fpm.
    func applyOpcache(_ cfg: OpcacheConfig, binary bin: String) async {
        guard let dir = await confDir(forBinary: bin) else { return }
        let jit = cfg.jit ? "tracing" : "off"
        let body = """
        ; Managed by dpl — OPcache tuning (edit via the Extensions panel)
        opcache.enable=\(cfg.enabled ? 1 : 0)
        opcache.enable_cli=0
        opcache.memory_consumption=\(cfg.memoryMB)
        opcache.interned_strings_buffer=16
        opcache.max_accelerated_files=\(cfg.maxFiles)
        opcache.validate_timestamps=\(cfg.validateTimestamps ? 1 : 0)
        opcache.revalidate_freq=\(cfg.revalidateFreq)
        opcache.jit=\(jit)
        opcache.jit_buffer_size=\(cfg.jitBufferMB)M

        """
        let path = dir + "/zz-dpl-opcache.ini"
        _ = await backgroundQuiet { (try? body.write(toFile: path, atomically: true, encoding: .utf8)) != nil }
        await restartDaemon()
    }

    /// When each section last loaded, so returning to one within a short window
    /// reuses what's already in memory instead of re-shelling out to `dpl`.
    /// Every sidebar click used to re-run the section's commands unconditionally.
    private var sectionRefreshedAt: [Surface: Date] = [:]
    /// How long a section's data is considered fresh enough to skip a reload.
    private let sectionStaleAfter: TimeInterval = 2

    /// Load the data the current section needs. Skips the work when the section
    /// was refreshed within `sectionStaleAfter` — unless `force` (⌘R, the
    /// refresh button, and post-action reloads always fetch).
    func refreshCurrentSection(force: Bool = false) async {
        // Capture the section now — the user may navigate away mid-load, and we
        // want the freshness stamp and the switch to agree on one target.
        let target = section
        if !force, let at = sectionRefreshedAt[target],
           Date().timeIntervalSince(at) < sectionStaleAfter {
            return
        }
        let cli = self.cli
        isLoading = true
        defer {
            isLoading = false
            sectionRefreshedAt[target] = Date()
        }
        switch target {
        case .dashboard:
            if let r = await background({ try cli.rows(["sites"]) }) { localSites = r }
            await loadServices()
        case .local:
            if let r = await background({ try cli.rows(["sites"]) }) { localSites = r }
        case .services:
            if let r = await background({ try cli.rows(["services"]) }) { dbServices = r }
            await loadVersions()
        case .mail:
            await loadMail()
        case .dumps:
            break // dumps arrive via the live stream, not a refresh
        case .status, .system, .doctor:
            break // both load their own report on appear
        case .settings:
            await loadTlds()
        case .php:
            if let r = await background({ try cli.rows(["php"]) }) { phpVersions = r }
            await loadPhpCatalog()
        case .extensions:
            if let r = await background({ try cli.rows(["php"]) }) { phpVersions = r }
        case .edgeSites:
            if let r = await background({ try cli.rows(["dply", "edge:sites"]) }) { edgeSites = r }
        case .sites:
            if let r = await background({ try cli.rows(["dply", "sites:list"]) }) { sites = r }
        case .servers:
            if let r = await background({ try cli.rows(["dply", "servers:list"]) }) { servers = r }
        case .account:
            await refreshAccount()
        case .keelSites:
            if let r = await background({ try cli.rows(["keel", "sites:list"]) }) { keelSites = r }
        case .keelAccount:
            await refreshKeelAccount()
        }
    }

    // MARK: Keel Cloud

    func refreshKeelAccount() async {
        let cli = self.cli
        // `whoami` fails cleanly when logged out; a nil account renders the
        // login state.
        keelAccount = await backgroundQuiet { try cli.object(["keel", "whoami"]) }
    }

    /// Store a pasted Keel Cloud token (and optional non-default URL).
    func keelLogin(token: String, url: String) async {
        let cli = self.cli
        var args = ["keel", "login", "--token", token]
        let trimmed = url.trimmingCharacters(in: .whitespaces)
        if !trimmed.isEmpty { args.append(contentsOf: ["--url", trimmed]) }
        _ = await background { try cli.runRaw(args) }
        await refreshKeelAccount()
        if keelAccount != nil { keelEnabled = true }
    }

    func keelLogout() async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["keel", "logout"]) }
        keelAccount = nil
    }

    /// Deploy a Keel Cloud site to production (or preview).
    func keelDeploy(id: String, preview: Bool = false) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["keel", preview ? "preview" : "publish", id]) }
        await refreshCurrentSection(force: true)
    }

    /// Recent deploys for a Keel Cloud site (detail pane).
    func keelDeploys(id: String) async -> [Row] {
        let cli = self.cli
        return await backgroundQuiet { try cli.rows(["keel", "deploys", id]) } ?? []
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

    /// Link a project under a custom name, optionally securing it, then refresh.
    func linkLocalAs(path: String, name: String, secure: Bool) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["link", path, "--name", name]) }
        if secure { _ = await backgroundQuiet { try cli.runRaw(["secure", name]) } }
        await loadLocal()
    }

    // MARK: Valet import

    /// Check the machine's PHP-dev prerequisites for the Setup Assistant.
    func checkSetupRequirements() async -> [SetupRequirement] {
        let clt = await backgroundQuiet {
            !runProcess("/usr/bin/xcode-select", ["-p"]).trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        } ?? false
        let brew = await backgroundQuiet { whichExists("brew") } ?? false
        let phpOnPath = await backgroundQuiet { whichExists("php") } ?? false
        let php = !phpVersions.isEmpty || phpOnPath
        let composer = await backgroundQuiet { whichExists("composer") } ?? false
        return [
            SetupRequirement(name: "Command Line Tools", detail: "Apple's developer tools (compilers, git).",
                             ok: clt, installCommand: "xcode-select --install"),
            SetupRequirement(name: "Homebrew", detail: "The package manager dpl uses for PHP & databases.",
                             ok: brew, installCommand: "/bin/bash -c \"$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)\""),
            SetupRequirement(name: "PHP", detail: "At least one PHP version (php-fpm).",
                             ok: php, installCommand: "brew install php"),
            SetupRequirement(name: "Composer", detail: "PHP's dependency manager.",
                             ok: composer, installCommand: "brew install composer"),
        ]
    }

    /// Read what an existing Laravel Valet install has configured.
    func valetSnapshot() async -> ValetSnapshot {
        let cli = self.cli
        guard let v = await background({ try cli.json(["valet", "list"]) }), let obj = v.objectValue else {
            return ValetSnapshot()
        }
        let parked = obj["parked"]?.arrayValue?.compactMap { $0.stringValue } ?? []
        let linked = obj["linked"]?.arrayValue?.compactMap { el -> ValetSite? in
            guard let o = el.objectValue else { return nil }
            return ValetSite(
                name: o["name"]?.stringValue ?? "",
                path: o["path"]?.stringValue ?? "",
                exists: o["exists"] == .bool(true)
            )
        } ?? []
        return ValetSnapshot(
            installed: obj["installed"] == .bool(true),
            tld: obj["tld"]?.stringValue ?? "test",
            parked: parked,
            linked: linked
        )
    }

    /// Import a selected set of Valet sites in ONE bulk operation (a single
    /// daemon reconcile). Writes a manifest and calls `dpl valet import`.
    /// Returns the number of sites imported.
    @discardableResult
    func valetImport(parked: [String], linked: [ValetSite], matchTld: Bool, tld: String) async -> Int {
        let cli = self.cli
        if matchTld { await adoptTld(tld) }
        let links = linked.filter(\.exists).map { ["name": $0.name, "path": $0.path] }
        let manifest: [String: Any] = ["parked": parked, "links": links]
        guard let data = try? JSONSerialization.data(withJSONObject: manifest) else { return 0 }
        let tmp = (NSTemporaryDirectory() as NSString).appendingPathComponent("dpl-valet-\(UUID().uuidString).json")
        do { try data.write(to: URL(fileURLWithPath: tmp)) } catch { return 0 }
        _ = await background { try cli.runRaw(["valet", "import", "--manifest", tmp]) }
        try? FileManager.default.removeItem(atPath: tmp)
        await loadLocal()
        return parked.count + links.count
    }

    /// Remove a selected set of sites in one bulk operation (reverse of import).
    @discardableResult
    func valetRemove(parked: [String], linked: [ValetSite]) async -> Int {
        let cli = self.cli
        let links = linked.map { ["name": $0.name, "path": $0.path] }
        let manifest: [String: Any] = ["parked": parked, "links": links]
        guard let data = try? JSONSerialization.data(withJSONObject: manifest) else { return 0 }
        let tmp = (NSTemporaryDirectory() as NSString).appendingPathComponent("dpl-valet-\(UUID().uuidString).json")
        do { try data.write(to: URL(fileURLWithPath: tmp)) } catch { return 0 }
        _ = await background { try cli.runRaw(["valet", "remove", "--manifest", tmp]) }
        try? FileManager.default.removeItem(atPath: tmp)
        await loadLocal()
        return parked.count + links.count
    }

    /// Remove a set of linked sites by name in one bulk pass (one reconcile).
    @discardableResult
    func removeLocalSites(_ sites: [(name: String, path: String)]) async -> Int {
        let cli = self.cli
        let links = sites.map { ["name": $0.name, "path": $0.path] }
        let manifest: [String: Any] = ["parked": [], "links": links]
        guard let data = try? JSONSerialization.data(withJSONObject: manifest) else { return 0 }
        let tmp = (NSTemporaryDirectory() as NSString).appendingPathComponent("dpl-cleanup-\(UUID().uuidString).json")
        do { try data.write(to: URL(fileURLWithPath: tmp)) } catch { return 0 }
        _ = await background { try cli.runRaw(["valet", "remove", "--manifest", tmp]) }
        try? FileManager.default.removeItem(atPath: tmp)
        await loadLocal()
        return sites.count
    }

    /// Ensure a TLD exists and is the primary domain.
    func adoptTld(_ tld: String) async {
        if !tlds.contains(tld) { await addTld(tld) }
        await setPrimaryTld(tld)
    }

    /// Switch a site's runtime (fpm or an already-installed Octane server).
    func setRuntime(site: String, runtime: String) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["runtime", site, runtime]) }
        await loadLocal()
    }

    /// Install Laravel Octane in a site and switch to it — runs composer +
    /// artisan in Terminal (it modifies the project, may prompt).
    func runOctaneSetupInTerminal(site: String, server: String) {
        guard let dpl = try? cli.resolveBinary() else { return }
        runInTerminal("\(dpl) octane install \(site) --server \(server)")
    }

    /// Reload a site's Octane workers so they pick up the code on disk. The
    /// listener stays up, so the site keeps answering throughout.
    func reloadOctane(site: String) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["octane", "reload", site]) }
        await loadLocal()
    }

    /// Restart a site's Octane server outright — the hammer for when reloading
    /// the workers isn't enough.
    func restartOctane(site: String) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["octane", "restart", site]) }
        await loadLocal()
    }

    /// Turn reload-on-save on or off for a site's Octane server.
    func setOctaneWatch(site: String, on: Bool) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["octane", "watch", site, on ? "on" : "off"]) }
        await loadLocal()
    }

    func unlinkLocal(name: String) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["unlink", name]) }
        await loadLocal()
    }

    /// Point a linked site at a different directory. The site keeps its name and
    /// every setting — this is for projects that moved on disk, not a re-link.
    func relinkLocal(name: String, path: String) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["relink", name, path]) }
        await loadLocal()
    }

    /// Proxy a `.test` host to another local service.
    func proxyLocal(name: String, target: String) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["proxy", name, target]) }
        await loadLocal()
    }

    func unproxyLocal(name: String) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["unproxy", name]) }
        await loadLocal()
    }

    func setLocalSecure(name: String, secure: Bool) async {
        let cli = self.cli
        _ = await background { try cli.runRaw([secure ? "secure" : "unsecure", name]) }
        await loadLocal()
    }

    /// Toggle the SPX profiler for a site. Enabling installs SPX for the site's
    /// PHP if needed, which can take a Homebrew minute — hence `background`.
    func setProfile(name: String, on: Bool) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["profile", on ? "on" : "off", name]) }
        await loadLocal()
    }

    func loadLocal() async {
        let cli = self.cli
        if let r = await background({ try cli.rows(["sites"]) }) { localSites = r }
    }

    /// The Node version manager dpl detected (`fnm`/`nvm`), or nil if none — for
    /// the Node panel's "who does the switching" hint. Per-site pins ride in
    /// `localSites`.
    @Published var nodeManager: String?

    func loadNodeManager() async {
        let cli = self.cli
        nodeManager = await backgroundQuiet { try cli.object(["node"]).first(["manager"]) } ?? nil
    }

    /// Pin a Node version for a site (writes its `.nvmrc`).
    func setNodeVersion(name: String, version: String) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["node", "use", version, name]) }
        await loadLocal()
    }

    /// Derive a site's Node version from package.json and pin it.
    func detectNodeVersion(name: String) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["node", "detect", name]) }
        await loadLocal()
    }

    /// A site's tags, read off a `dpl sites` row. `nonisolated` because it's a
    /// pure read of the row — grouping calls it from the view's sync layout path.
    nonisolated static func tags(of row: Row) -> [String] {
        row.dig("tags")?.arrayValue?.compactMap { $0.stringValue } ?? []
    }

    /// Replace a site's tags. The daemon normalises them, so what comes back may
    /// differ from what was typed — reload rather than assume.
    func setTags(site: String, tags: [String]) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["tags", "set", site] + tags) }
        await loadLocal()
    }

    /// Turn a site's supervised dev server on with `script`, or off with nil.
    /// The daemon owns the process from here — it restarts it if it dies and it
    /// outlives the app, so this is a config change, not a launch.
    func setDevServer(site: String, script: String?) async {
        let cli = self.cli
        let args = script.map { ["dev", "on", site, "--script", $0] } ?? ["dev", "off", site]
        _ = await background { try cli.runRaw(args) }
        await loadLocal()
    }

    /// Bounce a site's dev server, clearing any give-up state.
    func restartDevServer(site: String) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["dev", "restart", site]) }
        await loadLocal()
    }

    /// A site's package.json scripts, for the Run menu. Quiet on failure — a
    /// site with no readable scripts simply offers none, which is not an error
    /// worth a banner.
    func nodeScripts(site: String) async -> [String] {
        let cli = self.cli
        let row = await backgroundQuiet { try cli.object(["node", "scripts", site]) }
        guard let entry = row?.dig("sites")?.arrayValue?.first?.objectValue else { return [] }
        return entry["scripts"]?.arrayValue?.compactMap { $0.stringValue } ?? []
    }

    // MARK: Branch-aware databases

    /// One row of `dpl db branches <site>`: a branch, its database size, and
    /// whether it's the one currently live in the base database.
    struct DbBranch: Identifiable, Equatable {
        let branch: String
        let size: String
        let live: Bool
        var id: String { branch }
    }

    /// Attach branch-aware databases to a site (DB name read from its .env).
    func attachBranchDb(name: String) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["db", "attach", name]) }
        await loadLocal()
    }

    /// Detach — parked `<db>@<branch>` databases are kept.
    func detachBranchDb(name: String) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["db", "detach", name]) }
        await loadLocal()
    }

    /// Drop one parked branch database.
    func dropDbBranch(name: String, branch: String) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["db", "drop-branch", name, branch]) }
    }

    /// Live + parked branches for a site, from `dpl db branches` (plain lines:
    /// `* <branch>\t<size>\tlive in ...` / `  <branch>\t<size>\tparked`).
    func dbBranches(name: String) async -> [DbBranch] {
        let cli = self.cli
        guard let data = await backgroundQuiet({ try cli.runRaw(["db", "branches", name]) }) else { return [] }
        return String(decoding: data, as: UTF8.self)
            .split(separator: "\n")
            .compactMap { line in
                let live = line.hasPrefix("*")
                let parts = line.dropFirst(live ? 1 : 0)
                    .trimmingCharacters(in: .whitespaces)
                    .split(separator: "\t", omittingEmptySubsequences: false)
                guard let branch = parts.first, !branch.isEmpty else { return nil }
                let size = parts.count > 1 ? String(parts[1]) : ""
                return DbBranch(branch: String(branch), size: size, live: live)
            }
    }

    // MARK: Site project notes

    /// Notes live inside the project itself — `<project>/.dpl/project.json` — so
    /// they travel with the repo and can be committed and shared, rather than
    /// being stranded in this Mac's preferences. `path` is the site's project dir.
    private func projectFile(_ path: String) -> URL {
        URL(fileURLWithPath: path, isDirectory: true)
            .appendingPathComponent(".dpl", isDirectory: true)
            .appendingPathComponent("project.json")
    }

    /// The user's notes for the project at `path` — its summary, to-dos, and
    /// external URLs. A missing or unreadable file decodes to an empty project.
    func project(atPath path: String) -> SiteProject {
        guard !path.isEmpty,
              let data = try? Data(contentsOf: projectFile(path)),
              let project = try? JSONDecoder().decode(SiteProject.self, from: data)
        else { return SiteProject() }
        return project
    }

    /// Persist a project's notes, pretty-printed so the file reads well in a
    /// diff. An empty project deletes the file (and prunes an empty `.dpl` dir)
    /// rather than committing a blank record.
    func setProject(_ project: SiteProject, atPath path: String) {
        guard !path.isEmpty else { return }
        let file = projectFile(path)
        let fm = FileManager.default
        if project.isEmpty {
            try? fm.removeItem(at: file)
            let dir = file.deletingLastPathComponent()
            if let contents = try? fm.contentsOfDirectory(atPath: dir.path), contents.isEmpty {
                try? fm.removeItem(at: dir)
            }
            return
        }
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        guard let data = try? encoder.encode(project) else { return }
        try? fm.createDirectory(at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
        try? data.write(to: file, options: .atomic)
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
    @Published var dumpSiteFilter: String? = nil { didSet { if oldValue != dumpSiteFilter { recomputeDerived() } } }
    @Published var dumpScreenFilter: String? = nil { didSet { if oldValue != dumpScreenFilter { recomputeDerived() } } }
    // nil=all; else a category: dump/query/log/mail/job/view/cache/http/…
    @Published var dumpTypeFilter: String? = nil { didSet { if oldValue != dumpTypeFilter { recomputeDerived() } } }
    // nil=all; else one request id — set from a row's "Only this request".
    @Published var dumpRequestFilter: String? = nil { didSet { if oldValue != dumpRequestFilter { recomputeDerived() } } }
    @Published var dumpSearch: String = "" { didSet { if oldValue != dumpSearch { scheduleSearchRefilter() } } }

    /// Derived views of `dumps`, recomputed only when the buffer or a filter
    /// changes (see `recomputeDerived`) — never on every SwiftUI render, which
    /// under a dump-heavy request meant re-filtering a 2000-entry buffer several
    /// times per frame. The list binds to `filteredDumps`; the pickers read the
    /// site/screen lists and the per-category counts.
    @Published private(set) var filteredDumps: [DumpEntry] = []
    @Published private(set) var dumpCounts: [String: Int] = [:]
    @Published private(set) var dumpSites: [String] = []
    @Published private(set) var dumpScreens: [String] = []

    /// Editor for jump-to-source: "code" | "cursor" | "phpstorm" | "subl" | "system".
    @AppStorage("dumpsEditor") var dumpsEditor: String = "code"
    /// Follow the tail of the stream. Turned off automatically when the user
    /// scrolls away from the bottom, so a burst of dumps can't yank the view.
    @AppStorage("dumpsAutoscroll") var dumpsAutoscroll: Bool = true

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
                              var entry = try? JSONDecoder().decode(DumpEntry.self, from: data)
                        else { continue }
                        entry.raw = line
                        entry.buildSearchIndex()
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
        // Backfill can resend ids we already have; de-dupe. A set, not a linear
        // scan — a single request can emit hundreds of query dumps at once.
        guard dumpIDs.insert(entry.id).inserted else { return }
        pendingDumps.append(entry)
        // A paused breakpoint is holding a live request open, so it can't wait
        // for the coalesce tick — surface it (and the batch so far) at once.
        if entry.pause == true, entry.token != nil {
            pausedDump = entry
            flushPendingDumps()
        } else {
            scheduleFlush()
        }
    }

    private var dumpIDs = Set<Int>()
    /// Dumps received but not yet published. One request can emit hundreds of
    /// entries; appending them to `@Published dumps` one at a time would
    /// re-render and re-filter the whole list per entry. Buffer here and flush
    /// the burst as a single mutation instead.
    private var pendingDumps: [DumpEntry] = []
    private var flushScheduled = false

    /// Schedule a coalesced flush of `pendingDumps` on the next tick.
    private func scheduleFlush() {
        guard !flushScheduled else { return }
        flushScheduled = true
        Task { @MainActor in
            // 50ms is imperceptible on a live tail but collapses a burst of
            // hundreds of appends into one published update.
            try? await Task.sleep(nanoseconds: 50_000_000)
            flushPendingDumps()
        }
    }

    /// Move the buffered dumps into the published list in one mutation, trim to
    /// `maxDumps`, and refresh the derived collections once.
    private func flushPendingDumps() {
        flushScheduled = false
        guard !pendingDumps.isEmpty else { return }
        dumps.append(contentsOf: pendingDumps)
        pendingDumps.removeAll(keepingCapacity: true)
        let overflow = dumps.count - maxDumps
        if overflow > 0 {
            for d in dumps[..<overflow] { dumpIDs.remove(d.id) }
            dumps.removeFirst(overflow)
        }
        recomputeDerived()
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
        dumpIDs.removeAll()
        pendingDumps.removeAll()
        recomputeDerived()
        var req = URLRequest(url: URL(string: "http://127.0.0.1:\(dumpsPort)/dumps/clear")!)
        req.httpMethod = "POST"
        _ = try? await URLSession.shared.data(for: req)
    }

    /// Recompute every derived collection in a single pass over the buffer —
    /// called when `dumps` or a filter changes, not on every render. The
    /// site/screen lists reflect all dumps; the counts ignore the *type* filter
    /// (so the picker shows how many of each kind survive the other filters);
    /// `filteredDumps` applies every filter. The search text is lowercased once
    /// here rather than once per entry.
    private func recomputeDerived() {
        let search = dumpSearch.lowercased()
        var sites = Set<String>()
        var screens = Set<String>()
        var counts: [String: Int] = [:]
        var filtered: [DumpEntry] = []
        filtered.reserveCapacity(dumps.count)
        for d in dumps {
            if let s = d.site { sites.insert(s) }
            if let s = d.screen { screens.insert(s) }
            let passesNonType = (dumpSiteFilter == nil || d.site == dumpSiteFilter)
                && (dumpScreenFilter == nil || d.screen == dumpScreenFilter)
                && (dumpRequestFilter == nil || d.request == dumpRequestFilter)
                && (search.isEmpty || d.searchIndex.contains(search))
            guard passesNonType else { continue }
            counts[d.category, default: 0] += 1
            if dumpTypeFilter == nil || d.category == dumpTypeFilter {
                filtered.append(d)
            }
        }
        dumpSites = sites.sorted()
        dumpScreens = screens.sorted()
        dumpCounts = counts
        filteredDumps = filtered
    }

    /// Debounce search-driven refilters — the field fires on every keystroke,
    /// and filtering the whole buffer each time is wasteful (Mail debounces the
    /// same way). Discrete filter picks refilter immediately via their `didSet`.
    private var searchRefilter: Task<Void, Never>?
    private func scheduleSearchRefilter() {
        searchRefilter?.cancel()
        searchRefilter = Task { @MainActor in
            try? await Task.sleep(nanoseconds: 200_000_000) // 200ms after the last keystroke
            guard !Task.isCancelled else { return }
            recomputeDerived()
        }
    }

    /// Remove a single entry from the local list. The daemon's ring buffer still
    /// holds it, so this is a view-level dismiss, not a delete.
    func hideDump(_ id: Int) {
        dumps.removeAll { $0.id == id }
        dumpIDs.remove(id)
        recomputeDerived()
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
        if site == nil { await loadPhpVersions() } // menu-bar default changed
    }

    /// Mailbox counts from `dpl mail mailboxes`, for the switcher.
    @Published var mailboxes: [Row] = []
    /// Which mailbox the list is showing; `nil` means all of them.
    @Published var mailboxFilter: String? = nil

    /// The mailbox name the sink uses for mail that arrived without an SMTP
    /// username. Matches `UNATTRIBUTED` in the CLI.
    static let unattributedMailbox = "-"

    /// Display name for a mailbox: sites use their `MAIL_USERNAME` verbatim.
    static func mailboxLabel(_ name: String) -> String {
        name == unattributedMailbox ? "No username" : name
    }

    /// Free-text query across subject, sender, recipient and body.
    @Published var mailSearch = ""

    func loadMail() async {
        let cli = self.cli
        let filter = mailboxFilter
        let query = mailSearch.trimmingCharacters(in: .whitespaces)
        var args = ["mail", "list"]
        if let filter { args += ["--mailbox", filter] }
        if !query.isEmpty { args += ["--search", query] }
        if let r = await background({ try cli.rows(args) }) { mailMessages = r }
        // Counts always cover every mailbox, so the switcher can show the ones
        // the current filter is hiding.
        if let m = await background({ try cli.rows(["mail", "mailboxes"]) }) { mailboxes = m }
    }

    func selectMailbox(_ name: String?) async {
        mailboxFilter = name
        await loadMail()
    }

    /// Clear the visible mailbox, or everything when no filter is active.
    func clearMail() async {
        let cli = self.cli
        let filter = mailboxFilter
        var args = ["mail", "clear"]
        if let filter { args += ["--mailbox", filter] }
        _ = await background { try cli.runRaw(args) }
        // The mailbox may no longer exist once emptied.
        if filter != nil { mailboxFilter = nil }
        await loadMail()
    }

    /// Raw source of one captured message (the Raw tab).
    func mailBody(_ id: String) async -> String {
        let cli = self.cli
        let data = await background { try cli.runRaw(["mail", "show", id]) }
        return data.map { String(decoding: $0, as: UTF8.self) } ?? ""
    }

    /// Everything about one message in a single call: bodies, headers, links,
    /// attachment metadata and the remote-resource count.
    func mailMessage(_ id: String) async -> MailMessage? {
        let cli = self.cli
        guard let value = await background({ try cli.json(["mail", "show", id]) }) else { return nil }
        return MailMessage(value)
    }

    /// Save an attachment, asking where to put it.
    func saveAttachment(messageId: String, attachment: MailAttachment) async {
        let panel = NSSavePanel()
        panel.nameFieldStringValue = attachment.name
        panel.canCreateDirectories = true
        guard panel.runModal() == .OK, let url = panel.url else { return }

        let cli = self.cli
        _ = await background {
            try cli.runRaw(["mail", "save", messageId, String(attachment.index), "--out", url.path])
        }
    }

    /// Drop a sample message into the sink — proves the wiring and gives the
    /// viewer something to render.
    func sendTestMail(mailbox: String?, html: Bool) async {
        let cli = self.cli
        var args = ["mail", "send"]
        if html { args.append("--html") }
        if let mailbox, !mailbox.isEmpty { args += ["--mailbox", mailbox] }
        _ = await background { try cli.runRaw(args) }
        await loadMail()
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

    /// Make a TLD the primary (canonical) domain for all sites, then refresh so
    /// site hosts/URLs update.
    func setPrimaryTld(_ name: String) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["tld", "primary", name]) }
        await loadTlds()
        await loadLocal()
    }

    /// Distinct PHP versions available (for the per-site picker).
    var availablePhpVersions: [String] {
        phpVersions.compactMap { $0.first(["version"]) }
    }

    /// The current default PHP version (shown in the menu bar).
    var defaultPhp: String? {
        phpVersions.first { $0.dig("default") == .bool(true) }?.first(["version"])
    }

    /// Refresh the data the menu bar shows (php/sites/services), independent of
    /// the main window being open.
    func refreshMenuData() async {
        await loadPhpVersions()
        await loadPhpConfig()
        await loadLocal()
        if let r = await background({ try self.cli.rows(["services"]) }) { dbServices = r }
    }

    /// Open a site's folder in Finder.
    func openFolder(_ path: String) {
        NSWorkspace.shared.open(URL(fileURLWithPath: path))
    }

    // MARK: PHP configuration (PhpMon-style)

    /// Full version (e.g. 8.5.8) of the active PHP, for the menu header.
    @Published var phpFull: String?
    /// Loaded php.ini path.
    @Published var phpIni: String?
    /// memory_limit / post_max_size / upload_max_filesize.
    @Published var phpMemory: [(String, String)] = []
    /// Extensions in the active PHP's conf.d (enabled + disabled).
    @Published var phpExtensions: [PhpExt] = []

    struct PhpExt: Identifiable, Hashable {
        let id = UUID()
        let name: String
        let enabled: Bool
        let path: String
    }

    /// Binary of the current default PHP.
    var defaultPhpBinary: String? {
        phpVersions.first { $0.dig("default") == .bool(true) }?.first(["binary"])
    }

    /// Load the active PHP's config (version, ini, memory, extensions).
    func loadPhpConfig() async {
        guard let bin = defaultPhpBinary else { return }
        // `-d display_errors=stderr` keeps a broken extension's startup warning
        // out of the stdout we parse (CLI defaults it to stdout on this box).
        let quiet = ["-d", "display_errors=stderr"]
        phpFull = (await backgroundQuiet { runProcess(bin, quiet + ["-r", "echo PHP_VERSION;"]) })?
            .trimmingCharacters(in: .whitespacesAndNewlines)

        let ini = await backgroundQuiet { runProcess(bin, quiet + ["--ini"]) } ?? ""
        phpIni = value(in: ini, after: "Loaded Configuration File:")
        let scanDir = value(in: ini, after: "Scan for additional .ini files in:")

        let mem = (await backgroundQuiet {
            runProcess(bin, quiet + ["-r", "echo ini_get('memory_limit'),'|',ini_get('post_max_size'),'|',ini_get('upload_max_filesize');"])
        }) ?? ""
        let parts = mem.split(separator: "|", omittingEmptySubsequences: false).map(String.init)
        phpMemory = zip(["Memory limit", "Max post", "Max upload"], parts).map { ($0, $1) }

        if let dir = scanDir, !dir.isEmpty {
            phpExtensions = await backgroundQuiet { scanExtensions(dir) } ?? []
        }
        await loadXdebug()
    }

    private func value(in text: String, after key: String) -> String? {
        for line in text.split(separator: "\n") {
            if line.contains(key) {
                // `php --ini` wraps paths in quotes — strip them.
                let v = line.replacingOccurrences(of: key, with: "")
                    .trimmingCharacters(in: CharacterSet(charactersIn: "\" \t"))
                return v == "(none)" || v.isEmpty ? nil : v
            }
        }
        return nil
    }

    /// Enable/disable an extension by toggling its .ini file, then restart fpm.
    func toggleExtension(_ ext: PhpExt) async {
        let from = ext.path
        let to = ext.enabled ? ext.path + ".disabled" : String(ext.path.dropLast(".disabled".count))
        _ = await backgroundQuiet { renameFile(from, to) }
        await restartDaemon()
        await loadPhpConfig()
    }

    /// Is Xdebug currently enabled?
    var xdebugExt: PhpExt? {
        phpExtensions.first { $0.name.lowercased().contains("xdebug") }
    }

    /// Open the active php.ini in the editor.
    func openPhpIni() {
        if let ini = phpIni { openFolderInEditor(ini) }
    }

    /// Render phpinfo() to a temp HTML page and open it in the browser.
    func showPhpInfo() {
        guard let bin = defaultPhpBinary else { return }
        Task {
            let text = await backgroundQuiet { runProcess(bin, ["-i"]) } ?? "phpinfo unavailable"
            let escaped = text.replacingOccurrences(of: "&", with: "&amp;")
                .replacingOccurrences(of: "<", with: "&lt;")
            let html = "<!doctype html><meta charset=utf-8><title>phpinfo</title><pre style='font:13px ui-monospace,monospace;padding:1rem'>\(escaped)</pre>"
            let url = FileManager.default.temporaryDirectory.appendingPathComponent("phpinfo.html")
            try? html.write(to: url, atomically: true, encoding: .utf8)
            NSWorkspace.shared.open(url)
        }
    }

    /// Run `composer global update` in Terminal.
    func updateComposerGlobal() {
        let script = "tell application \"Terminal\" to do script \"composer global update\"\ntell application \"Terminal\" to activate"
        runOsascript(script)
    }

    func openDplFolder() {
        if let home = ProcessInfo.processInfo.environment["HOME"] {
            NSWorkspace.shared.open(URL(fileURLWithPath: home + "/.dpl"))
        }
    }

    func openComposerFolder() {
        if let home = ProcessInfo.processInfo.environment["HOME"] {
            NSWorkspace.shared.open(URL(fileURLWithPath: home + "/.composer"))
        }
    }

    /// Open a Laravel Tinker REPL for a site in Terminal — Tinker is interactive,
    /// so it can't run headless inside the app.
    func openTinker(_ name: String) {
        guard let dpl = try? cli.resolveBinary() else { return }
        runInTerminal("'\(dpl)' tinker \(name)")
    }

    /// Run a shell command in a new Terminal window (for steps needing sudo or
    /// live output, e.g. `brew install php`).
    func runInTerminal(_ command: String) {
        let escaped = command.replacingOccurrences(of: "\"", with: "\\\"")
        runOsascript("tell application \"Terminal\" to do script \"\(escaped)\"\ntell application \"Terminal\" to activate")
    }

    private func runOsascript(_ script: String) {
        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: "/usr/bin/osascript")
        proc.arguments = ["-e", script]
        try? proc.run()
    }

    /// Open a folder in the configured editor.
    func openFolderInEditor(_ path: String) {
        switch dumpsEditor {
        case "code", "cursor", "subl":
            runEditor(dumpsEditor == "subl" ? "subl" : dumpsEditor, [path])
        case "phpstorm":
            if let url = URL(string: "phpstorm://open?file=\(path.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? path)") {
                NSWorkspace.shared.open(url)
            }
        default:
            NSWorkspace.shared.open(URL(fileURLWithPath: path))
        }
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
        await refreshCurrentSection(force: true)
    }

    /// Deploy a server-hosted site.
    func deploySite(_ id: String) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["dply", "sites:deploy", id]) }
        await refreshCurrentSection(force: true)
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

    // MARK: System / status actions

    /// The full `dpl doctor --json` health report — every probe the CLI runs.
    func doctorReport() async -> DoctorReport? {
        let cli = self.cli
        return await backgroundQuiet {
            try JSONDecoder().decode(DoctorReport.self, from: cli.runRaw(["doctor", "--json"]))
        }
    }

    /// One line per check, problems first — for the compact health lists on the
    /// Dashboard and Status panels.
    func doctor() async -> [String] {
        guard let report = await doctorReport() else {
            return ["✗ daemon: not running — start it or install autostart"]
        }
        return report.checks
            .enumerated()
            .sorted { a, b in
                let rank: (DoctorStatus) -> Int = { [.fail: 0, .warn: 1, .pass: 2, .info: 3][$0] ?? 4 }
                let (ra, rb) = (rank(a.element.status), rank(b.element.status))
                return ra == rb ? a.offset < b.offset : ra < rb
            }
            .map { "\($0.element.status.mark) \($0.element.title): \($0.element.detail)" }
    }

    /// The dply server-hosted sites, for the parity picker.
    func loadRemoteSites() async {
        guard dplyEnabled, sites.isEmpty else { return }
        let cli = self.cli
        if let r = await backgroundQuiet({ try cli.rows(["dply", "sites:list"]) }) { sites = r }
    }

    /// Diff a local site against the dply site it deploys to. Reads env key
    /// names on both sides — never their values.
    func parity(site: String, remote: String?) async -> DoctorReport? {
        let cli = self.cli
        var args = ["parity", site]
        if let remote, !remote.isEmpty, remote != site { args += ["--remote", remote] }
        // Surface failures (not logged in, unknown remote site) — the CLI's
        // stderr explains them better than a silent empty sheet.
        return await background {
            try JSONDecoder().decode(DoctorReport.self, from: cli.runRaw(args + ["--json"]))
        }
    }

    /// Re-run every probe and publish the result to the Doctor page.
    private var doctorRefreshedAt: Date?
    /// How long a doctor report is treated as current. `dpl doctor` runs every
    /// health probe (the most expensive command), and the Doctor/System pages
    /// re-run it on every `didBecomeActive` — so ⌘-tabbing in and out re-probed
    /// the machine each time. Throttle those; explicit runs still `force`.
    private let doctorStaleAfter: TimeInterval = 30

    func refreshDoctor(force: Bool = false) async {
        if !force, let at = doctorRefreshedAt,
           Date().timeIntervalSince(at) < doctorStaleAfter {
            return
        }
        doctorRunning = true
        defer {
            doctorRunning = false
            doctorRefreshedAt = Date()
        }
        doctorHealth = await doctorReport()
    }

    /// Apply a check's fix.
    ///
    /// `sudo` fixes run behind the system authorization sheet, in the app. Fixes
    /// that aren't `dpl` at all (`brew install …`) still go to Terminal, where the
    /// user can watch a long, chatty command and answer its questions.
    func runFix(_ fix: DoctorFix) async {
        let parts = fix.command.split(separator: " ").map(String.init)
        guard !parts.isEmpty else { return }

        // A command with a `<placeholder>` must never run as typed — it would
        // write the literal placeholder (e.g. into a production env var).
        if fix.needsEditing {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(fix.command, forType: .string)
            return
        }

        if fix.sudo {
            await runPrivilegedFix(parts)
            return
        }
        if parts[0] != "dpl" {
            // A non-dpl fix (`brew install composer`) needs its own program present.
            // Opening Terminal to `brew: command not found` teaches the user nothing;
            // name what's actually missing. Absolute paths — the Homebrew installer's
            // `/bin/bash` — are taken as given, since that fix is what provides brew.
            let program = parts[0]
            if !program.hasPrefix("/"), await backgroundQuiet({ whichExists(program) }) != true {
                lastError = "`\(program)` isn't installed, so this fix can't run yet — install \(program) first."
                return
            }
            guard let dpl = try? cli.resolveBinary() else { return }
            let command = parts.map { $0 == "dpl" ? dpl : $0 }.joined(separator: " ")
            runInTerminal(command)
            return
        }
        let cli = self.cli
        let args = Array(parts.dropFirst())
        _ = await background { try cli.runRaw(args) }
        await refreshDoctor(force: true)
    }

    /// Run a `sudo …` doctor fix as root, via the authorization sheet.
    ///
    /// The command arrives as `sudo dpl setup`. We drop the `sudo` (the sheet is
    /// what grants root), expand `dpl` to the binary this GUI drives, and — because
    /// the sheet makes us root outright, with no `SUDO_USER` to fall back on — tell
    /// `setup` which human it is configuring. Without that it would install the
    /// daemon as root and write root-owned files into the user's `~/.dpl`.
    private func runPrivilegedFix(_ parts: [String]) async {
        guard let dpl = try? cli.resolveBinary() else { return }

        var args = Array(parts.dropFirst())                     // drop "sudo"
        args = args.map { $0 == "dpl" ? dpl : $0 }
        if args.first == dpl, args.dropFirst().first == "setup" {
            args += ["--as-user", NSUserName()]
        }

        let command = args.map(PrivilegedTask.shellQuote).joined(separator: " ")
        do {
            try PrivilegedTask.run(command)
        } catch let failure as PrivilegedTask.Failure {
            // Cancelling the sheet is a choice, not a failure to report.
            if !failure.cancelled { lastError = failure.errorDescription }
        } catch {
            lastError = error.localizedDescription
        }
        await refreshDoctor(force: true)
    }

    /// Recent request-log text for a local site.
    func siteLogs(_ name: String, lines: Int = 200) async -> String {
        let cli = self.cli
        return await backgroundQuiet({
            String(decoding: try cli.runRaw(["logs", name, "-n", String(lines)]), as: UTF8.self)
        }) ?? "(no log yet — the site may not have been served)"
    }

    /// Manage the login autostart service.
    func daemonAction(_ action: String) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["daemon", action]) }
    }

    /// Is the autostart LaunchAgent installed?
    func daemonStatus() async -> Bool {
        let cli = self.cli
        let out = await backgroundQuiet({ String(decoding: try cli.runRaw(["daemon", "status"]), as: UTF8.self) })
        return out?.contains("loaded") ?? false
    }

    func restartDaemon() async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["restart"]) }
    }

    /// Start the dpld daemon.
    func startDaemon() async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["start"]) }
    }

    /// Stop the dpld daemon.
    func stopDaemon() async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["stop"]) }
    }

    /// Hot-reload config + reconcile backends without restarting the daemon.
    func reloadDaemon() async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["reload"]) }
    }

    /// Hard-reset all backends (kill stale php-fpm/Octane + rebuild).
    func repairBackends() async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["repair"]) }
    }

    // MARK: System load

    /// Start polling the system load average every few seconds.
    func startLoadMonitor() {
        refreshLoad()
        loadTimer?.invalidate()
        loadTimer = Timer.scheduledTimer(withTimeInterval: 4, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.refreshLoad() }
        }
    }

    func refreshLoad() {
        var loads = [Double](repeating: 0, count: 3)
        getloadavg(&loads, 3)
        loadAvg = loads[0]
    }

    /// Total physical memory, GB.
    var totalMemoryGB: Double { Double(ProcessInfo.processInfo.physicalMemory) / 1_073_741_824 }

    /// Every supervised dev server, from `dpl dev --json`.
    func devServers() async -> [DevServer] {
        let cli = self.cli
        let row = await backgroundQuiet { try cli.object(["dev"]) }
        guard let list = row?.dig("servers")?.arrayValue else { return [] }
        return list.compactMap { entry in
            guard let o = entry.objectValue, let site = o["site"]?.stringValue else { return nil }
            return DevServer(
                site: site,
                script: o["script"]?.stringValue ?? "dev",
                agent: o["agent"]?.stringValue ?? "npm",
                running: o["running"]?.boolValue ?? false,
                pgid: o["pgid"]?.intValue,
                port: o["port"]?.intValue,
                detail: o["detail"]?.stringValue
            )
        }
    }

    /// Every supervised Octane server and what it's doing — the other half of
    /// the workers dpl owns, and the only ones where "reload" means something.
    func octaneServers() async -> [OctaneServer] {
        let cli = self.cli
        let row = await backgroundQuiet { try cli.object(["octane"]) }
        guard let list = row?.dig("servers")?.arrayValue else { return [] }
        return list.compactMap { entry in
            guard let o = entry.objectValue, let site = o["site"]?.stringValue else { return nil }
            return OctaneServer(
                site: site,
                runtime: o["runtime"]?.stringValue ?? "octane",
                port: o["port"]?.intValue,
                running: o["running"]?.boolValue ?? false,
                watch: o["watch"]?.boolValue ?? false,
                reloads: o["reloads"]?.intValue ?? 0,
                detail: o["detail"]?.stringValue
            )
        }
    }

    /// Detect running Laravel/dev workers (Horizon, queues, Reverb, scheduler,
    /// Stripe listener, Vite) grouped by kind, labeled by their project folder.
    ///
    /// `supervised` carries the process groups dpld owns. Their children show up
    /// in `ps` like any other worker, and listing them here as well would put the
    /// same Vite in two places — one row you can stop and one you can't.
    func detectWorkers(excluding supervised: Set<Int> = []) async -> [WorkerGroup] {
        // pgid, not just pid: a supervised dev server's *children* are what `ps`
        // finds, and they share the group rather than the pid.
        let out = await backgroundQuiet { runProcess("/bin/ps", ["-Ao", "pid=,pgid=,command="]) } ?? ""
        var groups: [String: [WorkerItem]] = [:]
        for raw in out.split(separator: "\n") {
            let fields = raw.trimmingCharacters(in: .whitespaces)
                .split(separator: " ", maxSplits: 2, omittingEmptySubsequences: true)
            guard fields.count == 3, let pid = Int(fields[0]), let pgid = Int(fields[1]) else { continue }
            if supervised.contains(pgid) { continue }
            let cmd = String(fields[2])
            let low = cmd.lowercased()
            guard low.contains("artisan") || low.contains("stripe") || low.contains("vite") else { continue }
            let kind: String? =
                low.contains("horizon") ? "Horizon" :
                (low.contains("queue:work") || low.contains("queue:listen")) ? "Queue" :
                low.contains("reverb") ? "Reverb" :
                (low.contains("schedule:work") || low.contains("schedule:run")) ? "Scheduler" :
                (low.contains("stripe listen") || low.contains("stripe-cli")) ? "Stripe" :
                low.contains("vite") ? "Vite" : nil
            guard let k = kind else { continue }
            // Label by the worker's queue/name (the actual worker identity), not
            // its project folder — e.g. "default", "mail", "dply-provision".
            let label = workerLabel(kind: k, command: cmd)
            groups[k, default: []].append(WorkerItem(pid: pid, name: label))
        }
        let order = ["Horizon", "Queue", "Reverb", "Scheduler", "Stripe", "Vite"]
        return order.compactMap { k in
            groups[k].map { WorkerGroup(kind: k, items: $0.sorted { $0.name < $1.name }) }
        }
    }

    /// Pull a worker's identity out of its command line: the `--queue=` value
    /// (falling back to `--name`/`--supervisor`), else the worker kind.
    private func workerLabel(kind: String, command: String) -> String {
        for key in ["--queue=", "--name=", "--supervisor="] {
            if let r = command.range(of: key) {
                let val = command[r.upperBound...].prefix { !$0.isWhitespace }
                    .split(separator: ",").first.map(String.init) ?? ""
                if !val.isEmpty {
                    // strip a "host:supervisor-1" prefix down to the tail.
                    return val.split(separator: ":").last.map(String.init) ?? val
                }
            }
        }
        return kind.lowercased()
    }

    /// Top processes by memory (name, MB, cpu%), for the dashboard Resources card.
    func topProcesses(limit: Int = 8) async -> [(name: String, mb: Int, cpu: Double)] {
        let out = await backgroundQuiet {
            runProcess("/bin/ps", ["-Ao", "rss=,pcpu=,comm=", "-m"])
        } ?? ""
        var rows: [(String, Int, Double)] = []
        for line in out.split(separator: "\n").prefix(limit) {
            let parts = line.split(separator: " ", maxSplits: 2, omittingEmptySubsequences: true)
            guard parts.count == 3, let rss = Int(parts[0]), let cpu = Double(parts[1]) else { continue }
            let name = (String(parts[2]) as NSString).lastPathComponent
            rows.append((name, rss / 1024, cpu))
        }
        return rows
    }

    /// Load per core — >1 means the CPU is oversubscribed.
    var loadRatio: Double { loadAvg / Double(cpuCount) }

    /// 0 = fine, 1 = busy (serving may slow), 2 = saturated (serving will hang).
    var loadSeverity: Int {
        if loadRatio >= 1.5 { return 2 }
        if loadRatio >= 0.9 { return 1 }
        return 0
    }

    /// Open Terminal to run the one-time privileged setup (needs sudo).
    /// Run `dpl setup` as root behind the system authorization sheet.
    ///
    /// Returns true when setup completed. Cancelling the sheet returns false and
    /// reports nothing — the user said no, which is an answer, not an error.
    ///
    /// `--as-user` is not optional here: the sheet grants root with no `SUDO_USER`,
    /// and `dpl setup` refuses to install a daemon that would run every site's PHP
    /// as root.
    @discardableResult
    func runSetup() async -> Bool {
        guard let dpl = try? cli.resolveBinary() else { return false }
        let command = [dpl, "setup", "--as-user", NSUserName()]
            .map(PrivilegedTask.shellQuote)
            .joined(separator: " ")
        do {
            try PrivilegedTask.run(command)
            await refreshDoctor(force: true)
            return true
        } catch let failure as PrivilegedTask.Failure {
            if !failure.cancelled { lastError = failure.errorDescription }
            return false
        } catch {
            lastError = error.localizedDescription
            return false
        }
    }

    /// Run `dpl takeover` in Terminal — stops Valet/Apache and gives dpl :80/:443.
    func runTakeoverInTerminal() {
        guard let dpl = try? cli.resolveBinary() else { return }
        runInTerminal("\(dpl) takeover")
    }

    /// Run `dpl untakeover` in Terminal — restores Valet and its ports.
    func runUntakeoverInTerminal() {
        guard let dpl = try? cli.resolveBinary() else { return }
        runInTerminal("\(dpl) untakeover")
    }

    /// Switch .test resolution mode (`hosts` keeps Private Relay on) in Terminal.
    func runResolutionInTerminal(_ mode: String) {
        guard let dpl = try? cli.resolveBinary() else { return }
        runInTerminal("\(dpl) resolution \(mode)")
    }

    /// Current resolution mode ("resolver" | "hosts"), for the settings UI.
    func loadResolutionMode() async -> String {
        let cli = self.cli
        let out = await background { try cli.runRaw(["resolution"]) }
        guard let data = out, let text = String(data: data, encoding: .utf8) else { return "resolver" }
        return text.contains("hosts") && text.contains("Resolution mode: hosts") ? "hosts" : "resolver"
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
