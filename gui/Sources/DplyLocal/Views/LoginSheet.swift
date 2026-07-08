import SwiftUI

/// Drives `dpl login` (the device-authorization flow) and streams its output
/// live. The CLI opens the browser itself and polls until you approve; this
/// sheet shows the verification URL/code and the progress dots, then closes on
/// success.
struct LoginSheet: View {
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss

    @State private var output = "Starting device-flow login…\n"
    @State private var running = false
    @State private var process: Process?
    @State private var finished = false
    @State private var succeeded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Log in to dply")
                .font(.headline)
            Text(store.activeHost)
                .font(.caption)
                .foregroundStyle(.secondary)

            ScrollView {
                Text(output)
                    .font(.system(.callout, design: .monospaced))
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(10)
            }
            .frame(height: 240)
            .background(Color(nsColor: .textBackgroundColor),
                        in: RoundedRectangle(cornerRadius: 8))

            HStack {
                if running { ProgressView().controlSize(.small); Text("Waiting for approval…") }
                Spacer()
                if finished {
                    Button(succeeded ? "Done" : "Close") {
                        dismiss()
                    }
                    .keyboardShortcut(.defaultAction)
                } else {
                    Button("Cancel", role: .cancel) {
                        process?.terminate()
                        dismiss()
                    }
                }
            }
        }
        .padding(16)
        .frame(width: 520)
        .task { start() }
        .onDisappear { process?.terminate() }
    }

    private func start() {
        guard !running, process == nil else { return }
        let cli = store.cli
        let binary: String
        do {
            binary = try cli.resolveBinary()
        } catch {
            output += "\n\(error.localizedDescription)\n"
            finished = true
            return
        }

        var args = ["login"]
        if !store.host.isEmpty { args.append(contentsOf: ["--host", store.host]) }

        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: binary)
        proc.arguments = args
        let pipe = Pipe()
        proc.standardOutput = pipe
        proc.standardError = pipe

        pipe.fileHandleForReading.readabilityHandler = { handle in
            let data = handle.availableData
            guard !data.isEmpty else { return }
            let chunk = String(decoding: data, as: UTF8.self)
            DispatchQueue.main.async { output += chunk }
        }

        proc.terminationHandler = { p in
            DispatchQueue.main.async {
                pipe.fileHandleForReading.readabilityHandler = nil
                running = false
                finished = true
                succeeded = p.terminationStatus == 0
                Task { await store.refreshAll() }
                if succeeded { output += "\n✓ Logged in.\n" }
            }
        }

        do {
            try proc.run()
            process = proc
            running = true
        } catch {
            output += "\nFailed to start login: \(error.localizedDescription)\n"
            finished = true
        }
    }
}
