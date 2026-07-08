import SwiftUI

/// Detail pane for a provisioned server: the summary fields from the list row
/// plus a live firewall snapshot fetched on appear.
struct ServerDetailView: View {
    @EnvironmentObject var store: Store
    let server: Row

    @State private var firewall: [Row] = []
    @State private var loading = true

    private var id: String { server.first(["id", "name"]) ?? "" }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                HStack(alignment: .firstTextBaseline, spacing: 10) {
                    Text(server.first(["name"]) ?? id)
                        .font(.title2.weight(.semibold))
                    StatusBadge(status: server.cell(["status"]))
                    Spacer()
                }

                DetailSection(title: "Overview") {
                    KeyValueView(row: server, fields: [
                        ("ID", ["id"]),
                        ("Provider", ["provider"]),
                        ("Region", ["region"]),
                        ("IP", ["ip_address", "ip"]),
                        ("Status", ["status"]),
                        ("Updated", ["updated_at"]),
                    ])
                }

                DetailSection(title: "Firewall") {
                    if loading {
                        ProgressView().controlSize(.small)
                    } else {
                        RowsTable(rows: firewall, columns: [
                            ("Action", ["action"]),
                            ("Proto", ["protocol"]),
                            ("Port", ["port", "port_range"]),
                            ("Source", ["source"]),
                            ("Comment", ["comment"]),
                        ])
                    }
                }
            }
            .padding(18)
        }
        .task(id: id) { await load() }
    }

    private func load() async {
        loading = true
        let cli = store.cli
        let sid = id
        // `servers:firewall --json` returns an object with a `rules` array;
        // pull it out for the table.
        if let value = await store.background({ try cli.json(["dply", "servers:firewall", sid]) }) {
            if let rules = value.objectValue?["rules"]?.arrayValue {
                firewall = rules.compactMap { $0.objectValue.map(Row.init) }
            } else if let arr = value.arrayValue {
                firewall = arr.compactMap { $0.objectValue.map(Row.init) }
            }
        }
        loading = false
    }
}
