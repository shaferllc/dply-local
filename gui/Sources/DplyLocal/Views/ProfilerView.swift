import SwiftUI
import WebKit

/// The Profiler surface: flip SPX on for a site and watch its requests turn into
/// flame graphs, served from the site's own origin.
///
/// One-click on/off per site on the left; the selected site's SPX web UI embedded
/// on the right. dpl loads SPX only into a profiled site's php-fpm pool and
/// auto-profiles every request, so there's nothing to trigger — reload the site
/// and the report shows up in the panel.
struct ProfilerPage: View {
    @EnvironmentObject var store: Store
    @State private var selected: String?
    @State private var busy: Set<String> = []
    /// Bumped to force the embedded web view to reload after a fresh request.
    @State private var reloadToken = 0

    /// Non-proxy local sites — a reverse proxy runs no PHP of ours to profile.
    private var sites: [Row] {
        store.localSites.filter { $0.cell(["source"]) != "proxy" }
    }

    private func isOn(_ r: Row) -> Bool { r.dig("profile")?.boolValue ?? false }
    private func installed(_ r: Row) -> Bool { r.dig("profile_installed")?.boolValue ?? false }

    /// The same-origin SPX UI URL for a site.
    private func profilerURL(_ r: Row) -> URL? {
        let base = r.cell(["url"]).trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        return URL(string: "\(base)/?SPX_UI_URI=/&SPX_KEY=dpl-local")
    }

    private var selectedRow: Row? {
        sites.first { $0.cell(["name"]) == selected }
    }

    var body: some View {
        HStack(spacing: 0) {
            siteList
                .frame(width: 300)
            Divider()
            detail
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .task { if store.localSites.isEmpty { await store.loadLocal() } }
        .onAppear { selectFirstProfiledIfNeeded() }
    }

    // MARK: Site list

    private var siteList: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Label("Profiler", systemImage: "flame")
                    .font(.headline)
                Spacer()
                if sites.contains(where: isOn) {
                    Text("\(sites.filter(isOn).count) on")
                        .font(.caption).foregroundStyle(.secondary)
                }
            }
            .padding(14)
            Divider()

            if sites.isEmpty {
                ContentUnavailableView(
                    "No local sites",
                    systemImage: "flame",
                    description: Text("Link a project with `dpl link .`, then flip the profiler on here.")
                )
                .frame(maxHeight: .infinity)
            } else {
                ScrollView {
                    LazyVStack(spacing: 2) {
                        ForEach(sites) { row in
                            siteRow(row)
                        }
                    }
                    .padding(8)
                }
            }
        }
    }

    private func siteRow(_ row: Row) -> some View {
        let name = row.cell(["name"])
        let on = isOn(row)
        return Button {
            if on { selected = name }
        } label: {
            HStack(spacing: 10) {
                Circle()
                    .fill(on ? Theme.violet : Color.secondary.opacity(0.35))
                    .frame(width: 7, height: 7)
                VStack(alignment: .leading, spacing: 1) {
                    Text(name).font(.callout)
                    let php = row.cell(["php"])
                    Text(php.isEmpty ? "PHP default" : "PHP \(php)")
                        .font(.caption2).foregroundStyle(.secondary)
                }
                Spacer()
                if busy.contains(name) {
                    ProgressView().controlSize(.small)
                } else {
                    Toggle("", isOn: Binding(
                        get: { on },
                        set: { newValue in Task { await toggle(name: name, on: newValue) } }
                    ))
                    .labelsHidden()
                    .toggleStyle(.switch)
                    .controlSize(.mini)
                    .tint(Theme.violet)
                }
            }
            .padding(.vertical, 6)
            .padding(.horizontal, 8)
            .background(
                RoundedRectangle(cornerRadius: 7)
                    .fill(selected == name ? Theme.violet.opacity(0.14) : .clear)
            )
        }
        .buttonStyle(.plain)
        .help(installed(row) ? "" : "SPX will be installed for this PHP when you turn it on.")
    }

    // MARK: Flame-graph detail

    @ViewBuilder
    private var detail: some View {
        if let row = selectedRow, isOn(row), let url = profilerURL(row) {
            VStack(spacing: 0) {
                HStack(spacing: 10) {
                    Text(row.cell(["name"]))
                        .font(.headline)
                    Text("every request is captured")
                        .font(.caption).foregroundStyle(.secondary)
                    Spacer()
                    Button {
                        Task { await hitAndReload(row) }
                    } label: {
                        Label("Send a request", systemImage: "arrow.clockwise")
                    }
                    .help("Load the site once so a fresh flame graph appears")
                    Button {
                        if let u = URL(string: row.cell(["url"])) { NSWorkspace.shared.open(u) }
                    } label: {
                        Label("Open site", systemImage: "arrow.up.forward.app")
                    }
                }
                .padding(12)
                Divider()
                ProfilerWebView(url: url, reloadToken: reloadToken)
            }
        } else {
            ContentUnavailableView {
                Label("Turn the profiler on for a site", systemImage: "flame")
            } description: {
                Text("Every PHP request to a profiled site becomes a flame graph, browsable here at the site's own origin.")
            }
        }
    }

    // MARK: Actions

    private func toggle(name: String, on: Bool) async {
        busy.insert(name)
        await store.setProfile(name: name, on: on)
        busy.remove(name)
        if on { selected = name }
        else if selected == name { selected = sites.first(where: isOn)?.cell(["name"]) }
    }

    /// Load the site once (auto-profiled) so a new report exists, then reload the UI.
    private func hitAndReload(_ row: Row) async {
        if let u = URL(string: row.cell(["url"])) {
            var req = URLRequest(url: u)
            req.timeoutInterval = 20
            _ = try? await URLSession.shared.data(for: req)
        }
        reloadToken += 1
    }

    private func selectFirstProfiledIfNeeded() {
        if selected == nil { selected = sites.first(where: isOn)?.cell(["name"]) }
    }
}

/// A plain WKWebView pointed at the site's same-origin SPX UI. Unlike the mail
/// preview, this is trusted local content and a full JS app, so scripts run and
/// subresources load normally.
private struct ProfilerWebView: NSViewRepresentable {
    let url: URL
    /// Changing this reloads the page (a new profile was just captured).
    var reloadToken: Int

    func makeNSView(context: Context) -> WKWebView {
        let view = WKWebView(frame: .zero, configuration: WKWebViewConfiguration())
        context.coordinator.load(view, url: url)
        return view
    }

    func updateNSView(_ view: WKWebView, context: Context) {
        context.coordinator.reloadIfNeeded(view, url: url, token: reloadToken)
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    final class Coordinator {
        private var loadedURL: URL?
        private var token: Int = -1

        func load(_ view: WKWebView, url: URL) {
            loadedURL = url
            view.load(URLRequest(url: url))
        }

        func reloadIfNeeded(_ view: WKWebView, url: URL, token: Int) {
            if loadedURL != url {
                load(view, url: url)
            } else if token != self.token {
                view.reload()
            }
            self.token = token
        }
    }
}
