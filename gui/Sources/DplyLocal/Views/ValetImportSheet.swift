import SwiftUI

/// Import an existing Laravel Valet setup: its parked directories and linked
/// sites (names preserved). Pick what to bring over, then import in one pass.
struct ValetImportSheet: View {
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss

    @State private var snapshot: ValetSnapshot?
    @State private var loading = true
    @State private var selectedParked: Set<String> = []
    @State private var selectedLinked: Set<String> = []
    @State private var matchTld = true

    @State private var importing = false
    @State private var done = 0
    @State private var total = 0
    @State private var finished = false
    @State private var removed = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Divider()
            content
            Divider()
            footer
        }
        .frame(width: 560, height: 620)
        .task {
            let snap = await store.valetSnapshot()
            snapshot = snap
            selectedParked = Set(snap.parked)
            selectedLinked = Set(snap.linked.filter(\.exists).map(\.name))
            loading = false
        }
    }

    // MARK: Header

    private var header: some View {
        HStack(spacing: 12) {
            GradientTile(systemImage: "arrow.down.doc", size: 40)
            VStack(alignment: .leading, spacing: 2) {
                Text("Import from Laravel Valet").font(.title3.weight(.bold))
                if let s = snapshot, s.installed {
                    Text("Found Valet · .\(s.tld) · \(s.parked.count) parked · \(s.linked.count) linked")
                        .font(.caption).foregroundStyle(.secondary)
                } else {
                    Text("Bring your parked folders and linked sites into Dply Local")
                        .font(.caption).foregroundStyle(.secondary)
                }
            }
            Spacer()
        }
        .padding(16)
    }

    // MARK: Content

    @ViewBuilder private var content: some View {
        if loading {
            centered { ProgressView("Reading Valet config…").controlSize(.small) }
        } else if let snap = snapshot, snap.installed {
            if finished {
                centered {
                    VStack(spacing: 10) {
                        Image(systemName: removed ? "trash.circle.fill" : "checkmark.seal.fill")
                            .font(.largeTitle).foregroundStyle(removed ? .orange : .green)
                        Text("\(removed ? "Removed" : "Imported") \(done) site\(done == 1 ? "" : "s")").font(.headline)
                        Text(removed ? "They're no longer served." : "They're now in Local Sites.")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                }
            } else {
                List {
                    if !snap.parked.isEmpty {
                        Section {
                            ForEach(snap.parked, id: \.self) { path in
                                selectRow(
                                    on: selectedParked.contains(path),
                                    title: (path as NSString).lastPathComponent,
                                    subtitle: path,
                                    exists: true,
                                    toggle: { toggle(path, in: &selectedParked) }
                                )
                            }
                        } header: {
                            sectionHeader("Parked directories", all: snap.parked)
                        }
                    }
                    Section {
                        ForEach(snap.linked) { site in
                            selectRow(
                                on: selectedLinked.contains(site.name),
                                title: "\(site.name).\(snap.tld)",
                                subtitle: site.path,
                                exists: site.exists,
                                toggle: { if site.exists { toggle(site.name, in: &selectedLinked) } }
                            )
                        }
                    } header: {
                        sectionHeader("Linked sites", all: snap.linked.filter(\.exists).map(\.name))
                    }
                }
            }
        } else {
            centered {
                VStack(spacing: 8) {
                    Image(systemName: "questionmark.folder").font(.largeTitle).foregroundStyle(.secondary)
                    Text("No Valet install found").font(.headline)
                    Text("Looked in ~/.config/valet and ~/.valet.").font(.caption).foregroundStyle(.secondary)
                }
            }
        }
    }

    private func sectionHeader(_ title: String, all: [String]) -> some View {
        HStack {
            Text(title)
            Spacer()
            Button(allSelected(all) ? "Deselect all" : "Select all") {
                toggleAll(all)
            }
            .font(.caption).buttonStyle(.plain).foregroundStyle(Theme.violet)
        }
    }

    private func selectRow(on: Bool, title: String, subtitle: String, exists: Bool, toggle: @escaping () -> Void) -> some View {
        Button(action: toggle) {
            HStack(spacing: 10) {
                Image(systemName: on ? "checkmark.circle.fill" : "circle")
                    .foregroundStyle(on ? AnyShapeStyle(Theme.violet) : AnyShapeStyle(.secondary))
                VStack(alignment: .leading, spacing: 1) {
                    Text(title).font(.callout.weight(.medium)).foregroundStyle(exists ? .primary : .secondary)
                    Text(exists ? subtitle : "\(subtitle)  —  missing")
                        .font(.system(.caption2, design: .monospaced))
                        .foregroundStyle(exists ? AnyShapeStyle(.secondary) : AnyShapeStyle(.red))
                        .lineLimit(1).truncationMode(.middle)
                }
                Spacer()
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!exists)
    }

    // MARK: Footer

    private var footer: some View {
        HStack {
            if let s = snapshot, s.installed, !finished {
                Toggle("Also switch dpl's primary domain to .\(s.tld)", isOn: $matchTld)
                    .toggleStyle(.checkbox).font(.caption)
            }
            Spacer()
            if importing {
                ProgressView().controlSize(.small)
                Text("\(removed ? "Removing" : "Importing") \(total) site\(total == 1 ? "" : "s")…")
                    .font(.caption).foregroundStyle(.secondary)
            }
            if finished {
                Button("Done") { dismiss() }.keyboardShortcut(.defaultAction)
            } else {
                Button("Cancel") { dismiss() }.disabled(importing)
                Button("Remove") { runImport(remove: true) }
                    .tint(.red)
                    .disabled(importing || selectionCount == 0)
                    .help("Remove the selected sites from Dply Local")
                Button(importLabel) { runImport(remove: false) }
                    .buttonStyle(.borderedProminent).tint(Theme.violet)
                    .keyboardShortcut(.defaultAction)
                    .disabled(importing || selectionCount == 0)
            }
        }
        .padding(16)
    }

    private var selectionCount: Int { selectedParked.count + selectedLinked.count }
    private var importLabel: String { importing ? "Importing…" : "Import \(selectionCount) site\(selectionCount == 1 ? "" : "s")" }

    // MARK: Actions

    private func toggle(_ item: String, in set: inout Set<String>) {
        if set.contains(item) { set.remove(item) } else { set.insert(item) }
    }

    private func allSelected(_ all: [String]) -> Bool {
        !all.isEmpty && all.allSatisfy { selectedParked.contains($0) || selectedLinked.contains($0) }
    }

    /// Select-all / deselect-all for whichever group these items belong to.
    private func toggleAll(_ all: [String]) {
        guard let snap = snapshot else { return }
        let isParked = Set(all) == Set(snap.parked)
        if isParked {
            selectedParked = allSelected(all) ? [] : Set(all)
        } else {
            selectedLinked = allSelected(all) ? [] : Set(all)
        }
    }

    private func runImport(remove: Bool) {
        guard let snap = snapshot else { return }
        let parks = snap.parked.filter { selectedParked.contains($0) }
        let links = snap.linked.filter { selectedLinked.contains($0.name) }
        total = parks.count + links.count
        importing = true
        removed = remove
        Task {
            if remove {
                done = await store.valetRemove(parked: parks, linked: links)
            } else {
                done = await store.valetImport(parked: parks, linked: links.filter(\.exists), matchTld: matchTld, tld: snap.tld)
            }
            importing = false
            finished = true
        }
    }

    private func centered<C: View>(@ViewBuilder _ content: () -> C) -> some View {
        VStack { Spacer(); content(); Spacer() }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
