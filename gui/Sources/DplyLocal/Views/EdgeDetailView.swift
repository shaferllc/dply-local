import SwiftUI

/// Detail pane for a single edge site: overview, deploy/purge/logs actions,
/// and deployments / environment / domains tables. Everything is fetched via
/// the `dpl` CLI when the pane appears.
struct EdgeDetailView: View {
    @EnvironmentObject var store: Store
    let siteID: String

    @State private var site: Row?
    @State private var deployments: [Row] = []
    @State private var env: [Row] = []
    @State private var domains: [Row] = []
    @State private var loading = true
    @State private var busy = false
    @State private var showLogs = false

    private var liveURL: String { site?.first(["live_url", "hostname"]) ?? "" }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                header
                actions

                DetailSection(title: "Overview") {
                    if let site {
                        KeyValueView(row: site, fields: [
                            ("ID", ["id"]),
                            ("Status", ["status"]),
                            ("Backend", ["edge_backend", "backend"]),
                            ("Runtime", ["runtime_mode"]),
                            ("Framework", ["build.framework", "framework"]),
                            ("Repo", ["source.repo"]),
                            ("Branch", ["source.branch"]),
                            ("Build cmd", ["build.command"]),
                            ("Output dir", ["build.output"]),
                            ("Active deploy", ["active_deployment_id"]),
                            ("Created", ["created_at"]),
                            ("Updated", ["updated_at"]),
                        ])
                    } else if loading {
                        ProgressView().controlSize(.small)
                    }
                }

                DetailSection(title: "Deployments") {
                    RowsTable(rows: deployments, columns: [
                        ("ID", ["id"]),
                        ("Status", ["status"]),
                        ("Commit", ["git_commit"]),
                        ("Branch", ["git_branch"]),
                        ("Published", ["published_at", "created_at"]),
                    ])
                }

                DetailSection(title: "Environment") {
                    RowsTable(rows: env, columns: [
                        ("Key", ["key"]),
                        ("Scope", ["scope"]),
                        ("Updated", ["updated_at"]),
                    ])
                }

                DetailSection(title: "Domains") {
                    RowsTable(rows: domains, columns: [
                        ("Hostname", ["hostname"]),
                        ("Status", ["status"]),
                        ("Verified", ["verified_at"]),
                    ])
                }
            }
            .padding(18)
        }
        .task(id: siteID) { await load() }
        .sheet(isPresented: $showLogs) {
            LogsSheet(siteID: siteID, title: site?.first(["name"]) ?? siteID)
                .environmentObject(store)
        }
    }

    private var header: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Text(site?.first(["name"]) ?? siteID)
                .font(.title2.weight(.semibold))
            StatusBadge(status: site?.cell(["status"]) ?? "")
            Spacer()
        }
    }

    private var actions: some View {
        HStack(spacing: 10) {
            Button {
                Task { busy = true; await store.deployEdge(siteID); await load(); busy = false }
            } label: {
                Label("Deploy", systemImage: "arrow.up.circle.fill")
            }
            .disabled(busy)

            Button {
                Task { busy = true; await store.purgeEdge(siteID); busy = false }
            } label: {
                Label("Purge cache", systemImage: "trash")
            }
            .disabled(busy)

            Button { showLogs = true } label: {
                Label("Logs", systemImage: "text.alignleft")
            }

            if !liveURL.isEmpty {
                Button { store.openURL(liveURL) } label: {
                    Label("Open", systemImage: "safari")
                }
            }

            if busy { ProgressView().controlSize(.small) }
            Spacer()
        }
    }

    private func load() async {
        loading = true
        let cli = store.cli
        let id = siteID
        if let s = await store.background({ try cli.object(["dply", "edge:show", id]) }) {
            site = s
        }
        deployments = await store.background({ try cli.rows(["dply", "edge:deployments", id]) }) ?? []
        env = await store.background({ try cli.rows(["dply", "edge:env", id]) }) ?? []
        domains = await store.background({ try cli.rows(["dply", "edge:domains", id]) }) ?? []
        loading = false
    }
}
