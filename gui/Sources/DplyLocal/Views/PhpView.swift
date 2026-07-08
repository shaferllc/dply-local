import SwiftUI

/// Middle column: installed PHP versions, each able to become the global
/// default. Per-site pinning lives on the site's detail pane.
struct PhpListView: View {
    @EnvironmentObject var store: Store

    var body: some View {
        List {
            if store.phpVersions.isEmpty && !store.isLoading {
                ContentUnavailableView("No PHP found", systemImage: "chevron.left.forwardslash.chevron.right",
                    description: Text("Install one with `brew install php`."))
            }
            ForEach(store.phpVersions) { row in
                let version = row.cell(["version"])
                HStack(spacing: 11) {
                    GradientTile(systemImage: "chevron.left.forwardslash.chevron.right", size: 30)
                    VStack(alignment: .leading, spacing: 2) {
                        Text("PHP \(version)").font(.body.weight(.semibold))
                        Text(row.cell(["binary"]))
                            .font(.system(.caption2, design: .monospaced))
                            .foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                    }
                    Spacer()
                    Button("Set default") {
                        Task { await store.usePhp(version: version, site: nil) }
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                }
                .padding(.vertical, 3)
            }
        }
        .overlay { if store.isLoading { ProgressView().controlSize(.small) } }
    }
}

/// Detail pane: a short explainer of PHP management.
struct PhpDetailView: View {
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                HStack(spacing: 12) {
                    GradientTile(systemImage: "chevron.left.forwardslash.chevron.right", size: 40)
                    Text("PHP versions").font(.title2.weight(.bold))
                    Spacer()
                }
                DetailSection(title: "How it works") {
                    VStack(alignment: .leading, spacing: 8) {
                        Label("Each installed version runs its own php-fpm pool.", systemImage: "bolt.fill")
                        Label("“Set default” applies to sites without an explicit pin.", systemImage: "star")
                        Label("Pin a single site from its detail pane (Local Sites → pick a site → PHP).", systemImage: "pin")
                    }
                    .font(.callout)
                    .labelStyle(.titleAndIcon)
                }
            }
            .padding(18)
        }
    }
}
