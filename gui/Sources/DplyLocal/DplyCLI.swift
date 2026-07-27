import Foundation

/// Errors surfaced from running the `dpl` CLI.
enum DplyError: LocalizedError {
    case binaryNotFound(triedPaths: [String])
    case commandFailed(command: String, status: Int32, stderr: String)
    case decodeFailed(command: String, underlying: Error)

    var errorDescription: String? {
        switch self {
        case .binaryNotFound(let tried):
            return "Couldn't find the `dpl` binary. Tried:\n" + tried.joined(separator: "\n")
                + "\n\nBuild it with `cargo build` in the workspace, or set a path in Settings."
        case .commandFailed(let cmd, let status, let stderr):
            // dpl's own errors are already written for a human and arrive as
            // "error: <message>". Lead with that; the "`cmd` failed (exit N)"
            // wrapper is noise once there's a real message. Fall back to it only
            // when the command died without saying anything useful.
            var msg = stderr.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !msg.isEmpty else { return "`\(cmd)` failed (exit \(status))." }
            if msg.hasPrefix("error: ") { msg.removeFirst("error: ".count) }
            if let first = msg.first, first.isLowercase {
                msg = first.uppercased() + msg.dropFirst()
            }
            return msg
        case .decodeFailed(let cmd, let underlying):
            return "Couldn't parse JSON from `\(cmd)`: \(underlying.localizedDescription)"
        }
    }
}

/// Thin wrapper that runs the `dpl` CLI and decodes its `--json` output.
///
/// The GUI owns no dply logic of its own — it drives the exact same binary the
/// terminal uses, so the two never drift. `--json` guarantees machine-readable
/// output for every command.
struct DplyCLI {
    /// Optional explicit path to the binary (from Settings). When nil, we
    /// auto-resolve (see `resolveBinary`).
    var overridePath: String?
    /// dply host to target (`--host`); nil uses the CLI's stored default.
    var host: String?

    // MARK: Binary resolution

    /// Locate the `dpl` binary. Order: explicit override → the *newest* workspace
    /// build (found relative to this source file) → the copy inside this app
    /// bundle → `dpl` on `PATH`.
    ///
    /// Newest, not release-then-debug. Building release once would otherwise pin the
    /// app to that binary forever, silently ignoring every `cargo build` after it —
    /// the app would keep reporting a world that stopped existing, with no clue why.
    ///
    /// The bundled copy comes *after* the workspace so `cargo build` still updates
    /// what a developer's app drives without re-bundling, and *before* `PATH` so a
    /// downloaded app uses the binary shipped with it: an app and a CLI from
    /// different versions disagree about the wire protocol, which surfaces as
    /// fields silently missing rather than as an error.
    func resolveBinary() throws -> String {
        var tried: [String] = []

        if let override = overridePath, !override.isEmpty {
            if FileManager.default.isExecutableFile(atPath: override) { return override }
            tried.append(override + " (from Settings)")
        }

        let candidates = workspaceBinaryCandidates()
        tried.append(contentsOf: candidates)
        let newest = candidates
            .filter { FileManager.default.isExecutableFile(atPath: $0) }
            .compactMap { path -> (String, Date)? in
                let modified = try? FileManager.default.attributesOfItem(atPath: path)[.modificationDate] as? Date
                return (modified ?? nil).map { (path, $0) }
            }
            .max { $0.1 < $1.1 }?
            .0
        if let newest { return newest }

        // Shipped alongside the app (see gui/make-app.sh). Absent in a plain
        // `swift run`, where the workspace build above is what's wanted anyway.
        let bundled = DplyCLI.bundledBinary
        tried.append(bundled + " (bundled)")
        if FileManager.default.isExecutableFile(atPath: bundled) {
            return bundled
        }

        if let onPath = which("dpl") {
            return onPath
        }
        tried.append("dpl (on $PATH)")

        throw DplyError.binaryNotFound(triedPaths: tried)
    }

    /// `dpl` inside this app bundle: `DplyLocal.app/Contents/Helpers/dpl`, beside
    /// the `dpld` and `dpl-helper` it expects to find next to itself.
    static var bundledBinary: String {
        Bundle.main.bundleURL
            .appendingPathComponent("Contents/Helpers/dpl")
            .path
    }

    /// `<workspace>/target/{debug,release}/dpl` for every workspace root we know
    /// about — the build-time one first, then a relocated checkout if the
    /// build-time path has gone missing.
    private func workspaceBinaryCandidates() -> [String] {
        DplyCLI.workspaceRoots().flatMap { root in
            [
                root.appendingPathComponent("target/release/dpl").path,
                root.appendingPathComponent("target/debug/dpl").path,
            ]
        }
    }

    /// The workspace root as of compile time, derived from this file's location:
    /// …/gui/Sources/DplyLocal/DplyCLI.swift → up 3 → gui → up 1 → workspace root.
    ///
    /// `#filePath` is baked in when the app is built, so this is only correct for
    /// as long as the checkout stays put. See `relocatedWorkspace()`.
    private static let compileTimeWorkspace = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent() // DplyLocal
        .deletingLastPathComponent() // Sources
        .deletingLastPathComponent() // gui
        .deletingLastPathComponent() // dply-local (workspace root)

    private static let relocationLock = NSLock()
    private static var relocationSearched = false
    private static var relocationResult: URL?
    private static let relocationDefaultsKey = "dplyRelocatedWorkspace"

    /// Every plausible workspace root, best first.
    private static func workspaceRoots() -> [URL] {
        var roots: [URL] = []
        if isWorkspace(compileTimeWorkspace) { roots.append(compileTimeWorkspace) }
        if let moved = relocatedWorkspace(), moved != compileTimeWorkspace { roots.append(moved) }
        return roots.isEmpty ? [compileTimeWorkspace] : roots
    }

    /// A workspace is a directory holding the Cargo manifest and the GUI package —
    /// enough to tell our checkout from any other directory sharing its name.
    private static func isWorkspace(_ url: URL) -> Bool {
        let fm = FileManager.default
        return fm.fileExists(atPath: url.appendingPathComponent("Cargo.toml").path)
            && fm.fileExists(atPath: url.appendingPathComponent("gui/Package.swift").path)
    }

    /// Find the checkout after it has been moved.
    ///
    /// A released app is built once and then outlives several reorganisations of
    /// the developer's `Projects` tree; when `#filePath` stops resolving, the app
    /// has no idea the workspace it needs is sitting one directory over. So: walk
    /// up the compile-time path to the deepest ancestor that still exists, then
    /// search a bounded distance below it for a directory that looks like this
    /// workspace. The answer is cached in memory and in defaults — the search runs
    /// once per relocation, not once per `dpl` invocation.
    private static func relocatedWorkspace() -> URL? {
        relocationLock.lock()
        defer { relocationLock.unlock() }
        if relocationSearched { return relocationResult }
        relocationSearched = true

        // A previously discovered root, if it is still a workspace.
        if let remembered = UserDefaults.standard.string(forKey: relocationDefaultsKey) {
            let url = URL(fileURLWithPath: remembered)
            if isWorkspace(url) {
                relocationResult = url
                return url
            }
            UserDefaults.standard.removeObject(forKey: relocationDefaultsKey)
        }

        let name = compileTimeWorkspace.lastPathComponent
        var base = compileTimeWorkspace
        let fm = FileManager.default
        while base.pathComponents.count > 1, !fm.fileExists(atPath: base.path) {
            base = base.deletingLastPathComponent()
        }
        // Bail rather than crawl `/` or `/Users` when nothing on the path survives.
        guard base.pathComponents.count > 2, fm.fileExists(atPath: base.path) else { return nil }

        guard let found = search(from: base, named: name, maxDepth: 4) else { return nil }
        UserDefaults.standard.set(found.path, forKey: relocationDefaultsKey)
        relocationResult = found
        return found
    }

    /// Breadth-first search for a workspace directory named `name`, at most
    /// `maxDepth` levels below `base`. Skips hidden directories and the usual
    /// deep-and-uninteresting build/dependency trees so this stays milliseconds.
    private static func search(from base: URL, named name: String, maxDepth: Int) -> URL? {
        let skip: Set<String> = ["node_modules", "target", ".build", "vendor", "Library", "Pods"]
        let fm = FileManager.default
        var frontier = [base]
        for _ in 0..<maxDepth {
            var next: [URL] = []
            for dir in frontier {
                let children = (try? fm.contentsOfDirectory(
                    at: dir,
                    includingPropertiesForKeys: [.isDirectoryKey],
                    options: [.skipsHiddenFiles, .skipsPackageDescendants]
                )) ?? []
                for child in children {
                    guard (try? child.resourceValues(forKeys: [.isDirectoryKey]))?.isDirectory == true,
                          !skip.contains(child.lastPathComponent) else { continue }
                    if child.lastPathComponent == name, isWorkspace(child) { return child }
                    next.append(child)
                }
            }
            if next.isEmpty { return nil }
            frontier = next
        }
        return nil
    }

    /// The environment to hand every child process.
    ///
    /// An app launched from Finder or the Dock inherits launchd's `PATH`
    /// (`/usr/bin:/bin:/usr/sbin:/sbin`) — not a login shell's. Every `which`-based
    /// probe downstream then reports Homebrew, Composer and the database engines as
    /// "not installed" on a machine where they plainly are. The dpld launchd plists
    /// prepend these same directories for exactly this reason.
    static var toolEnvironment: [String: String] {
        var env = ProcessInfo.processInfo.environment
        let brewDirs = ["/opt/homebrew/bin", "/opt/homebrew/sbin", "/usr/local/bin", "/usr/local/sbin"]
        let current = (env["PATH"] ?? "").split(separator: ":").map(String.init)
        let missing = brewDirs.filter { !current.contains($0) && FileManager.default.fileExists(atPath: $0) }
        env["PATH"] = (missing + current).joined(separator: ":")
        return env
    }

    private func which(_ name: String) -> String? {
        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        proc.environment = DplyCLI.toolEnvironment
        proc.arguments = ["which", name]
        let pipe = Pipe()
        proc.standardOutput = pipe
        proc.standardError = Pipe()
        do {
            try proc.run()
            proc.waitUntilExit()
        } catch {
            return nil
        }
        guard proc.terminationStatus == 0 else { return nil }
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        let path = String(decoding: data, as: UTF8.self)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return path.isEmpty ? nil : path
    }

    // MARK: Running

    /// Run `dpl <args>` and return raw stdout. Injects `--host` when set.
    @discardableResult
    func runRaw(_ args: [String]) throws -> Data {
        let binary = try resolveBinary()
        var fullArgs = args
        if let host, !host.isEmpty {
            fullArgs.append(contentsOf: ["--host", host])
        }

        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: binary)
        proc.arguments = fullArgs
        proc.environment = DplyCLI.toolEnvironment
        let out = Pipe()
        let err = Pipe()
        proc.standardOutput = out
        proc.standardError = err

        try proc.run()
        let outData = out.fileHandleForReading.readDataToEndOfFile()
        let errData = err.fileHandleForReading.readDataToEndOfFile()
        proc.waitUntilExit()

        guard proc.terminationStatus == 0 else {
            throw DplyError.commandFailed(
                command: "dpl " + fullArgs.joined(separator: " "),
                status: proc.terminationStatus,
                stderr: String(decoding: errData, as: UTF8.self)
            )
        }
        return outData
    }

    /// Run a read command with `--json` and decode the result as a list of rows
    /// (an array payload, or a single object wrapped into one row).
    func rows(_ args: [String]) throws -> [Row] {
        let value = try json(args)
        switch value {
        case .array(let items):
            return items.compactMap { $0.objectValue.map(Row.init) }
        case .object(let obj):
            return [Row(obj)]
        default:
            return []
        }
    }

    /// Run a read command with `--json` and decode a single object row.
    func object(_ args: [String]) throws -> Row {
        let value = try json(args)
        return Row(value.objectValue ?? [:])
    }

    /// Run with `--json` and return the raw decoded value.
    func json(_ args: [String]) throws -> JSONValue {
        let data = try runRaw(args + ["--json"])
        do {
            return try JSONDecoder().decode(JSONValue.self, from: data)
        } catch {
            throw DplyError.decodeFailed(
                command: "dpl " + args.joined(separator: " "),
                underlying: error
            )
        }
    }
}
