import SwiftUI

/// The **PHP** page: which version serves your sites, what else is installed,
/// and the rest of the Homebrew catalog — install, upgrade, repair, or remove
/// without typing a `brew` command.
///
/// Installed and available versions are separate sections rather than one list
/// with a status badge on every row: the grouping *is* the status, so the rows
/// only carry what differs.
struct PhpPage: View {
    @EnvironmentObject var store: Store
    @State private var action: PhpAction?
    @State private var showOlder = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                header
                if store.phpCatalog.isEmpty {
                    loading
                } else {
                    activeCard
                    installedSection
                    availableSection
                    tips
                }
            }
            .padding(18)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .defaultScrollAnchor(.top)
        .task { await store.loadPhpCatalog() }
        .sheet(item: $action) { act in PhpManagerSheet(action: act).environmentObject(store) }
    }

    // MARK: Header

    private var header: some View {
        HStack(spacing: 12) {
            GradientTile(systemImage: "chevron.left.forwardslash.chevron.right", size: 40)
            VStack(alignment: .leading, spacing: 2) {
                Text("PHP").font(.title2.weight(.bold))
                Text("Install, upgrade, repair or remove — no Homebrew commands to type.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
            Button { Task { await store.loadPhpCatalog() } } label: { Image(systemName: "arrow.clockwise") }
                .help("Rescan Homebrew")
        }
    }

    private var loading: some View {
        HStack(spacing: 8) {
            ProgressView().controlSize(.small)
            Text("Scanning Homebrew…").foregroundStyle(.secondary)
        }
        .padding(.top, 30)
        .frame(maxWidth: .infinity)
    }

    // MARK: Active version

    /// The version every site uses unless it pins its own — the one fact this
    /// page exists to answer, so it gets the top card rather than a list row.
    @ViewBuilder
    private var activeCard: some View {
        if let active = catalog.first(where: { $0.active }) {
            HStack(spacing: 14) {
                GradientTile(systemImage: "bolt.fill", size: 38)
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 8) {
                        Text("PHP \(active.version)").font(.title3.weight(.semibold))
                        Text(active.fullVersion).font(.caption.monospaced()).foregroundStyle(.secondary)
                    }
                    Text("Serves every site that doesn't pin its own version")
                        .font(.caption).foregroundStyle(.secondary)
                    if let path = binary(for: active.version) {
                        Text(path)
                            .font(.caption2.monospaced()).foregroundStyle(.tertiary)
                            .lineLimit(1).truncationMode(.middle).textSelection(.enabled)
                    }
                }
                Spacer(minLength: 0)
                Button {
                    store.section = .extensions
                } label: {
                    Label("Extensions", systemImage: "puzzlepiece.extension")
                }
                .help("Manage extensions and OPcache for PHP \(active.version)")
            }
            .cardSurface()
        }
    }

    // MARK: Installed

    private var installedSection: some View {
        DetailSection(title: "Installed · \(installed.count)") {
            VStack(spacing: 0) {
                ForEach(Array(installed.enumerated()), id: \.element.version) { i, entry in
                    if i > 0 { Divider().opacity(0.5) }
                    installedRow(entry)
                }
            }
        }
    }

    private func installedRow(_ entry: PhpEntry) -> some View {
        HStack(spacing: 12) {
            Text(entry.version)
                .font(.callout.weight(.semibold).monospacedDigit())
                .frame(width: 34, alignment: .leading)
                .foregroundStyle(entry.active ? AnyShapeStyle(Theme.brand) : AnyShapeStyle(.primary))

            VStack(alignment: .leading, spacing: 2) {
                Text(entry.fullVersion.isEmpty ? entry.formula : entry.fullVersion)
                    .font(.caption.monospaced()).foregroundStyle(.secondary)
                if let path = binary(for: entry.version) {
                    Text(path)
                        .font(.caption2.monospaced()).foregroundStyle(.tertiary)
                        .lineLimit(1).truncationMode(.middle)
                }
            }
            Spacer(minLength: 8)

            // Only a broken keg needs a badge — "installed" is what this section means.
            if entry.broken {
                Text("Needs repair")
                    .font(.caption2.weight(.medium))
                    .padding(.horizontal, 8).padding(.vertical, 3)
                    .background(Color.orange.opacity(0.15), in: Capsule())
                    .foregroundStyle(.orange)
                Button("Repair") { action = .repair(entry.version) }
                    .buttonStyle(.borderedProminent).tint(.orange).controlSize(.small)
            } else if entry.active {
                Label("Active", systemImage: "checkmark.circle.fill")
                    .font(.caption2.weight(.medium))
                    .padding(.horizontal, 8).padding(.vertical, 3)
                    .background(Theme.live.opacity(0.15), in: Capsule())
                    .foregroundStyle(Theme.live)
            } else {
                // A quiet action, but in the app's accent — `.link` renders in
                // system blue and fights the theme.
                Button("Set default") { Task { await store.usePhp(version: entry.version, site: nil) } }
                    .buttonStyle(.plain)
                    .font(.caption.weight(.medium))
                    .foregroundStyle(Theme.violet)
            }

            Menu {
                if !entry.active && !entry.broken {
                    Button { Task { await store.usePhp(version: entry.version, site: nil) } } label: {
                        Label("Set as default", systemImage: "checkmark.circle")
                    }
                    Divider()
                }
                Button { action = .upgrade(entry.version) } label: { Label("Upgrade", systemImage: "arrow.up.circle") }
                Button { action = .repair(entry.version) } label: { Label("Repair", systemImage: "wrench.and.screwdriver") }
                if !entry.active {
                    Divider()
                    Button(role: .destructive) { action = .uninstall(entry.version) } label: {
                        Label("Uninstall", systemImage: "trash")
                    }
                }
            } label: {
                Image(systemName: "ellipsis")
            }
            .menuStyle(.borderlessButton)
            .frame(width: 22)
            .controlSize(.small)
        }
        .padding(.vertical, 8)
    }

    // MARK: Available

    /// Everything else Homebrew can install, folded away — a fresh machine
    /// shouldn't open onto seven Install buttons for PHP 5.6.
    @ViewBuilder
    private var availableSection: some View {
        if !available.isEmpty {
            DetailSection(title: "Available") {
                VStack(alignment: .leading, spacing: 0) {
                    Button {
                        withAnimation(.easeInOut(duration: 0.15)) { showOlder.toggle() }
                    } label: {
                        HStack(spacing: 6) {
                            Image(systemName: "chevron.right")
                                .font(.caption2.weight(.semibold))
                                .rotationEffect(.degrees(showOlder ? 90 : 0))
                            Text("\(available.count) more version\(available.count == 1 ? "" : "s") to install")
                                .font(.callout)
                            Spacer()
                            Text(available.map(\.version).joined(separator: ", "))
                                .font(.caption2.monospaced()).foregroundStyle(.tertiary)
                                .lineLimit(1).truncationMode(.tail)
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)

                    if showOlder {
                        ForEach(Array(available.enumerated()), id: \.element.version) { i, entry in
                            if i > 0 { Divider().opacity(0.5) }
                            availableRow(entry)
                        }
                        .padding(.top, 4)
                    }
                }
            }
        }
    }

    private func availableRow(_ entry: PhpEntry) -> some View {
        HStack(spacing: 12) {
            Text(entry.version)
                .font(.callout.weight(.semibold).monospacedDigit())
                .frame(width: 34, alignment: .leading)
                .foregroundStyle(.secondary)
            Text(entry.formula)
                .font(.caption.monospaced()).foregroundStyle(.tertiary)
            Spacer(minLength: 8)
            Button("Install") { action = .install(entry.version) }
                .buttonStyle(.bordered).controlSize(.small)
        }
        .padding(.vertical, 7)
    }

    // MARK: Tips

    private var tips: some View {
        DetailSection(title: "How it works") {
            VStack(alignment: .leading, spacing: 8) {
                Label("Installs run `brew install php@x.y`; older lines auto-tap shivammathur/php.", systemImage: "arrow.down.circle")
                Label("“Repair” reinstalls a broken keg (missing binary / opt link).", systemImage: "wrench.and.screwdriver")
                Label("Each installed version runs its own php-fpm pool.", systemImage: "bolt.fill")
                Label("Pin a single site from its detail pane (Local Sites → pick a site → PHP).", systemImage: "pin")
            }
            .font(.callout)
            .labelStyle(.titleAndIcon)
        }
    }

    // MARK: Data

    /// One row of `dpl php available`.
    private struct PhpEntry {
        let version: String
        let formula: String
        let fullVersion: String
        let installed: Bool
        let active: Bool
        let broken: Bool
    }

    private var catalog: [PhpEntry] {
        store.phpCatalog.map {
            PhpEntry(
                version: $0.cell(["version"]),
                formula: $0.cell(["formula"]),
                fullVersion: $0.cell(["installed_version"]),
                installed: $0.dig("installed") == .bool(true),
                active: $0.dig("active") == .bool(true),
                broken: $0.dig("broken") == .bool(true)
            )
        }
    }

    private var installed: [PhpEntry] { catalog.filter(\.installed) }
    private var available: [PhpEntry] { catalog.filter { !$0.installed } }

    /// The binary path for a version, from `dpl php` (the catalog omits it).
    private func binary(for version: String) -> String? {
        store.phpVersions
            .first { $0.cell(["version"]) == version }
            .map { $0.cell(["binary"]) }
            .flatMap { $0.isEmpty ? nil : $0 }
    }
}
