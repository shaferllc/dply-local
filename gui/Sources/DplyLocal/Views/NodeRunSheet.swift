import SwiftUI

/// A pending run, used as `.sheet(item:)` state so presenting one is a single
/// assignment rather than a boolean plus three parallel `@State` fields.
struct NodeRunRequest: Identifiable {
    let title: String
    let scope: String
    let args: [String]
    var asksForScript = false
    var id: String { title + scope + args.joined(separator: " ") }
}

/// One package-manager run, streamed live.
///
/// The CLI already does the interesting work — pick each site's agent, apply its
/// Node pin, fan out, tally failures — so this sheet is deliberately thin: it
/// spawns `dpl node …`, shows the output as it arrives, and reports the exit
/// status. Nothing here decides *what* npm/pnpm/yarn/bun should do, which is why
/// the GUI and the terminal can't drift apart.
///
/// It streams rather than buffering because that is the whole point: a fleet
/// install is minutes long, and a spinner that hides which site is currently
/// building is worse than a terminal.
struct NodeRunSheet: View {
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss

    /// Human title, e.g. "Install dependencies".
    let title: String
    /// What it runs against, e.g. "aurora-ui" or "all sites".
    let scope: String
    /// Arguments after `dpl` — everything except the script name when `asksForScript`.
    let args: [String]
    /// Prompt for a script name before running (`dpl node run <script>`).
    var asksForScript = false

    @State private var script = ""
    @State private var output = ""
    @State private var process: Process?
    @State private var running = false
    @State private var exitStatus: Int32?
    /// Bumped on every output chunk so the scroll view has something to follow.
    @State private var tick = 0

    /// Keeps a long install from growing without bound — SwiftUI renders this as
    /// one `Text`, and a full `npm install` across a fleet is megabytes.
    private static let maxOutput = 200_000

    private var canRun: Bool {
        !running && (!asksForScript || !script.trimmingCharacters(in: .whitespaces).isEmpty)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .firstTextBaseline) {
                Text(title).font(.headline)
                Text("· \(scope)").font(.caption).foregroundStyle(.secondary)
                Spacer()
            }

            if asksForScript {
                HStack(spacing: 6) {
                    TextField("Script name", text: $script, prompt: Text("build"))
                        .textFieldStyle(.roundedBorder).frame(width: 180)
                        .disabled(running)
                        .onSubmit { if canRun { start() } }
                    Button("Run") { start() }
                        .buttonStyle(.borderedProminent).tint(Theme.violet)
                        .disabled(!canRun)
                    Text("Sites without that script are reported as failures.")
                        .font(.caption2).foregroundStyle(.secondary)
                    Spacer()
                }
            }

            ScrollViewReader { proxy in
                ScrollView {
                    Text(output.isEmpty ? (running ? "Starting…" : "Nothing yet.") : output)
                        .font(.system(.caption2, design: .monospaced))
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(10)
                    // Anchor the autoscroll follows.
                    Color.clear.frame(height: 1).id("bottom")
                }
                .background(Color(nsColor: .textBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
                .onChange(of: tick) {
                    proxy.scrollTo("bottom", anchor: .bottom)
                }
            }
            .frame(height: 320)

            HStack(spacing: 8) {
                if running {
                    ProgressView().controlSize(.small)
                    Text("Running — closing this stops it.").font(.caption).foregroundStyle(.secondary)
                } else if let exitStatus {
                    Label(
                        exitStatus == 0 ? "Finished" : "Finished with errors (exit \(exitStatus))",
                        systemImage: exitStatus == 0 ? "checkmark.circle.fill" : "exclamationmark.triangle.fill"
                    )
                    .font(.caption)
                    .foregroundStyle(exitStatus == 0 ? Theme.live : .orange)
                }
                Spacer()
                if !output.isEmpty {
                    Button("Copy") { store.copyToPasteboard(output) }.buttonStyle(.bordered)
                }
                Button(running ? "Stop" : "Close") {
                    process?.terminate()
                    if !running { dismiss() }
                }
                .keyboardShortcut(running ? .cancelAction : .defaultAction)
            }
        }
        .padding(16)
        .frame(width: 660)
        .task { if !asksForScript { start() } }
        // A run outlives its sheet otherwise — the package manager would keep
        // writing into a project nobody is watching.
        .onDisappear { process?.terminate() }
    }

    private func start() {
        guard !running else { return }
        guard let dpl = try? store.cli.resolveBinary() else {
            output = "Couldn't find the `dpl` binary — set its path in Settings."
            return
        }
        var full = args
        if asksForScript {
            full.append(script.trimmingCharacters(in: .whitespaces))
        }

        output = ""
        exitStatus = nil
        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: dpl)
        proc.arguments = full
        proc.environment = DplyCLI.toolEnvironment
        let pipe = Pipe()
        proc.standardOutput = pipe
        // Package managers write progress to stderr and dpl reports the Node
        // switch there too; both belong in the same transcript, in order.
        proc.standardError = pipe
        pipe.fileHandleForReading.readabilityHandler = { handle in
            let data = handle.availableData
            guard !data.isEmpty else { return }
            let chunk = String(decoding: data, as: UTF8.self)
            DispatchQueue.main.async {
                output += chunk
                if output.count > Self.maxOutput {
                    output = "… earlier output trimmed …\n"
                        + String(output.suffix(Self.maxOutput))
                }
                tick += 1
            }
        }
        proc.terminationHandler = { p in
            DispatchQueue.main.async {
                running = false
                exitStatus = p.terminationStatus
                pipe.fileHandleForReading.readabilityHandler = nil
            }
        }
        do {
            try proc.run()
            process = proc
            running = true
        } catch {
            output = "Failed to start: \(error.localizedDescription)"
        }
    }
}
