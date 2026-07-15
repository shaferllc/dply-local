import SwiftUI

/// Keel Cloud account pane: token-paste login (Keel Cloud has no device flow —
/// tokens are minted in the web UI at /tokens), or the signed-in summary.
struct KeelAccountView: View {
    @EnvironmentObject var store: Store
    @State private var token = ""
    @State private var url = ""
    @State private var busy = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                HStack(spacing: 12) {
                    GradientTile(systemImage: "cloud", size: 44, active: store.keelAccount != nil)
                    VStack(alignment: .leading, spacing: 3) {
                        Text("Keel Cloud").font(.title2.weight(.bold))
                        Text(store.keelAccount == nil ? "not logged in" : "connected")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                    Spacer()
                }

                if let account = store.keelAccount {
                    DetailSection(title: "Account") {
                        KeyValueView(row: account, fields: [
                            ("Name", ["name"]),
                            ("Email", ["email"]),
                            ("Team", ["team_id"]),
                            ("Plan", ["plan"]),
                            ("Site limit", ["site_limit"]),
                        ])
                    }
                    HStack {
                        Button(role: .destructive) {
                            Task { busy = true; await store.keelLogout(); busy = false }
                        } label: { Label("Log out", systemImage: "rectangle.portrait.and.arrow.right") }
                        .disabled(busy)
                        Spacer()
                    }
                } else {
                    DetailSection(title: "Log in") {
                        VStack(alignment: .leading, spacing: 10) {
                            Text("Paste a personal access token — mint one in the Keel Cloud web UI under **/tokens**.")
                                .font(.caption).foregroundStyle(.secondary)
                            SecureField("keel_…", text: $token)
                                .textFieldStyle(.roundedBorder)
                            TextField("Cloud URL (blank = app.keeljs.cloud)", text: $url)
                                .textFieldStyle(.roundedBorder)
                            HStack(spacing: 10) {
                                Button {
                                    Task {
                                        busy = true
                                        await store.keelLogin(token: token, url: url)
                                        busy = false
                                        if store.keelAccount != nil { token = "" }
                                    }
                                } label: { Label("Log in", systemImage: "key") }
                                .buttonStyle(.borderedProminent).tint(Theme.violet)
                                .disabled(busy || token.trimmingCharacters(in: .whitespaces).isEmpty)
                                if busy { ProgressView().controlSize(.small) }
                            }
                        }
                    }
                }
            }
            .padding(18)
        }
        .task { if store.keelAccount == nil { await store.refreshKeelAccount() } }
    }
}

/// Detail pane for one Keel Cloud site: hostnames, deploy actions, recent deploys.
struct KeelSiteDetailView: View {
    @EnvironmentObject var store: Store
    let site: Row
    @State private var deploys: [Row] = []
    @State private var busy = false

    private var id: String { site.cell(["id"]) }
    private var prodURL: String {
        let host = site.first(["custom_hostname"]) ?? site.cell(["prod_hostname"])
        return "https://\(host)"
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                HStack(spacing: 12) {
                    GradientTile(systemImage: "sailboat", size: 44, active: true)
                    VStack(alignment: .leading, spacing: 3) {
                        Text(site.cell(["name"])).font(.title2.weight(.bold))
                        Text(site.cell(["status"])).font(.caption).foregroundStyle(.secondary)
                    }
                    Spacer()
                    StatusBadge(status: site.cell(["status"]))
                }

                HStack(spacing: 10) {
                    Button { store.openURL(prodURL) } label: { Label("Open", systemImage: "safari") }
                        .buttonStyle(.borderedProminent).tint(Theme.violet).fixedSize()
                    Button {
                        Task { busy = true; await store.keelDeploy(id: id); deploys = await store.keelDeploys(id: id); busy = false }
                    } label: { Label("Publish", systemImage: "paperplane") }
                        .disabled(busy).fixedSize()
                    Button {
                        Task { busy = true; await store.keelDeploy(id: id, preview: true); deploys = await store.keelDeploys(id: id); busy = false }
                    } label: { Label("Preview deploy", systemImage: "eye") }
                        .disabled(busy).fixedSize()
                    if busy { ProgressView().controlSize(.small) }
                    Spacer()
                }

                DetailSection(title: "Overview") {
                    KeyValueView(row: site, fields: [
                        ("Name", ["name"]),
                        ("Preset", ["preset"]),
                        ("Status", ["status"]),
                        ("Production", ["prod_hostname"]),
                        ("Preview", ["preview_hostname"]),
                        ("Custom domain", ["custom_hostname"]),
                        ("Domain status", ["custom_hostname_status"]),
                        ("Git", ["git_html_url", "git_url"]),
                    ])
                }

                DetailSection(title: "Recent deploys") {
                    if deploys.isEmpty {
                        Text("No deploys yet.").font(.caption).foregroundStyle(.secondary)
                    } else {
                        VStack(alignment: .leading, spacing: 6) {
                            ForEach(deploys) { d in
                                HStack(spacing: 8) {
                                    Text("#\(d.cell(["id"]))").font(.caption.monospaced()).foregroundStyle(.tertiary)
                                    Text(d.cell(["target", "environment"])).font(.caption)
                                    StatusBadge(status: d.cell(["status"]))
                                    Spacer()
                                    Text(d.cell(["created_at"])).font(.caption2).foregroundStyle(.tertiary)
                                }
                            }
                        }
                    }
                }
            }
            .padding(18)
        }
        .task(id: id) { deploys = await store.keelDeploys(id: id) }
    }
}
