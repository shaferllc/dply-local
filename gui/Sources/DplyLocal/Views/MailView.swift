import SwiftUI

/// Middle column: the captured-mail inbox (newest first), scoped to a mailbox.
///
/// A mailbox is a project's `MAIL_USERNAME` — see `MailSetupSheet` for why the
/// SMTP username is what identifies a site.
struct MailListView: View {
    @EnvironmentObject var store: Store
    @Binding var selection: String?

    @State private var showSetup = false

    /// The first linked site, so the setup snippets show a real name.
    private var exampleSite: String {
        store.localSites.first { $0.cell(["source"]) == "linked" }
            .map { $0.cell(["name"]) } ?? "blog"
    }

    var body: some View {
        VStack(spacing: 0) {
            mailboxBar
            Divider()
            list
        }
        .sheet(isPresented: $showSetup) {
            MailSetupSheet(site: exampleSite).environmentObject(store)
        }
    }

    // MARK: Mailbox switcher

    private var totalCount: Int {
        store.mailboxes.reduce(0) { $0 + (Int($1.cell(["count"])) ?? 0) }
    }

    @ViewBuilder
    private var mailboxBar: some View {
        HStack(spacing: 8) {
            Menu {
                Button {
                    Task { await store.selectMailbox(nil) }
                } label: {
                    Text("All mailboxes (\(totalCount))")
                }
                if !store.mailboxes.isEmpty { Divider() }
                ForEach(store.mailboxes, id: \.id) { box in
                    let name = box.cell(["name"])
                    Button {
                        Task { await store.selectMailbox(name) }
                    } label: {
                        Text("\(Store.mailboxLabel(name)) (\(box.cell(["count"])))")
                    }
                }
            } label: {
                HStack(spacing: 4) {
                    Image(systemName: "tray.2")
                    Text(store.mailboxFilter.map(Store.mailboxLabel) ?? "All mailboxes")
                        .lineLimit(1)
                }
            }
            .menuStyle(.borderlessButton)
            .fixedSize()

            Spacer()

            Menu {
                Button("Plain text") { Task { await store.sendTestMail(mailbox: nil, html: false) } }
                Button("HTML (with a blocked tracking pixel)") {
                    Task { await store.sendTestMail(mailbox: nil, html: true) }
                }
                if let box = store.mailboxFilter, box != Store.unattributedMailbox {
                    Divider()
                    Button("HTML as `\(box)`") {
                        Task { await store.sendTestMail(mailbox: box, html: true) }
                    }
                }
            } label: {
                Image(systemName: "paperplane")
            }
            .menuStyle(.borderlessButton).fixedSize()
            .help("Send a test message into the sink")

            Button { showSetup = true } label: {
                Image(systemName: "questionmark.circle")
            }
            .buttonStyle(.borderless)
            .help("How to point your app's mailer at dpl")
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .font(.callout)
    }

    // MARK: Messages

    @ViewBuilder
    private var list: some View {
        List(selection: $selection) {
            if store.mailMessages.isEmpty && !store.isLoading {
                emptyState
            }
            ForEach(store.mailMessages) { row in
                let mailbox = row.cell(["mailbox"])
                let attachments = Int(row.cell(["attachments"])) ?? 0
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 6) {
                        Text(nonEmpty(row.cell(["subject"]), "(no subject)"))
                            .font(.body.weight(.semibold))
                            .lineLimit(1)
                        if attachments > 0 {
                            Image(systemName: "paperclip")
                                .font(.caption2).foregroundStyle(.secondary)
                        }
                    }
                    let preview = row.cell(["preview"])
                    if !preview.isEmpty {
                        Text(preview).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    }
                    HStack(spacing: 6) {
                        Image(systemName: "arrow.right").font(.caption2).foregroundStyle(.secondary)
                        Text(nonEmpty(row.cell(["to"]), "—"))
                            .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                        // Only worth the pixels when several mailboxes are in view.
                        if store.mailboxFilter == nil, !mailbox.isEmpty {
                            Text(Store.mailboxLabel(mailbox))
                                .font(.caption2)
                                .padding(.horizontal, 5).padding(.vertical, 1)
                                .background(Theme.violet.opacity(0.15), in: Capsule())
                                .foregroundStyle(Theme.violet)
                        }
                    }
                }
                .padding(.vertical, 3)
                .tag(row.cell(["id"]))
            }
        }
        .searchable(text: $store.mailSearch, placement: .automatic, prompt: "Search mail")
        // Re-run the search server-side: the body text lives in the .eml, not in
        // the rows we already hold.
        .task(id: store.mailSearch) {
            try? await Task.sleep(for: .milliseconds(250)) // debounce typing
            if !Task.isCancelled { await store.loadMail() }
        }
        .overlay { if store.isLoading { ProgressView().controlSize(.small) } }
    }

    @ViewBuilder
    private var emptyState: some View {
        if let filter = store.mailboxFilter {
            ContentUnavailableView {
                Label("No mail in \(Store.mailboxLabel(filter))", systemImage: "tray")
            } description: {
                Text("Other mailboxes may still have messages.")
            } actions: {
                Button("Show all mailboxes") { Task { await store.selectMailbox(nil) } }
            }
        } else {
            ContentUnavailableView {
                Label("Inbox empty", systemImage: "envelope")
            } description: {
                Text("Point your app's mailer at **127.0.0.1:1025** and set `MAIL_USERNAME` to the site's name — that name becomes its mailbox. Any password works.")
            } actions: {
                Button("Show me how") { showSetup = true }
                    .buttonStyle(.borderedProminent)
                Button("Send a test message") {
                    Task { await store.sendTestMail(mailbox: nil, html: true) }
                }
            }
        }
    }

    private func nonEmpty(_ s: String, _ fallback: String) -> String {
        s.isEmpty ? fallback : s
    }
}

// `MailDetailView` lives in MailDetailView.swift.
