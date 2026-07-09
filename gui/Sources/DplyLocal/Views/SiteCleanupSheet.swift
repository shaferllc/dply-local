import SwiftUI

/// Find linked sites that point at the same folder (e.g. `dply`, `tsmonitor`,
/// `moose` all → Projects/moose) and remove the redundant ones in one pass.
struct SiteCleanupSheet: View {
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss

    /// Names marked for removal.
    @State private var toRemove: Set<String> = []
    @State private var working = false
    @State private var finished = false
    @State private var removedCount = 0

    /// Duplicate groups: a project path shared by more than one linked site,
    /// each group's sites sorted so the shortest/simplest name is first (kept).
    private var groups: [(path: String, sites: [Row])] {
        let linked = store.localSites.filter { $0.cell(["source"]) == "linked" }
        let byPath = Dictionary(grouping: linked) { $0.cell(["path"]) }
        return byPath
            .filter { $0.value.count > 1 }
            .map { (path: $0.key, sites: $0.value.sorted { $0.cell(["name"]).count < $1.cell(["name"]).count }) }
            .sorted { $0.path < $1.path }
    }

    private var dupCount: Int { groups.reduce(0) { $0 + $1.sites.count - 1 } }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Divider()
            content
            Divider()
            footer
        }
        .frame(width: 560, height: 560)
        .onAppear(perform: preselect)
    }

    private var header: some View {
        HStack(spacing: 12) {
            GradientTile(systemImage: "square.on.square.dashed", size: 40)
            VStack(alignment: .leading, spacing: 2) {
                Text("Clean up duplicate sites").font(.title3.weight(.bold))
                Text("\(groups.count) folder\(groups.count == 1 ? "" : "s") served under multiple names · \(dupCount) removable")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
        }
        .padding(16)
    }

    @ViewBuilder private var content: some View {
        if finished {
            centered {
                VStack(spacing: 10) {
                    Image(systemName: "checkmark.seal.fill").font(.largeTitle).foregroundStyle(.green)
                    Text("Removed \(removedCount) duplicate\(removedCount == 1 ? "" : "s")").font(.headline)
                }
            }
        } else if groups.isEmpty {
            centered {
                VStack(spacing: 8) {
                    Image(systemName: "checkmark.circle").font(.largeTitle).foregroundStyle(.green)
                    Text("No duplicates").font(.headline)
                    Text("No folder is served under more than one linked name.")
                        .font(.caption).foregroundStyle(.secondary)
                }
            }
        } else {
            List {
                ForEach(groups, id: \.path) { group in
                    Section {
                        ForEach(group.sites) { site in
                            let name = site.cell(["name"])
                            let removing = toRemove.contains(name)
                            Button {
                                toggle(name)
                            } label: {
                                HStack(spacing: 10) {
                                    Image(systemName: removing ? "trash.circle.fill" : "checkmark.circle.fill")
                                        .foregroundStyle(removing ? AnyShapeStyle(.red) : AnyShapeStyle(.green))
                                    Text(site.cell(["host"]))
                                        .font(.callout.weight(.medium))
                                        .strikethrough(removing, color: .red)
                                    Spacer()
                                    Text(removing ? "remove" : "keep")
                                        .font(.caption2).foregroundStyle(removing ? .red : .green)
                                }
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                        }
                    } header: {
                        Text(group.path).font(.system(.caption2, design: .monospaced))
                            .foregroundStyle(.secondary).textCase(nil)
                    }
                }
            }
        }
    }

    private var footer: some View {
        HStack {
            if !finished && !groups.isEmpty {
                Button("Reset") { preselect() }.font(.caption).buttonStyle(.plain).foregroundStyle(Theme.violet)
            }
            Spacer()
            if working { ProgressView().controlSize(.small); Text("Removing…").font(.caption).foregroundStyle(.secondary) }
            if finished {
                Button("Done") { dismiss() }.keyboardShortcut(.defaultAction)
            } else {
                Button("Cancel") { dismiss() }.disabled(working)
                Button("Remove \(toRemove.count) duplicate\(toRemove.count == 1 ? "" : "s")") { run() }
                    .buttonStyle(.borderedProminent).tint(.red)
                    .keyboardShortcut(.defaultAction)
                    .disabled(working || toRemove.isEmpty)
            }
        }
        .padding(16)
    }

    // MARK: Logic

    /// Default: in each group keep the first (shortest-named) site, mark the rest.
    private func preselect() {
        var set = Set<String>()
        for group in groups {
            for site in group.sites.dropFirst() { set.insert(site.cell(["name"])) }
        }
        toRemove = set
    }

    private func toggle(_ name: String) {
        if toRemove.contains(name) { toRemove.remove(name) } else { toRemove.insert(name) }
    }

    private func run() {
        let victims: [(name: String, path: String)] = groups
            .flatMap(\.sites)
            .filter { toRemove.contains($0.cell(["name"])) }
            .map { (name: $0.cell(["name"]), path: $0.cell(["path"])) }
        working = true
        Task {
            removedCount = await store.removeLocalSites(victims)
            working = false
            finished = true
        }
    }

    private func centered<C: View>(@ViewBuilder _ content: () -> C) -> some View {
        VStack { Spacer(); content(); Spacer() }.frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
