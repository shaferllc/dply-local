import SwiftUI

/// Installs a database engine via Homebrew (`dpl service install <engine>[@ver]`)
/// so a fresh machine needs neither DBngin nor a manual brew install. Streams
/// the live install output, then refreshes the version list.
struct InstallEngineSheet: View {
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss

    @State private var engine = "postgres"
    @State private var version = ""
    @State private var output = ""
    @State private var running = false
    @State private var finished = false
    @State private var process: Process?

    private let engines = ["postgres", "mysql", "mariadb", "redis", "meilisearch", "mongodb", "minio", "stripe-mock"]

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Install a database engine").font(.headline)

            if !running && !finished {
                Form {
                    Picker("Engine", selection: $engine) {
                        ForEach(engines, id: \.self) { Text($0.capitalized).tag($0) }
                    }
                    TextField("Version", text: $version, prompt: Text("optional, e.g. 17"))
                }
                .formStyle(.columns)
                Text("Runs `brew install` for the engine. Homebrew must be installed.")
                    .font(.caption).foregroundStyle(.secondary)
            } else {
                ScrollView {
                    Text(output.isEmpty ? "Starting…" : output)
                        .font(.system(.caption, design: .monospaced))
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(10)
                }
                .frame(height: 260)
                .background(Color(nsColor: .textBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
            }

            HStack {
                if running { ProgressView().controlSize(.small); Text("Installing…") }
                Spacer()
                if finished {
                    Button("Done") { dismiss() }.keyboardShortcut(.defaultAction)
                } else if running {
                    Button("Cancel", role: .cancel) { process?.terminate(); dismiss() }
                } else {
                    Button("Cancel") { dismiss() }
                    Button("Install") { start() }
                        .buttonStyle(.borderedProminent).tint(Theme.violet)
                }
            }
        }
        .padding(18)
        .frame(width: 520)
        .onDisappear { process?.terminate() }
    }

    private func start() {
        let spec = version.trimmingCharacters(in: .whitespaces).isEmpty ? engine : "\(engine)@\(version)"
        let binary: String
        do { binary = try store.cli.resolveBinary() } catch {
            output = error.localizedDescription; finished = true; return
        }
        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: binary)
        proc.arguments = ["service", "install", spec]
        let pipe = Pipe()
        proc.standardOutput = pipe
        proc.standardError = pipe
        pipe.fileHandleForReading.readabilityHandler = { h in
            let d = h.availableData
            guard !d.isEmpty else { return }
            let chunk = String(decoding: d, as: UTF8.self)
            DispatchQueue.main.async { output += chunk }
        }
        proc.terminationHandler = { _ in
            DispatchQueue.main.async {
                pipe.fileHandleForReading.readabilityHandler = nil
                running = false; finished = true
                Task { await store.loadVersions(); await store.loadServices() }
            }
        }
        do { try proc.run(); process = proc; running = true }
        catch { output += "\nFailed to start: \(error.localizedDescription)"; finished = true }
    }
}
