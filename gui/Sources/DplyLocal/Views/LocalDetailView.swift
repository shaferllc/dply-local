import SwiftUI

/// Detail pane for a local `.test` site: overview + open / secure / unlink.
/// The data comes straight from `dpl sites --json` (a `SiteInfo`).
struct LocalDetailView: View {
    @EnvironmentObject var store: Store
    let site: Row

    private var name: String { site.cell(["name"]) }
    private var url: String { site.cell(["url"]) }
    private var isSecure: Bool { site.dig("secure") == .bool(true) }
    private var isServing: Bool { site.dig("serving") == .bool(true) }
    private var isLinked: Bool { site.cell(["source"]) == "linked" }
    private var phpLabel: String {
        let v = site.cell(["php"])
        return v.isEmpty ? "default" : v
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                HStack(spacing: 12) {
                    GradientTile(systemImage: "globe", size: 44, active: isServing)
                    VStack(alignment: .leading, spacing: 3) {
                        Text(site.cell(["host"]))
                            .font(.title2.weight(.bold))
                        Text(site.cell(["path"]))
                            .font(.system(.caption, design: .monospaced))
                            .foregroundStyle(.secondary)
                            .lineLimit(1).truncationMode(.middle)
                    }
                    Spacer()
                    StatusBadge(status: isServing ? "serving" : "stopped")
                }

                HStack(spacing: 10) {
                    Button {
                        store.openURL(url)
                    } label: {
                        Label("Open", systemImage: "safari")
                    }
                    .buttonStyle(.borderedProminent)
                    .tint(Theme.violet)
                    .disabled(!isServing)

                    Button {
                        Task { await store.setLocalSecure(name: name, secure: !isSecure) }
                    } label: {
                        Label(isSecure ? "Disable HTTPS" : "Secure (HTTPS)",
                              systemImage: isSecure ? "lock.open" : "lock")
                    }
                    .disabled(!isLinked)
                    .help(isLinked ? "" : "HTTPS applies to linked sites")

                    if isLinked {
                        Menu {
                            ForEach(store.availablePhpVersions, id: \.self) { v in
                                Button("PHP \(v)") { Task { await store.usePhp(version: v, site: name) } }
                            }
                        } label: {
                            Label("PHP \(phpLabel)", systemImage: "chevron.left.forwardslash.chevron.right")
                        }
                        .menuStyle(.borderlessButton)
                        .fixedSize()

                        Button(role: .destructive) {
                            Task { await store.unlinkLocal(name: name) }
                        } label: {
                            Label("Unlink", systemImage: "trash")
                        }
                    }
                    Spacer()
                }

                DetailSection(title: "Overview") {
                    KeyValueView(row: site, fields: [
                        ("Host", ["host"]),
                        ("URL", ["url"]),
                        ("Source", ["source"]),
                        ("Serving", ["serving"]),
                        ("HTTPS", ["secure"]),
                        ("PHP", ["php"]),
                        ("Project", ["path"]),
                        ("Doc root", ["docroot"]),
                    ])
                }

                if !isServing {
                    Text("This site isn't being served — check the daemon log (the folder may be missing or PHP failed to start).")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }
            }
            .padding(18)
        }
    }
}
