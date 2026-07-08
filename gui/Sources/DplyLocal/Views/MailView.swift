import SwiftUI

/// Middle column: the captured-mail inbox (newest first).
struct MailListView: View {
    @EnvironmentObject var store: Store
    @Binding var selection: String?

    var body: some View {
        List(selection: $selection) {
            if store.mailMessages.isEmpty && !store.isLoading {
                ContentUnavailableView(
                    "Inbox empty",
                    systemImage: "envelope",
                    description: Text("Point your app's mailer at 127.0.0.1:1025 (no auth). Captured mail appears here.")
                )
            }
            ForEach(store.mailMessages) { row in
                VStack(alignment: .leading, spacing: 3) {
                    Text(nonEmpty(row.cell(["subject"]), "(no subject)"))
                        .font(.body.weight(.semibold))
                        .lineLimit(1)
                    HStack(spacing: 6) {
                        Image(systemName: "arrow.right").font(.caption2).foregroundStyle(.secondary)
                        Text(nonEmpty(row.cell(["to"]), "—"))
                            .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    }
                }
                .padding(.vertical, 3)
                .tag(row.cell(["id"]))
            }
        }
        .overlay { if store.isLoading { ProgressView().controlSize(.small) } }
    }

    private func nonEmpty(_ s: String, _ fallback: String) -> String {
        s.isEmpty ? fallback : s
    }
}

/// Detail pane: the raw message source.
struct MailDetailView: View {
    @EnvironmentObject var store: Store
    let id: String

    @State private var body_ = ""
    @State private var loading = true

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ScrollView {
                Text(loading ? "Loading…" : (body_.isEmpty ? "(empty message)" : body_))
                    .font(.system(.callout, design: .monospaced))
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(16)
            }
        }
        .background(Color(nsColor: .textBackgroundColor))
        .task(id: id) {
            loading = true
            body_ = await store.mailBody(id)
            loading = false
        }
    }
}
