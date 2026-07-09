import SwiftUI

/// One group of settings. The ⌘, window stacks them all in a single form; the
/// sidebar's Settings page shows them one at a time.
enum SettingsGroup: String, CaseIterable, Identifiable {
    case tlds = "TLDs"
    case dumps = "Dumps"
    case dply = "dply"
    case binary = "dpl binary"

    var id: String { rawValue }

    var systemImage: String {
        switch self {
        case .tlds: return "globe"
        case .dumps: return "ladybug"
        case .dply: return "cloud"
        case .binary: return "terminal"
        }
    }

    var blurb: String {
        switch self {
        case .tlds: return "Domains your sites answer on"
        case .dumps: return "Where dumps open"
        case .dply: return "Connect to the dply deployment platform"
        case .binary: return "Path to the CLI this app drives"
        }
    }

    @MainActor @ViewBuilder
    var section: some View {
        switch self {
        case .tlds: TldSettings()
        case .dumps: DumpsSettings()
        case .dply: DplySettings()
        case .binary: BinarySettings()
        }
    }
}

/// Settings: the dply host to target and an optional explicit path to the
/// `dpl` binary (otherwise auto-resolved from the workspace build or `$PATH`).
/// This is the ⌘, window; `SettingsPageView` is the same content in the sidebar.
struct SettingsView: View {
    @EnvironmentObject var store: Store

    var body: some View {
        Form {
            ForEach(SettingsGroup.allCases) { $0.section }
        }
        .formStyle(.grouped)
        .frame(width: 480, height: 440)
        .task { await store.loadTlds() }
    }
}

/// Settings as a full-width sidebar page: the same groups as ⌘, in one form.
struct SettingsPage: View {
    @EnvironmentObject var store: Store

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 12) {
                GradientTile(systemImage: "gearshape", size: 40)
                VStack(alignment: .leading, spacing: 2) {
                    Text("Settings").font(.title2.weight(.bold))
                    Text("Applies to this app and the `dpl` CLI it drives")
                        .font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
            }
            .padding(.horizontal, 18)
            .padding(.top, 18)

            Form {
                ForEach(SettingsGroup.allCases) { $0.section }
            }
            .formStyle(.grouped)
        }
        .task { await store.loadTlds() }
    }
}

// MARK: Groups

private struct TldSettings: View {
    @EnvironmentObject var store: Store
    @State private var newTld = ""

    var body: some View {
        Section("TLDs") {
            ForEach(store.tlds, id: \.self) { tld in
                HStack {
                    Text(".\(tld)")
                    if tld == store.tlds.first {
                        Text("primary").font(.caption2).foregroundStyle(.secondary)
                    }
                    Spacer()
                    if store.tlds.count > 1 {
                        Button(role: .destructive) {
                            Task { await store.removeTld(tld) }
                        } label: { Image(systemName: "minus.circle") }
                            .buttonStyle(.borderless)
                    }
                }
            }
            HStack {
                TextField("add a TLD, e.g. localhost", text: $newTld)
                    .textFieldStyle(.roundedBorder)
                    .onSubmit(addTld)
                Button("Add", action: addTld).disabled(newTld.isEmpty)
            }
            Text("Sites answer on every TLD. After adding one, run `dpl setup` so it resolves.")
                .font(.caption).foregroundStyle(.secondary)
        }
    }

    private func addTld() {
        let name = newTld.trimmingCharacters(in: .whitespaces)
        guard !name.isEmpty else { return }
        newTld = ""
        Task { await store.addTld(name) }
    }
}

private struct DumpsSettings: View {
    @EnvironmentObject var store: Store

    var body: some View {
        Section("Dumps") {
            Picker("Open dumps in", selection: $store.dumpsEditor) {
                Text("VS Code").tag("code")
                Text("Cursor").tag("cursor")
                Text("PhpStorm").tag("phpstorm")
                Text("Sublime Text").tag("subl")
                Text("System default").tag("system")
            }
            Text("Clicking a dump's file:line opens it here. `dumps($x)` in any served .test site streams to the Dumps panel.")
                .font(.caption).foregroundStyle(.secondary)
        }
    }
}

/// dply is opt-in: off, the app is a purely local dev environment. On, the
/// platform surfaces (Edge Sites, Server Sites, Servers, Account) appear.
private struct DplySettings: View {
    @EnvironmentObject var store: Store

    var body: some View {
        Section("dply") {
            Toggle(isOn: $store.dplyEnabled) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Enable dply integration")
                    Text("Adds Edge Sites, Server Sites, Servers, and Account to the sidebar so you can deploy and manage projects on dply.")
                        .font(.caption).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }

            if store.dplyEnabled {
                TextField("Host", text: $store.host, prompt: Text("https://dply.io (leave blank for default)"))
                    .textFieldStyle(.roundedBorder)
                Text("Targets a specific dply instance, e.g. https://dplyi.test for local. Blank uses the CLI's stored default.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                HStack(spacing: 6) {
                    Image(systemName: store.isAuthenticated ? "checkmark.circle.fill" : "person.crop.circle.badge.questionmark")
                        .foregroundStyle(store.isAuthenticated ? Theme.live : .secondary)
                    Text(store.isAuthenticated ? "Logged in to \(store.activeHost)" : "Not logged in")
                        .font(.caption)
                    Spacer()
                    Button("Open Account") { store.section = .account }
                }
            } else {
                Text("Everything local — sites, PHP, databases, mail, dumps — works without this.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
        .task { if store.dplyEnabled { await store.refreshAccount() } }
    }
}

private struct BinarySettings: View {
    @EnvironmentObject var store: Store
    @State private var resolved = ""

    var body: some View {
        Section("dpl binary") {
            TextField("Path", text: $store.binaryPath, prompt: Text("auto-detect"))
                .textFieldStyle(.roundedBorder)
            HStack {
                Button("Detect") { detect() }
                if !resolved.isEmpty {
                    Text(resolved).font(.caption).foregroundStyle(.secondary)
                        .lineLimit(1).truncationMode(.middle)
                }
            }
            Text("Leave blank to use the workspace build (target/debug or release) or `dpl` on your PATH.")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .task { detect() }
    }

    private func detect() {
        resolved = (try? store.cli.resolveBinary()) ?? "not found"
    }
}
