import SwiftUI

/// Detail pane for one captured message: rendered HTML, plain text, raw source,
/// headers, attachments and links.
///
/// Remote content is blocked on render (see `MailWebView`) and can be allowed
/// per-message. That choice deliberately resets when you select another message:
/// permitting a tracking pixel once should not permit the next sender's.
struct MailDetailView: View {
    @EnvironmentObject var store: Store
    let id: String

    enum Tab: String, CaseIterable { case html, text, raw, headers, attachments, links }

    @State private var message: MailMessage?
    @State private var raw = ""
    @State private var loading = true
    @State private var tab: Tab = .html
    @State private var allowRemote = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if loading {
                ProgressView().controlSize(.small)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if let message {
                header(message)
                Divider()
                tabBar(message)
                Divider()
                blockedBanner(message)
                content(message)
            } else {
                ContentUnavailableView("Could not read this message", systemImage: "exclamationmark.triangle")
            }
        }
        .background(Color(nsColor: .textBackgroundColor))
        .task(id: id) { await load() }
    }

    private func load() async {
        loading = true
        // A new message gets the safe default again.
        allowRemote = false
        message = await store.mailMessage(id)
        raw = await store.mailBody(id)
        // Land on a tab that has something in it.
        if let m = message, m.html == nil {
            tab = m.text != nil ? .text : .raw
        } else {
            tab = .html
        }
        loading = false
    }

    // MARK: Header

    @ViewBuilder
    private func header(_ m: MailMessage) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(m.subject.isEmpty ? "(no subject)" : m.subject)
                .font(.title3.weight(.semibold))
                .textSelection(.enabled)

            HStack(spacing: 8) {
                Text(m.from.isEmpty ? "(unknown sender)" : m.from)
                Image(systemName: "arrow.right").font(.caption2)
                Text(m.to.isEmpty ? "—" : m.to)
                if !m.cc.isEmpty {
                    Text("cc \(m.cc)")
                }
                Spacer()
                Text(Store.mailboxLabel(m.mailbox))
                    .font(.caption2)
                    .padding(.horizontal, 6).padding(.vertical, 2)
                    .background(Theme.violet.opacity(0.15), in: Capsule())
                    .foregroundStyle(Theme.violet)
            }
            .font(.caption).foregroundStyle(.secondary).lineLimit(1)
        }
        .padding(16)
    }

    // MARK: Tabs

    private func isEnabled(_ t: Tab, _ m: MailMessage) -> Bool {
        switch t {
        case .html: return m.html != nil
        case .text: return m.text != nil
        case .raw, .headers: return true
        case .attachments: return !m.downloads.isEmpty
        case .links: return !m.links.isEmpty
        }
    }

    private func label(_ t: Tab, _ m: MailMessage) -> String {
        switch t {
        case .attachments: return "Attachments (\(m.downloads.count))"
        case .links: return "Links (\(m.links.count))"
        default: return t.rawValue.capitalized
        }
    }

    @ViewBuilder
    private func tabBar(_ m: MailMessage) -> some View {
        HStack(spacing: 4) {
            // Tabs with nothing behind them are hidden, not disabled: a "Links (0)"
            // button is noise on the many messages that have none.
            ForEach(Tab.allCases.filter { isEnabled($0, m) }, id: \.self) { t in
                Button {
                    tab = t
                } label: {
                    Text(label(t, m))
                        .font(.callout)
                        .padding(.horizontal, 10).padding(.vertical, 4)
                        .background(
                            tab == t ? Theme.violet.opacity(0.18) : .clear,
                            in: RoundedRectangle(cornerRadius: 6)
                        )
                        .foregroundStyle(tab == t ? Theme.violet : .secondary)
                }
                .buttonStyle(.plain)
            }
            Spacer()
            Text("\(m.size) bytes").font(.caption2).foregroundStyle(.tertiary)
        }
        .padding(.horizontal, 12).padding(.vertical, 6)
    }

    // MARK: Remote-content banner

    @ViewBuilder
    private func blockedBanner(_ m: MailMessage) -> some View {
        if tab == .html, m.remoteResources > 0, !allowRemote {
            HStack(spacing: 10) {
                Image(systemName: "eye.slash.fill").foregroundStyle(.orange)
                VStack(alignment: .leading, spacing: 1) {
                    Text("\(m.remoteResources) remote \(m.remoteResources == 1 ? "resource" : "resources") blocked")
                        .font(.callout.weight(.medium))
                    Text("Loading them tells the sender you opened this message.")
                        .font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                Button("Load anyway") { allowRemote = true }
                    .controlSize(.small)
            }
            .padding(.horizontal, 12).padding(.vertical, 8)
            .background(Color.orange.opacity(0.10))
            Divider()
        }
    }

    // MARK: Content

    @ViewBuilder
    private func content(_ m: MailMessage) -> some View {
        switch tab {
        case .html:
            if let html = m.html {
                MailWebView(html: html, allowRemote: allowRemote)
            }
        case .text:
            monospace(m.text ?? "")
        case .raw:
            monospace(raw)
        case .headers:
            ScrollView {
                VStack(alignment: .leading, spacing: 4) {
                    ForEach(Array(m.headers.enumerated()), id: \.offset) { _, h in
                        HStack(alignment: .top, spacing: 8) {
                            Text(h.name)
                                .font(.system(.caption, design: .monospaced).weight(.semibold))
                                .frame(width: 150, alignment: .trailing)
                                .foregroundStyle(.secondary)
                            Text(h.value)
                                .font(.system(.caption, design: .monospaced))
                                .textSelection(.enabled)
                                .frame(maxWidth: .infinity, alignment: .leading)
                        }
                    }
                }
                .padding(16)
            }
        case .attachments:
            ScrollView {
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(m.downloads) { att in
                        HStack(spacing: 10) {
                            Image(systemName: "doc").foregroundStyle(.secondary)
                            VStack(alignment: .leading, spacing: 1) {
                                Text(att.name).font(.callout)
                                Text("\(att.mime) · \(att.humanSize)")
                                    .font(.caption).foregroundStyle(.secondary)
                            }
                            Spacer()
                            Button("Save…") {
                                Task { await store.saveAttachment(messageId: m.id, attachment: att) }
                            }
                            .controlSize(.small)
                        }
                        .padding(10)
                        .background(Color.primary.opacity(0.04), in: RoundedRectangle(cornerRadius: 8))
                    }
                }
                .padding(16)
            }
        case .links:
            ScrollView {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Click to open, or right-click to copy — this is the quick way to grab a password-reset or verification URL.")
                        .font(.caption).foregroundStyle(.secondary)
                        .padding(.bottom, 4)
                    ForEach(m.links, id: \.self) { link in
                        HStack(spacing: 8) {
                            Image(systemName: "link").font(.caption).foregroundStyle(.secondary)
                            Text(link)
                                .font(.system(.caption, design: .monospaced))
                                .textSelection(.enabled)
                                .lineLimit(2)
                            Spacer()
                        }
                        .contentShape(Rectangle())
                        .onTapGesture { if let url = URL(string: link) { NSWorkspace.shared.open(url) } }
                        .contextMenu {
                            Button("Copy link") {
                                NSPasteboard.general.clearContents()
                                NSPasteboard.general.setString(link, forType: .string)
                            }
                        }
                    }
                }
                .padding(16)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
    }

    @ViewBuilder
    private func monospace(_ body: String) -> some View {
        ScrollView {
            Text(body.isEmpty ? "(empty)" : body)
                .font(.system(.callout, design: .monospaced))
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(16)
        }
    }
}
