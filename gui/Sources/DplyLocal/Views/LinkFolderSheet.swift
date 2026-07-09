import SwiftUI

/// "Link a Folder" dialog — customize the site name and optionally secure it,
/// with a live URL preview. Shown after choosing a folder from the ➕ menu.
struct LinkFolderSheet: View {
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss

    let path: String
    @State private var name: String
    @State private var secure = false
    @State private var working = false

    init(path: String) {
        self.path = path
        _name = State(initialValue: (path as NSString).lastPathComponent.lowercased())
    }

    private var tld: String { store.tlds.first ?? "test" }
    private var cleanName: String {
        name.trimmingCharacters(in: .whitespaces).lowercased()
    }
    private var host: String { "\(cleanName).\(tld)" }
    private var url: String { "\(secure ? "https" : "http")://\(host)" }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 10) {
                Image(systemName: "link.badge.plus").foregroundStyle(Theme.brand)
                Text("Link a Folder").font(.headline)
            }

            // Folder path breadcrumb.
            HStack(spacing: 4) {
                Image(systemName: "folder")
                Text(breadcrumb).lineLimit(1).truncationMode(.head)
            }
            .font(.caption).foregroundStyle(.secondary)

            TextField("Domain name", text: $name)
                .textFieldStyle(.roundedBorder)
                .onSubmit(create)
            Text("This site will be available at **\(url)**")
                .font(.caption).foregroundStyle(.secondary)

            Toggle(isOn: $secure) {
                VStack(alignment: .leading, spacing: 1) {
                    Text("Secure \(host) after creation")
                    Text("Trusted HTTPS. Needs the local CA (run Setup once).")
                        .font(.caption2).foregroundStyle(.secondary)
                }
            }
            .toggleStyle(.checkbox)

            HStack {
                if working { ProgressView().controlSize(.small) }
                Spacer()
                Button("Cancel") { dismiss() }.disabled(working)
                Button("Create Link") { create() }
                    .buttonStyle(.borderedProminent).tint(Theme.violet)
                    .keyboardShortcut(.defaultAction)
                    .disabled(working || cleanName.isEmpty)
            }
        }
        .padding(18)
        .frame(width: 460)
    }

    private var breadcrumb: String {
        // Show the trailing path components, home-relative.
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        return path.hasPrefix(home) ? "~" + path.dropFirst(home.count) : path
    }

    private func create() {
        guard !cleanName.isEmpty else { return }
        working = true
        Task {
            await store.linkLocalAs(path: path, name: cleanName, secure: secure)
            working = false
            dismiss()
        }
    }
}
