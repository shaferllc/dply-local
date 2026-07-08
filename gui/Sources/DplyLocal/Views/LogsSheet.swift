import SwiftUI

/// A sheet showing a page of an edge site's request logs (`dpl dply edge:logs`),
/// with a refresh button and a line-count control.
struct LogsSheet: View {
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss

    let siteID: String
    let title: String

    @State private var text = ""
    @State private var limit = 100
    @State private var loading = false

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text("Logs — \(title)").font(.headline)
                Spacer()
                Picker("Lines", selection: $limit) {
                    Text("50").tag(50)
                    Text("100").tag(100)
                    Text("250").tag(250)
                }
                .pickerStyle(.segmented)
                .frame(width: 160)
                .onChange(of: limit) { Task { await load() } }

                Button { Task { await load() } } label: {
                    Image(systemName: "arrow.clockwise")
                }
                .disabled(loading)
                Button("Close") { dismiss() }
            }
            .padding(12)
            Divider()

            ScrollView {
                Text(text.isEmpty ? (loading ? "Loading…" : "No log entries.") : text)
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
        let cli = store.cli
        let id = siteID
        let n = limit
        if let out = await store.background({
            String(decoding: try cli.runRaw(["dply", "edge:logs", id, "--limit", String(n)]), as: UTF8.self)
        }) {
            text = out
        }
        loading = false
    }
}
