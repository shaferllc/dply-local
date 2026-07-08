import SwiftUI

/// Settings: the dply host to target and an optional explicit path to the
/// `dpl` binary (otherwise auto-resolved from the workspace build or `$PATH`).
struct SettingsView: View {
    @EnvironmentObject var store: Store
    @State private var resolved = ""
    @State private var newTld = ""

    var body: some View {
        Form {
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

            Section("dply host") {
                TextField("Host", text: $store.host, prompt: Text("https://dply.io (leave blank for default)"))
                    .textFieldStyle(.roundedBorder)
                Text("Targets a specific dply instance, e.g. https://dplyi.test for local. Blank uses the CLI's stored default.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

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
        }
        .formStyle(.grouped)
        .frame(width: 480, height: 440)
        .task { detect(); await store.loadTlds() }
    }

    private func detect() {
        resolved = (try? store.cli.resolveBinary()) ?? "not found"
    }

    private func addTld() {
        let name = newTld.trimmingCharacters(in: .whitespaces)
        guard !name.isEmpty else { return }
        newTld = ""
        Task { await store.addTld(name) }
    }
}
