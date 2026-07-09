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

/// Run an executable and capture stdout (off the main actor). Used to query
/// the active PHP binary for its config.
func runProcess(_ exe: String, _ args: [String]) -> String {
    let proc = Process()
    proc.executableURL = URL(fileURLWithPath: exe)
    proc.arguments = args
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
    case doctor = "Doctor"
    case settings = "Settings"
    case edgeSites = "Edge Sites"
    case sites = "Server Sites"
    case servers = "Servers"
    case account = "Account"

    var id: String { rawValue }

    /// The local-tooling surfaces (no dply login required).
    var isLocal: Bool {
        switch self {
        case .dashboard, .local, .services, .mail, .dumps, .php, .extensions,
             .status, .doctor, .settings:
            return true
        default:
            return false
        }
    }
    var isDply: Bool { !isLocal }

    /// Surfaces that are a single page rather than a list + detail, so they get
    /// the whole window instead of being squeezed into the middle column.
    var isFullWidth: Bool {
        switch self {
        case .dashboard, .php, .extensions, .status, .doctor, .settings: return true
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
        case .doctor: return "stethoscope"
        case .settings: return "gearshape"
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
        await refreshCurrentSection()
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

    func refreshCurrentSection() async {
        let cli = self.cli
        isLoading = true
        defer { isLoading = false }
        switch section {
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
        case .status, .doctor:
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
        runInTerminal("\(dpl) octane \(site) --server \(server)")
    }

    func unlinkLocal(name: String) async {
        let cli = self.cli
        _ = await background { try cli.runRaw(["unlink", name]) }
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
    func refreshDoctor() async {
        doctorRunning = true
        defer { doctorRunning = false }
        doctorHealth = await doctorReport()
    }

    /// Apply a check's fix. `sudo` commands (and anything that isn't `dpl`, like
    /// `brew install`) go to Terminal so the user sees the prompt and progress;
    /// plain `dpl` commands run headless and refresh in place.
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

        if fix.sudo || parts[0] != "dpl" {
            guard let dpl = try? cli.resolveBinary() else { return }
            // Expand the bare `dpl` token to the binary this GUI actually drives.
            let command = parts
                .map { $0 == "dpl" ? dpl : $0 }
                .joined(separator: " ")
            runInTerminal(command)
            return
        }
        let cli = self.cli
        let args = Array(parts.dropFirst())
        _ = await background { try cli.runRaw(args) }
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

    /// Detect running Laravel/dev workers (Horizon, queues, Reverb, scheduler,
    /// Stripe listener, Vite) grouped by kind, labeled by their project folder.
    func detectWorkers() async -> [WorkerGroup] {
        let out = await backgroundQuiet { runProcess("/bin/ps", ["-Ao", "pid=,command="]) } ?? ""
        var groups: [String: [WorkerItem]] = [:]
        for raw in out.split(separator: "\n") {
            let line = raw.trimmingCharacters(in: .whitespaces)
            guard let sp = line.firstIndex(of: " "), let pid = Int(line[..<sp]) else { continue }
            let cmd = String(line[line.index(after: sp)...])
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
    func runSetupInTerminal() {
        guard let dpl = try? cli.resolveBinary() else { return }
        let cmd = "sudo \(dpl) setup"
        let script = "tell application \"Terminal\" to do script \"\(cmd)\"\ntell application \"Terminal\" to activate"
        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: "/usr/bin/osascript")
        proc.arguments = ["-e", script]
        try? proc.run()
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
