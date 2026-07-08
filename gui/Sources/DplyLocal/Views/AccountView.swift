import SwiftUI

/// Middle column for the Account section: auth state + login/logout.
struct AccountView: View {
    @EnvironmentObject var store: Store
    @State private var showLogin = false

    var body: some View {
        List {
            Section("Connection") {
                LabeledContent("Host", value: store.activeHost)
                LabeledContent("Status") {
                    HStack(spacing: 6) {
                        Circle()
                            .fill(store.isAuthenticated ? .green : .secondary)
                            .frame(width: 8, height: 8)
                        Text(store.isAuthenticated ? "Logged in" : "Not logged in")
                    }
                }
            }

            Section {
                if store.isAuthenticated {
                    Button(role: .destructive) {
                        Task { await logout() }
                    } label: {
                        Label("Log out", systemImage: "rectangle.portrait.and.arrow.right")
                    }
                } else {
                    Button {
                        showLogin = true
                    } label: {
                        Label("Log in to dply…", systemImage: "person.badge.key")
                    }
                }
            }
        }
        .sheet(isPresented: $showLogin) { LoginSheet().environmentObject(store) }
    }

    private func logout() async {
        let cli = store.cli
        _ = await store.background { try cli.runRaw(["dply", "logout"]) }
        await store.refreshAccount()
    }
}

/// Detail column for the Account section: the operator profile from `whoami`.
struct AccountDetailView: View {
    @EnvironmentObject var store: Store

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                Text("Account")
                    .font(.title2.weight(.semibold))

                if let account = store.account, store.isAuthenticated {
                    DetailSection(title: "Profile") {
                        KeyValueView(row: account, fields: [
                            ("Host", ["host"]),
                            ("Operator", ["profile.operator.name"]),
                            ("Email", ["profile.operator.email"]),
                            ("Role", ["profile.operator.role"]),
                            ("Organization", ["profile.organization.name"]),
                            ("Plan", ["profile.organization.plan"]),
                            ("Since", ["updated_at"]),
                        ])
                    }
                } else {
                    ContentUnavailableView(
                        "Not logged in",
                        systemImage: "person.crop.circle.badge.xmark",
                        description: Text("Use the Log in button to authenticate with dply.")
                    )
                }
            }
            .padding(18)
        }
    }
}
