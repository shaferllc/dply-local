import SwiftUI

/// Create a reverse proxy: a `.test` host → another local service.
struct AddProxySheet: View {
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss
    @State private var name = ""
    @State private var target = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("New proxy").font(.headline)
            Text("Point a .test host at another local service — a Docker container, a Vite dev server, any port.")
                .font(.caption).foregroundStyle(.secondary)
            Form {
                TextField("Host", text: $name, prompt: Text("blog  (→ blog.test)"))
                TextField("Target", text: $target, prompt: Text("http://localhost:3000"))
            }
            .formStyle(.columns)
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                Button("Create") {
                    Task { await store.proxyLocal(name: name, target: target); dismiss() }
                }
                .buttonStyle(.borderedProminent).tint(Theme.violet)
                .disabled(name.isEmpty || target.isEmpty)
            }
        }
        .padding(18).frame(width: 400)
    }
}

/// Recent request logs for a local site (`dpl logs <name>`).
struct SiteLogsSheet: View {
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss
    let site: String

    @State private var text = ""
    @State private var loading = true

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text("Logs — \(site)").font(.headline)
                Spacer()
                Button { Task { await load() } } label: { Image(systemName: "arrow.clockwise") }.disabled(loading)
                Button("Close") { dismiss() }
            }
            .padding(12)
            Divider()
            ScrollView {
                Text(text.isEmpty ? (loading ? "Loading…" : "No log entries yet.") : text)
                    .font(.system(.caption, design: .monospaced))
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(12)
            }
            .background(Color(nsColor: .textBackgroundColor))
        }
        .frame(width: 720, height: 460)
        .task { await load() }
    }

    private func load() async {
        loading = true
        text = await store.siteLogs(site)
        loading = false
    }
}

/// Share a local site publicly via a Cloudflare quick tunnel — streams
/// `dpl share <name>` and surfaces the trycloudflare URL; closing stops it.
struct ShareSheet: View {
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss
    let site: String

    @State private var output = ""
    @State private var running = false
    @State private var process: Process?

    /// The public URL, scraped from cloudflared's output as it appears.
    private var url: String? {
        output.split(whereSeparator: \.isWhitespace)
            .map(String.init)
            .first { $0.contains("trycloudflare.com") && $0.hasPrefix("https://") }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Share \(site)").font(.headline)
            if let url {
                HStack {
                    Text(url).font(.system(.callout, design: .monospaced)).textSelection(.enabled)
                    Button { store.copyToPasteboard(url) } label: { Image(systemName: "doc.on.doc") }.buttonStyle(.borderless)
                    Button { store.openURL(url) } label: { Image(systemName: "safari") }.buttonStyle(.borderless)
                }
                .padding(8).background(.green.opacity(0.12), in: RoundedRectangle(cornerRadius: 8))
            }
            ScrollView {
                Text(output.isEmpty ? "Starting tunnel…" : output)
                    .font(.system(.caption2, design: .monospaced)).textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading).padding(10)
            }
            .frame(height: 200)
            .background(Color(nsColor: .textBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
            HStack {
                if running { ProgressView().controlSize(.small); Text("Sharing — closing stops it").font(.caption).foregroundStyle(.secondary) }
                Spacer()
                Button("Stop") { process?.terminate(); dismiss() }
            }
        }
        .padding(16).frame(width: 560)
        .task { start() }
        .onDisappear { process?.terminate() }
    }

    private func start() {
        guard let dpl = try? store.cli.resolveBinary() else { output = "dpl not found"; return }
        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: dpl)
        // `quick` explicitly: this sheet is the foreground Cloudflare tunnel that
        // lives as long as the window is open. Bare `dpl share` now heads a
        // subcommand tree for permanent Jetty tunnels, and would reject a site
        // name as an unknown subcommand.
        proc.arguments = ["share", "quick", site]
        let pipe = Pipe()
        proc.standardOutput = pipe
        proc.standardError = pipe
        pipe.fileHandleForReading.readabilityHandler = { h in
            let d = h.availableData
            guard !d.isEmpty else { return }
            let chunk = String(decoding: d, as: UTF8.self)
            DispatchQueue.main.async { output += chunk }
        }
        proc.terminationHandler = { _ in DispatchQueue.main.async { running = false } }
        do { try proc.run(); process = proc; running = true }
        catch { output += "\nFailed to start: \(error.localizedDescription)" }
    }
}
