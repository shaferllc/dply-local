import SwiftUI

/// Detail pane for a server-hosted site: overview + deploy action + recent
/// deployments.
struct SiteDetailView: View {
    @EnvironmentObject var store: Store
    let siteID: String

    @State private var site: Row?
    @State private var deployments: [Row] = []
    @State private var loading = true
    @State private var busy = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                HStack(alignment: .firstTextBaseline, spacing: 10) {
                    Text(site?.first(["name"]) ?? siteID)
                        .font(.title2.weight(.semibold))
                    StatusBadge(status: site?.cell(["status"]) ?? "")
                    Spacer()
                }

                HStack(spacing: 10) {
                    Button {
                        Task { busy = true; await store.deploySite(siteID); await load(); busy = false }
                    } label: {
                        Label("Deploy", systemImage: "arrow.up.circle.fill")
                    }
                    .disabled(busy)
                    if busy { ProgressView().controlSize(.small) }
                    Spacer()
                }

                DetailSection(title: "Overview") {
                    if let site {
                        KeyValueView(row: site, fields: [
                            ("Slug", ["slug"]),
                            ("Server", ["server_name"]),
                            ("Runtime", ["runtime"]),
                            ("Runtime ver", ["runtime_version"]),
                            ("Status", ["status"]),
                            ("SSL", ["ssl_status"]),
                            ("Repository", ["git_repository_url"]),
                            ("Branch", ["git_branch"]),
                            ("Last deploy", ["last_deploy_at"]),
                        ])
                    } else if loading {
                        ProgressView().controlSize(.small)
                    }
                }

                DetailSection(title: "Deployments") {
                    RowsTable(rows: deployments, columns: [
                        ("ID", ["id"]),
                        ("Status", ["status"]),
                        ("Commit", ["commit", "git_commit"]),
                        ("Started", ["started_at"]),
                        ("Finished", ["finished_at"]),
                    ])
                }
            }
            .padding(18)
        }
        .task(id: siteID) { await load() }
    }

    private func load() async {
        loading = true
        let cli = store.cli
        let id = siteID
        if let s = await store.background({ try cli.object(["dply", "sites:show", id]) }) {
            site = s
        }
        deployments = await store.background({ try cli.rows(["dply", "sites:deployments", id]) }) ?? []
        loading = false
    }
}
