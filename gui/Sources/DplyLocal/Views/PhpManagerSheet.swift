import SwiftUI

/// A PHP version-manager operation (install / upgrade / repair / uninstall).
/// Each maps to a `dpl php <verb> <version>` subcommand that shells out to
/// Homebrew.
enum PhpAction: Identifiable {
    case install(String)
    case upgrade(String)
    case repair(String)
    case uninstall(String)
    /// Install a PHP extension (version, extension name).
    case extInstall(String, String)

    var id: String {
        if case .extInstall(let v, let n) = self { return "ext-\(v)-\(n)" }
        return verb + version
    }

    var version: String {
        switch self {
        case .install(let v), .upgrade(let v), .repair(let v), .uninstall(let v): return v
        case .extInstall(let v, _): return v
        }
    }

    /// The full `dpl` argument list for this operation.
    var cliArgs: [String] {
        switch self {
        case .extInstall(let v, let n): return ["php", "ext-install", v, n]
        default: return ["php", verb, version]
        }
    }

    /// The `dpl php` subcommand verb.
    var verb: String {
        switch self {
        case .install: return "install"
        case .upgrade: return "upgrade"
        case .repair: return "repair"
        case .uninstall: return "uninstall"
        case .extInstall: return "ext-install"
        }
    }

    var title: String {
        switch self {
        case .install: return "Install PHP \(version)"
        case .upgrade: return "Upgrade PHP \(version)"
        case .repair: return "Repair PHP \(version)"
        case .uninstall: return "Uninstall PHP \(version)"
        case .extInstall(let v, let n): return "Install \(n) for PHP \(v)"
        }
    }

    var blurb: String {
        switch self {
        case .install: return "Runs `brew install php@\(version)` (taps shivammathur/php for older lines). This can take several minutes while Homebrew builds."
        case .upgrade: return "Runs `brew upgrade php@\(version)` to the newest patch release."
        case .repair: return "Fixes a broken PHP \(version): reinstalls the keg if its binary is missing, and disables any extension that fails to load (missing .so). Fast unless a reinstall is needed."
        case .uninstall: return "Runs `brew uninstall php@\(version)`. Sites pinned to it will fall back to the default."
        case .extInstall(let v, let n): return "Runs `brew install shivammathur/extensions/\(n)@\(v)`, then loads it into php-fpm."
        }
    }

    var icon: String {
        switch self {
        case .install: return "arrow.down.circle"
        case .upgrade: return "arrow.up.circle"
        case .repair: return "wrench.and.screwdriver"
        case .uninstall: return "trash"
        case .extInstall: return "puzzlepiece.extension"
        }
    }

    var destructive: Bool { if case .uninstall = self { return true } else { return false } }
}

/// Streams a `dpl php …` Homebrew operation live, then refreshes PHP state.
/// Modeled on `InstallEngineSheet`.
struct PhpManagerSheet: View {
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss

    let action: PhpAction

    @State private var output = ""
    @State private var running = false
    @State private var finished = false
    @State private var failed = false
    @State private var process: Process?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 10) {
                Image(systemName: action.icon)
                    .foregroundStyle(action.destructive ? AnyShapeStyle(.red) : AnyShapeStyle(Theme.brand))
                Text(action.title).font(.headline)
            }

            if !running && !finished {
                Text(action.blurb).font(.callout).foregroundStyle(.secondary)
            } else {
                ScrollViewReader { proxy in
                    ScrollView {
                        Text(output.isEmpty ? "Starting…" : output)
                            .font(.system(.caption, design: .monospaced))
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(10)
                        Color.clear.frame(height: 1).id("bottom")
                    }
                    .frame(height: 280)
                    .background(Color(nsColor: .textBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
                    .onChange(of: output) { proxy.scrollTo("bottom", anchor: .bottom) }
                }
            }

            HStack {
                if running { ProgressView().controlSize(.small); Text("Working…").foregroundStyle(.secondary) }
                if finished {
                    Label(failed ? "Failed — see output above" : "Done",
                          systemImage: failed ? "xmark.circle.fill" : "checkmark.circle.fill")
                        .foregroundStyle(failed ? .red : .green)
                }
                Spacer()
                if finished {
                    Button("Close") { dismiss() }.keyboardShortcut(.defaultAction)
                } else if running {
                    Button("Cancel", role: .cancel) { process?.terminate(); dismiss() }
                } else {
                    Button("Cancel") { dismiss() }
                    Button(action.destructive ? "Uninstall" : action.verb.capitalized) { start() }
                        .buttonStyle(.borderedProminent)
                        .tint(action.destructive ? .red : Theme.violet)
                }
            }
        }
        .padding(18)
        .frame(width: 540)
        .onDisappear { process?.terminate() }
    }

    private func start() {
        let binary: String
        do { binary = try store.cli.resolveBinary() } catch {
            output = error.localizedDescription; finished = true; failed = true; return
        }
        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: binary)
        proc.arguments = action.cliArgs
        // php-fpm builds shell out to brew, which lives under the Homebrew
        // prefix; make sure it's on PATH for the spawned process.
        var env = ProcessInfo.processInfo.environment
        let brewPaths = "/opt/homebrew/bin:/usr/local/bin"
        env["PATH"] = brewPaths + ":" + (env["PATH"] ?? "/usr/bin:/bin")
        proc.environment = env

        let pipe = Pipe()
        proc.standardOutput = pipe
        proc.standardError = pipe
        pipe.fileHandleForReading.readabilityHandler = { h in
            let d = h.availableData
            guard !d.isEmpty else { return }
            let chunk = String(decoding: d, as: UTF8.self)
            DispatchQueue.main.async { output += chunk }
        }
        proc.terminationHandler = { p in
            DispatchQueue.main.async {
                pipe.fileHandleForReading.readabilityHandler = nil
                running = false; finished = true
                failed = p.terminationStatus != 0
                Task {
                    await store.loadPhpVersions()
                    await store.loadPhpCatalog()
                    await store.loadPhpConfig()
                }
            }
        }
        do { try proc.run(); process = proc; running = true }
        catch { output += "\nFailed to start: \(error.localizedDescription)"; finished = true; failed = true }
    }
}
