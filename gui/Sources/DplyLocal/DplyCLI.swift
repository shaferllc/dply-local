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
            let msg = stderr.trimmingCharacters(in: .whitespacesAndNewlines)
            return "`\(cmd)` failed (exit \(status))." + (msg.isEmpty ? "" : "\n\(msg)")
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
    /// build (found relative to this source file) → `dpl` on `PATH`.
    ///
    /// Newest, not release-then-debug. Building release once would otherwise pin the
    /// app to that binary forever, silently ignoring every `cargo build` after it —
    /// the app would keep reporting a world that stopped existing, with no clue why.
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

        if let onPath = which("dpl") {
            return onPath
        }
        tried.append("dpl (on $PATH)")

        throw DplyError.binaryNotFound(triedPaths: tried)
    }

    /// `<workspace>/target/{debug,release}/dpl`, derived from this file's
    /// location: …/gui/Sources/DplyLocal/DplyCLI.swift → up 3 → gui → up 1 →
    /// workspace root.
    private func workspaceBinaryCandidates() -> [String] {
        let thisFile = URL(fileURLWithPath: #filePath)
        let workspace = thisFile
            .deletingLastPathComponent() // DplyLocal
            .deletingLastPathComponent() // Sources
            .deletingLastPathComponent() // gui
            .deletingLastPathComponent() // dply-local (workspace root)
        return [
            workspace.appendingPathComponent("target/release/dpl").path,
            workspace.appendingPathComponent("target/debug/dpl").path,
        ]
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
