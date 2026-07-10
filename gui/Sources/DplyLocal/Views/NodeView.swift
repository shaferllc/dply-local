import SwiftUI

/// The Node surface: each site's pinned Node version, and one-click pinning.
///
/// dpl doesn't run `node` — your shell does — so this panel is honest about the
/// split: it writes each repo's `.nvmrc` (the pin), and names the manager
/// (fnm/nvm) that does the actual switch when you `cd` in. Pinning here, detecting
/// from `package.json`, and the manager hint are all it can truthfully offer.
struct NodePage: View {
    @EnvironmentObject var store: Store
    @State private var editing: String?
    @State private var draft = ""
    @State private var busy: Set<String> = []

    /// Common LTS lines to offer as quick picks.
    private let quickPicks = ["22", "20", "18"]

    private var sites: [Row] {
        store.localSites.filter { $0.cell(["source"]) != "proxy" }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Divider()
            if sites.isEmpty {
                ContentUnavailableView(
                    "No local sites",
                    systemImage: "hexagon",
                    description: Text("Link a project with `dpl link .` to pin its Node version here.")
                )
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollView {
                    LazyVStack(spacing: 8) {
                        ForEach(sites) { row in siteCard(row) }
                    }
                    .padding(16)
                }
            }
        }
        .task {
            if store.localSites.isEmpty { await store.loadLocal() }
            await store.loadNodeManager()
        }
    }

    private var header: some View {
        HStack(spacing: 10) {
            Label("Node", systemImage: "hexagon")
                .font(.title3.weight(.semibold))
            Spacer()
            switch store.nodeManager {
            case .some(let m):
                Label("\(m) — switches on `cd`", systemImage: "checkmark.seal")
                    .font(.caption).foregroundStyle(.secondary)
            case .none:
                Label("No manager — install fnm to auto-switch", systemImage: "exclamationmark.triangle")
                    .font(.caption).foregroundStyle(.orange)
            }
        }
        .padding(16)
    }

    private func siteCard(_ row: Row) -> some View {
        let name = row.cell(["name"])
        let version = row.first(["node"])
        let source = row.first(["node_source"])
        return HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(name).font(.callout.weight(.medium))
                if let version {
                    Text("Node \(version)  ·  \(source ?? ".nvmrc")")
                        .font(.caption).foregroundStyle(.secondary)
                } else {
                    Text("unpinned").font(.caption).foregroundStyle(.tertiary)
                }
            }
            Spacer()
            if busy.contains(name) {
                ProgressView().controlSize(.small)
            } else if editing == name {
                editor(name)
            } else {
                controls(row, name: name, version: version)
            }
        }
        .padding(12)
        .background(RoundedRectangle(cornerRadius: Theme.tileRadius).fill(Theme.card))
    }

    @ViewBuilder
    private func controls(_ row: Row, name: String, version: String?) -> some View {
        ForEach(quickPicks, id: \.self) { v in
            Button(v) { Task { await pin(name, v) } }
                .buttonStyle(.bordered)
                .tint(version == v ? Theme.violet : nil)
        }
        // Only offer "Detect" when package.json has something to detect from.
        if row.first(["node"]) == nil || row.first(["node_source"]) == "package.json" {
            Button {
                Task { await runBusy(name) { await store.detectNodeVersion(name: name) } }
            } label: { Label("Detect", systemImage: "wand.and.stars") }
                .buttonStyle(.bordered)
                .help("Read the version from this repo's package.json engines.node")
        }
        Button {
            draft = version ?? ""
            editing = name
        } label: { Image(systemName: "pencil") }
            .buttonStyle(.bordered)
            .help("Type an exact version")
    }

    private func editor(_ name: String) -> some View {
        HStack(spacing: 6) {
            TextField("e.g. 20.11.0", text: $draft)
                .textFieldStyle(.roundedBorder)
                .frame(width: 120)
                .onSubmit { Task { await pin(name, draft) } }
            Button("Pin") { Task { await pin(name, draft) } }
                .buttonStyle(.borderedProminent).tint(Theme.violet)
                .disabled(draft.trimmingCharacters(in: .whitespaces).isEmpty)
            Button("Cancel") { editing = nil }
                .buttonStyle(.bordered)
        }
    }

    private func pin(_ name: String, _ version: String) async {
        let v = version.trimmingCharacters(in: .whitespaces)
        guard !v.isEmpty else { return }
        editing = nil
        await runBusy(name) { await store.setNodeVersion(name: name, version: v) }
    }

    private func runBusy(_ name: String, _ work: () async -> Void) async {
        busy.insert(name)
        await work()
        busy.remove(name)
    }
}
