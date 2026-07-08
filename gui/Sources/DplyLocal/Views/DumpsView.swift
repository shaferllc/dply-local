import SwiftUI
import WebKit

/// Middle column: the live stream of dumps, filterable by site/screen/search,
/// newest at the bottom with autoscroll.
struct DumpsListView: View {
    @EnvironmentObject var store: Store
    @Binding var selection: String?

    var body: some View {
        VStack(spacing: 0) {
            filterBar
            Divider()
            if store.filteredDumps.isEmpty {
                ContentUnavailableView(
                    "No dumps yet",
                    systemImage: "ladybug",
                    description: Text("Call `dumps($var)` in any served .test site (composer require shaferllc/dumps). It just works — no config.")
                )
            } else {
                ScrollViewReader { proxy in
                    List(selection: $selection) {
                        ForEach(store.filteredDumps) { dump in
                            DumpRow(dump: dump).tag(String(dump.id))
                        }
                    }
                    .onChange(of: store.dumps.count) {
                        if let last = store.filteredDumps.last { withAnimation { proxy.scrollTo(last.id, anchor: .bottom) } }
                    }
                }
            }
        }
        .task { store.startDumpsStream() }
    }

    private var filterBar: some View {
        HStack(spacing: 8) {
            Picker("", selection: $store.dumpSiteFilter) {
                Text("All sites").tag(String?.none)
                ForEach(store.dumpSites, id: \.self) { Text($0).tag(String?.some($0)) }
            }
            .labelsHidden().frame(maxWidth: 150)
            Picker("", selection: $store.dumpTypeFilter) {
                Text("All types").tag(String?.none)
                Text("Dumps").tag(String?.some("dump"))
                Text("Queries").tag(String?.some("query"))
                Text("Logs").tag(String?.some("log"))
                Text("Mail").tag(String?.some("mail"))
                Text("Jobs").tag(String?.some("job"))
                Text("HTTP").tag(String?.some("http"))
                Text("Events").tag(String?.some("event"))
                Text("Gates").tag(String?.some("gate"))
                Text("Livewire").tag(String?.some("livewire"))
            }
            .labelsHidden().frame(maxWidth: 110)
            if !store.dumpScreens.isEmpty {
                Picker("", selection: $store.dumpScreenFilter) {
                    Text("All screens").tag(String?.none)
                    ForEach(store.dumpScreens, id: \.self) { Text($0).tag(String?.some($0)) }
                }
                .labelsHidden().frame(maxWidth: 120)
            }
            TextField("Filter…", text: $store.dumpSearch)
                .textFieldStyle(.roundedBorder)
        }
        .padding(8)
    }
}

/// A compact row in the dump list — value dump, SQL query, or N+1 warning.
struct DumpRow: View {
    let dump: DumpEntry

    var body: some View {
        HStack(spacing: 9) {
            RoundedRectangle(cornerRadius: 3).fill(accent).frame(width: 3, height: 30)
            Image(systemName: icon).foregroundStyle(accent).font(.caption).frame(width: 14)
            VStack(alignment: .leading, spacing: 2) {
                title
                subtitle
            }
            Spacer()
            trailing
        }
        .padding(.vertical, 3)
    }

    @ViewBuilder private var title: some View {
        HStack(spacing: 6) {
            switch dump.kind {
            case "query", "http":
                Text(dump.preview).font(.system(.callout, design: .monospaced)).lineLimit(1)
            case "n1":
                Text("N+1 · \(dump.sql ?? "")").font(.system(.callout, design: .monospaced)).lineLimit(1)
            case "dump":
                if let label = dump.label, !label.isEmpty {
                    Text(label).font(.callout.weight(.semibold))
                } else {
                    Text(dump.preview).font(.callout).lineLimit(1)
                }
            default:
                Text(dump.preview).font(.callout).lineLimit(1)
            }
            if let site = dump.site {
                Text(site).font(.caption2).padding(.horizontal, 5).padding(.vertical, 1)
                    .background(Color.secondary.opacity(0.15), in: Capsule()).foregroundStyle(.secondary)
            }
        }
    }

    @ViewBuilder private var subtitle: some View {
        HStack(spacing: 6) {
            switch dump.kind {
            case "n1":
                Text("ran \(dump.count ?? 0) times — likely N+1").font(.caption2).foregroundStyle(.orange)
            case "query":
                if let conn = dump.connection { Text(conn).font(.caption2).foregroundStyle(.tertiary) }
            case "log":
                Text((dump.level ?? "info").uppercased()).font(.caption2).foregroundStyle(levelColor)
                if let loc = dump.location { Text(loc).font(.system(.caption2, design: .monospaced)).foregroundStyle(.tertiary) }
            case "mail":
                if let to = dump.to?.first { Text("to: \(to)").font(.caption2).foregroundStyle(.tertiary) }
            case "job":
                Text("\(dump.status?.display ?? "") · \(dump.connection ?? "")\(dump.queue.map { "/\($0)" } ?? "")")
                    .font(.caption2).foregroundStyle(.tertiary)
            case "gate":
                Text(dump.result ?? "").font(.caption2)
                    .foregroundStyle(dump.result == "allowed" ? .green : .red)
                if let u = dump.user { Text("· user \(u)").font(.caption2).foregroundStyle(.tertiary) }
            case "event":
                if let full = dump.name { Text(full).font(.system(.caption2, design: .monospaced)).foregroundStyle(.tertiary).lineLimit(1) }
            case "livewire":
                Text("component").font(.caption2).foregroundStyle(.tertiary)
            default:
                if let loc = dump.location { Text(loc).font(.system(.caption2, design: .monospaced)).foregroundStyle(.secondary) }
                if let screen = dump.screen { Text("· \(screen)").font(.caption2).foregroundStyle(.tertiary) }
            }
        }
    }

    @ViewBuilder private var trailing: some View {
        switch dump.kind {
        case "query":
            if let ms = dump.timeMs { msBadge(ms, red: dump.slow == true) }
        case "http":
            HStack(spacing: 5) {
                if let s = dump.status?.display { statusBadge(s) }
                if let ms = dump.timeMs { Text("\(ms, specifier: "%.0f")ms").font(.system(.caption2, design: .monospaced)).foregroundStyle(.tertiary) }
            }
        case "time":
            if let ms = dump.timeMs { msBadge(ms, red: false) }
        default:
            Text(dump.time).font(.system(.caption2, design: .monospaced)).foregroundStyle(.tertiary)
        }
    }

    private func msBadge(_ ms: Double, red: Bool) -> some View {
        Text("\(ms, specifier: "%.1f")ms")
            .font(.system(.caption2, design: .monospaced))
            .padding(.horizontal, 5).padding(.vertical, 1)
            .background((red ? Color.red : Color.secondary).opacity(0.15), in: Capsule())
            .foregroundStyle(red ? .red : .secondary)
    }

    private func statusBadge(_ s: String) -> some View {
        let code = Int(s) ?? 0
        let color: Color = code >= 500 ? .red : code >= 400 ? .orange : code >= 200 ? .green : .secondary
        return Text(s).font(.caption2.monospaced().weight(.semibold))
            .padding(.horizontal, 5).padding(.vertical, 1)
            .background(color.opacity(0.15), in: Capsule()).foregroundStyle(color)
    }

    private var levelColor: Color {
        switch dump.level?.lowercased() {
        case "error", "critical", "alert", "emergency": return .red
        case "warning": return .orange
        case "notice", "info": return .blue
        default: return .secondary
        }
    }

    private var icon: String {
        switch dump.kind {
        case "query": return "cylinder"
        case "n1": return "exclamationmark.triangle.fill"
        case "log": return "text.alignleft"
        case "mail": return "envelope"
        case "job": return "gearshape"
        case "http": return "network"
        case "event": return "arrow.triangle.branch"
        case "gate": return "lock.shield"
        case "livewire": return "bolt.horizontal.circle"
        case "time": return "timer"
        default: return dump.pause == true ? "pause.circle.fill" : (dump.diff == true ? "plusminus" : "curlybraces")
        }
    }

    private var accent: Color {
        switch dump.kind {
        case "query": return dump.slow == true ? .red : .blue
        case "n1": return .orange
        case "log": return levelColor
        case "mail": return Theme.violet
        case "job":
            switch dump.status?.display {
            case "failed": return .red
            case "processed": return .green
            default: return .blue
            }
        case "http": return .teal
        case "event": return .indigo
        case "gate": return dump.result == "allowed" ? .green : .red
        case "livewire": return .pink
        case "time": return .mint
        default:
            if dump.pause == true { return .orange }
            return colorFor(dump.color)
        }
    }
}

/// Detail: metadata header (open-in-editor) + the value trees.
struct DumpDetailView: View {
    @EnvironmentObject var store: Store
    let dump: DumpEntry

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                header
                Divider()
                switch dump.kind {
                case "query": queryBody
                case "n1": n1Body
                case "log": logBody
                case "mail": mailBody
                case "job": jobBody
                case "http": httpBody
                case "event": eventBody
                case "gate": gateBody
                case "livewire": livewireBody
                default:
                    if dump.diff == true, let vals = dump.values, vals.count >= 2 {
                        DiffView(before: vals[0], after: vals[1])
                    } else {
                        ForEach(Array((dump.values ?? []).enumerated()), id: \.offset) { _, node in
                            DumpNodeView(node: node, depth: 0, initiallyExpanded: true)
                        }
                    }
                }
            }
            .padding(16)
        }
    }

    @ViewBuilder private var queryBody: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 10) {
                if let conn = dump.connection {
                    Label(conn, systemImage: "cylinder").font(.caption).foregroundStyle(.secondary)
                }
                if let ms = dump.timeMs {
                    Text(String(format: "%.2f ms", ms))
                        .font(.system(.caption, design: .monospaced))
                        .foregroundStyle(dump.slow == true ? .red : .secondary)
                    if dump.slow == true { Text("SLOW").font(.caption2.bold()).foregroundStyle(.red) }
                }
                Spacer()
            }
            sqlBlock(dump.rawSql ?? dump.sql ?? "")
            if let bindings = dump.bindings, !bindings.isEmpty {
                DetailSection(title: "Bindings") {
                    ForEach(Array(bindings.enumerated()), id: \.offset) { i, b in
                        HStack {
                            Text("\(i)").font(.caption2.monospaced()).foregroundStyle(.tertiary).frame(width: 20, alignment: .leading)
                            Text(b.display).font(.system(.callout, design: .monospaced)).textSelection(.enabled)
                        }
                    }
                }
            }
        }
    }

    @ViewBuilder private var n1Body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Label("Ran \(dump.count ?? 0) times in one request — likely an N+1 query.",
                  systemImage: "exclamationmark.triangle.fill")
                .foregroundStyle(.orange)
                .font(.callout)
            if let conn = dump.connection, !conn.isEmpty {
                Text("connection: \(conn)").font(.caption).foregroundStyle(.secondary)
            }
            sqlBlock(dump.sql ?? "")
            Text("Eager-load the relationship (e.g. `with(...)`) to collapse these into one query.")
                .font(.caption).foregroundStyle(.secondary)
        }
    }

    @ViewBuilder private var logBody: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 8) {
                Text((dump.level ?? "info").uppercased())
                    .font(.caption.bold())
                    .padding(.horizontal, 7).padding(.vertical, 2)
                    .background(logColor.opacity(0.15), in: Capsule()).foregroundStyle(logColor)
                Spacer()
            }
            Text(dump.message ?? "")
                .font(.system(.callout, design: .monospaced))
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
            if let data = dump.data {
                DetailSection(title: "Context") {
                    DumpNodeView(node: data, depth: 0, initiallyExpanded: true)
                }
            }
        }
    }

    private var logColor: Color {
        switch dump.level?.lowercased() {
        case "error", "critical", "alert", "emergency": return .red
        case "warning": return .orange
        default: return .blue
        }
    }

    @ViewBuilder private var mailBody: some View {
        VStack(alignment: .leading, spacing: 10) {
            DetailSection(title: "Headers") {
                KeyValueView(row: mailHeaderRow, fields: [
                    ("Subject", ["subject"]), ("From", ["from"]),
                    ("To", ["to"]), ("Cc", ["cc"]), ("Bcc", ["bcc"]),
                ])
            }
            if let html = dump.html, !html.isEmpty {
                DetailSection(title: "Preview") {
                    MailWebView(html: html).frame(minHeight: 260)
                }
            } else if let text = dump.text {
                DetailSection(title: "Body") {
                    Text(text).font(.system(.callout, design: .monospaced)).textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
        }
    }

    /// Build a JSON-ish Row so KeyValueView can render the mail headers.
    private var mailHeaderRow: Row {
        var f: [String: JSONValue] = [:]
        if let s = dump.subject { f["subject"] = .string(s) }
        if let v = dump.from, !v.isEmpty { f["from"] = .string(v.joined(separator: ", ")) }
        if let v = dump.to, !v.isEmpty { f["to"] = .string(v.joined(separator: ", ")) }
        if let v = dump.cc, !v.isEmpty { f["cc"] = .string(v.joined(separator: ", ")) }
        if let v = dump.bcc, !v.isEmpty { f["bcc"] = .string(v.joined(separator: ", ")) }
        return Row(f)
    }

    @ViewBuilder private var jobBody: some View {
        DetailSection(title: "Job") {
            KeyValueView(row: jobRow, fields: [
                ("Name", ["name"]), ("Status", ["status"]),
                ("Connection", ["connection"]), ("Queue", ["queue"]),
            ])
        }
    }

    private var jobRow: Row {
        var f: [String: JSONValue] = [:]
        if let v = dump.name { f["name"] = .string(v) }
        if let v = dump.status?.display { f["status"] = .string(v) }
        if let v = dump.connection { f["connection"] = .string(v) }
        if let v = dump.queue { f["queue"] = .string(v) }
        return Row(f)
    }

    @ViewBuilder private var httpBody: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Text(dump.method ?? "").font(.callout.bold().monospaced())
                if let s = dump.status?.display { Text(s).font(.callout.monospaced()) }
                if let ms = dump.timeMs { Text(String(format: "%.0f ms", ms)).font(.caption).foregroundStyle(.secondary) }
                Spacer()
            }
            Text(dump.url ?? "")
                .font(.system(.callout, design: .monospaced)).textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(12)
                .background(Color(nsColor: .textBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
        }
    }

    @ViewBuilder private var eventBody: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(dump.name ?? "").font(.system(.callout, design: .monospaced)).textSelection(.enabled)
                .foregroundStyle(.indigo)
            if let data = dump.data {
                DetailSection(title: "Payload") {
                    DumpNodeView(node: data, depth: 0, initiallyExpanded: true)
                }
            } else {
                Text("No payload.").font(.callout).foregroundStyle(.secondary)
            }
        }
    }

    @ViewBuilder private var gateBody: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Text(dump.ability ?? "").font(.callout.weight(.semibold))
                Text((dump.result ?? "").uppercased())
                    .font(.caption.bold()).padding(.horizontal, 7).padding(.vertical, 2)
                    .background((dump.result == "allowed" ? Color.green : Color.red).opacity(0.15), in: Capsule())
                    .foregroundStyle(dump.result == "allowed" ? .green : .red)
                Spacer()
            }
            if let u = dump.user { Text("user: \(u)").font(.callout).foregroundStyle(.secondary) }
            if let args = dump.arguments {
                DetailSection(title: "Arguments") {
                    DumpNodeView(node: args, depth: 0, initiallyExpanded: true)
                }
            }
        }
    }

    @ViewBuilder private var livewireBody: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(dump.component ?? "").font(.title3.weight(.bold)).foregroundStyle(.pink)
            if let data = dump.data {
                DetailSection(title: "Properties") {
                    DumpNodeView(node: data, depth: 0, initiallyExpanded: true)
                }
            }
        }
    }

    private func sqlBlock(_ sql: String) -> some View {
        Text(sql)
            .font(.system(.callout, design: .monospaced))
            .textSelection(.enabled)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(12)
            .background(Color(nsColor: .textBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
            .overlay(alignment: .topTrailing) {
                Button { copySql(sql) } label: { Image(systemName: "doc.on.doc") }
                    .buttonStyle(.borderless).padding(8)
            }
    }

    private func copySql(_ s: String) { store.copyToPasteboard(s) }

    private var header: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                if let label = dump.label, !label.isEmpty {
                    Text(label).font(.title3.weight(.bold)).foregroundStyle(colorFor(dump.color))
                }
                if let site = dump.site {
                    Text(site).font(.caption).padding(.horizontal, 6).padding(.vertical, 2)
                        .background(Color.secondary.opacity(0.15), in: Capsule()).foregroundStyle(.secondary)
                }
                Spacer()
                Text(dump.time).font(.system(.caption, design: .monospaced)).foregroundStyle(.secondary)
            }
            HStack(spacing: 10) {
                if let file = dump.file, let loc = dump.location {
                    Button {
                        store.openInEditor(file: file, line: dump.line)
                    } label: {
                        Label(loc, systemImage: "arrow.up.forward.app")
                    }
                    .buttonStyle(.link)
                    .font(.system(.caption, design: .monospaced))
                }
                if let screen = dump.screen {
                    Text("screen: \(screen)").font(.caption).foregroundStyle(.secondary)
                }
            }
        }
    }
}

/// Recursive collapsible tree row for a dumped value.
struct DumpNodeView: View {
    let node: DumpNode
    let depth: Int
    var initiallyExpanded: Bool = false
    @State private var expanded: Bool = false

    var body: some View {
        if node.isExpandable {
            DisclosureGroup(isExpanded: $expanded) {
                ForEach(Array((node.children ?? []).enumerated()), id: \.offset) { _, child in
                    DumpNodeView(node: child, depth: depth + 1)
                }
            } label: {
                rowLabel
            }
            .onAppear { if initiallyExpanded { expanded = true } }
        } else {
            rowLabel.padding(.leading, 2)
        }
    }

    private var rowLabel: some View {
        HStack(spacing: 6) {
            if let key = node.keyDisplay {
                Text(key).font(.system(.callout, design: .monospaced)).foregroundStyle(keyColor)
                Text(":").foregroundStyle(.secondary)
            }
            Text(node.summary)
                .font(.system(.callout, design: .monospaced))
                .foregroundStyle(valueColor)
                .textSelection(.enabled)
            if let vis = node.visibility, vis != "public" {
                Text(vis).font(.caption2).foregroundStyle(.tertiary)
            }
        }
        .lineLimit(1)
    }

    private var keyColor: Color { .secondary }

    private var valueColor: Color {
        switch node.type {
        case "string": return Color(red: 0.80, green: 0.36, blue: 0.20) // rust
        case "int", "float": return Color(red: 0.20, green: 0.48, blue: 0.90) // blue
        case "bool": return Color(red: 0.55, green: 0.35, blue: 0.85) // purple
        case "null", "note": return .secondary
        case "object": return Color(red: 0.13, green: 0.55, blue: 0.45) // teal
        case "array": return .primary
        default: return .primary
        }
    }
}

/// Prominent banner shown while a `dumps()->pause()` breakpoint holds a PHP
/// request open — Continue lets it proceed, Stop terminates it.
struct PauseBanner: View {
    let dump: DumpEntry
    var onResume: (String) -> Void

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: "pause.circle.fill").font(.title2).foregroundStyle(.orange)
            VStack(alignment: .leading, spacing: 2) {
                Text("Paused — request is waiting").font(.callout.weight(.semibold))
                HStack(spacing: 6) {
                    if let site = dump.site { Text(site).font(.caption).foregroundStyle(.secondary) }
                    if let loc = dump.location { Text(loc).font(.system(.caption, design: .monospaced)).foregroundStyle(.secondary) }
                }
            }
            Spacer()
            Button("Stop", role: .destructive) { onResume("stop") }
            Button("Continue") { onResume("continue") }
                .buttonStyle(.borderedProminent).tint(.orange).keyboardShortcut(.defaultAction)
        }
        .padding(12)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).stroke(.orange.opacity(0.6), lineWidth: 1.5))
        .shadow(color: .black.opacity(0.15), radius: 12, y: 4)
        .padding(.horizontal, 14).padding(.top, 10)
    }
}

/// Renders the delta between two dumped values (a `->diff()` dump): compares
/// their children by key and marks added / removed / changed / unchanged.
struct DiffView: View {
    let before: DumpNode
    let after: DumpNode

    private struct Line: Identifiable { let id = UUID(); let key: String; let state: State; let old: String?; let new: String? }
    private enum State { case added, removed, changed, same }

    var body: some View {
        if before.isExpandable || after.isExpandable {
            VStack(alignment: .leading, spacing: 4) {
                ForEach(lines) { line in
                    HStack(spacing: 8) {
                        Image(systemName: symbol(line.state)).foregroundStyle(color(line.state)).frame(width: 14)
                        Text(line.key).font(.system(.callout, design: .monospaced)).foregroundStyle(.secondary)
                        Text(":").foregroundStyle(.tertiary)
                        switch line.state {
                        case .changed:
                            Text(line.old ?? "").font(.system(.callout, design: .monospaced)).foregroundStyle(.red).strikethrough()
                            Image(systemName: "arrow.right").font(.caption2).foregroundStyle(.secondary)
                            Text(line.new ?? "").font(.system(.callout, design: .monospaced)).foregroundStyle(.green)
                        case .added:
                            Text(line.new ?? "").font(.system(.callout, design: .monospaced)).foregroundStyle(.green)
                        case .removed:
                            Text(line.old ?? "").font(.system(.callout, design: .monospaced)).foregroundStyle(.red).strikethrough()
                        case .same:
                            Text(line.new ?? "").font(.system(.callout, design: .monospaced)).foregroundStyle(.secondary)
                        }
                        Spacer()
                    }
                }
            }
        } else {
            HStack(spacing: 8) {
                Text(before.summary).font(.system(.callout, design: .monospaced)).foregroundStyle(.red).strikethrough()
                Image(systemName: "arrow.right").foregroundStyle(.secondary)
                Text(after.summary).font(.system(.callout, design: .monospaced)).foregroundStyle(.green)
            }
        }
    }

    private var lines: [Line] {
        let b = Dictionary(uniqueKeysWithValues: (before.children ?? []).compactMap { c in c.keyDisplay.map { ($0, c) } })
        let a = Dictionary(uniqueKeysWithValues: (after.children ?? []).compactMap { c in c.keyDisplay.map { ($0, c) } })
        let keys = Array(Set(b.keys).union(a.keys)).sorted()
        return keys.map { key in
            switch (b[key], a[key]) {
            case let (old?, new?):
                return Line(key: key, state: old == new ? .same : .changed, old: old.summary, new: new.summary)
            case let (old?, nil):
                return Line(key: key, state: .removed, old: old.summary, new: nil)
            case let (nil, new?):
                return Line(key: key, state: .added, old: nil, new: new.summary)
            default:
                return Line(key: key, state: .same, old: nil, new: nil)
            }
        }
    }

    private func symbol(_ s: State) -> String {
        switch s { case .added: return "plus"; case .removed: return "minus"; case .changed: return "arrow.left.arrow.right"; case .same: return "equal" }
    }
    private func color(_ s: State) -> Color {
        switch s { case .added: return .green; case .removed: return .red; case .changed: return .orange; case .same: return .secondary }
    }
}

/// Renders a captured email's HTML in a lightweight web view for preview.
struct MailWebView: NSViewRepresentable {
    let html: String

    func makeNSView(context: Context) -> WKWebView {
        let view = WKWebView()
        view.setValue(false, forKey: "drawsBackground")
        return view
    }

    func updateNSView(_ view: WKWebView, context: Context) {
        view.loadHTMLString(html, baseURL: nil)
    }
}

/// Map a dump's color name to a SwiftUI color.
func colorFor(_ name: String?) -> Color {
    switch name?.lowercased() {
    case "red": return .red
    case "green": return .green
    case "blue": return .blue
    case "orange": return .orange
    case "purple": return Theme.violet
    case "gray", "grey": return .gray
    default: return Theme.violet
    }
}
