import SwiftUI
import WebKit

/// Middle column: the live stream of dumps, filterable by site/screen/search,
/// newest at the bottom with autoscroll.
struct DumpsListView: View {
    @EnvironmentObject var store: Store
    @Binding var selection: String?

    /// Categories offered in the type picker, in menu order.
    private static let categories: [(id: String, label: String)] = [
        ("dump", "Dumps"), ("query", "Queries"), ("log", "Logs"), ("mail", "Mail"),
        ("job", "Jobs"), ("http", "HTTP"), ("event", "Events"), ("gate", "Gates"),
        ("livewire", "Livewire"), ("time", "Timers"),
    ]

    var body: some View {
        content
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .safeAreaInset(edge: .top, spacing: 0) {
                VStack(spacing: 0) {
                    filterBar
                    Divider()
                }
                .background(.bar)
            }
            .task { store.startDumpsStream() }
    }

    @ViewBuilder private var content: some View {
        if store.dumps.isEmpty {
            ContentUnavailableView(
                "No dumps yet",
                systemImage: "ladybug",
                description: Text("Call `dumps($var)` in any served .test site (composer require shaferllc/dumps). It just works — no config.")
            )
        } else if store.filteredDumps.isEmpty {
            ContentUnavailableView {
                Label("No matches", systemImage: "line.3.horizontal.decrease.circle")
            } description: {
                Text("\(store.dumps.count) dumps received, none match the current filters.")
            } actions: {
                Button("Reset filters") { resetFilters() }
            }
        } else {
            ScrollViewReader { proxy in
                List(selection: $selection) {
                    ForEach(store.filteredDumps) { dump in
                        DumpRow(dump: dump, query: store.dumpSearch)
                            .tag(String(dump.id))
                            .contextMenu { DumpContextMenu(dump: dump) }
                    }
                }
                .onChange(of: store.dumps.count) {
                    guard store.dumpsAutoscroll, let last = store.filteredDumps.last else { return }
                    withAnimation { proxy.scrollTo(last.id, anchor: .bottom) }
                }
                .overlay(alignment: .bottomTrailing) {
                    if !store.dumpsAutoscroll { jumpToLatest(proxy) }
                }
            }
        }
    }

    /// Shown while following is off, so the tail is always one click away.
    private func jumpToLatest(_ proxy: ScrollViewProxy) -> some View {
        Button {
            if let last = store.filteredDumps.last { withAnimation { proxy.scrollTo(last.id, anchor: .bottom) } }
        } label: {
            Label("Latest", systemImage: "arrow.down.to.line")
                .font(.caption.weight(.medium))
                .padding(.horizontal, 9).padding(.vertical, 5)
        }
        .buttonStyle(.plain)
        .background(.regularMaterial, in: Capsule())
        .overlay(Capsule().stroke(Color(nsColor: .separatorColor)))
        .shadow(color: .black.opacity(0.12), radius: 6, y: 2)
        .padding(12)
    }

    private func resetFilters() {
        store.dumpSiteFilter = nil
        store.dumpScreenFilter = nil
        store.dumpTypeFilter = nil
        store.dumpSearch = ""
    }

    /// Two rows: the pickers alone are wider than this column, so the search
    /// field gets its own line rather than being crushed to nothing.
    private var filterBar: some View {
        VStack(spacing: 6) {
            HStack(spacing: 6) {
                Picker("", selection: $store.dumpSiteFilter) {
                    Text("All sites").tag(String?.none)
                    ForEach(store.dumpSites, id: \.self) { Text($0).tag(String?.some($0)) }
                }
                .labelsHidden()
                typePicker
                if !store.dumpScreens.isEmpty {
                    Picker("", selection: $store.dumpScreenFilter) {
                        Text("All screens").tag(String?.none)
                        ForEach(store.dumpScreens, id: \.self) { Text($0).tag(String?.some($0)) }
                    }
                    .labelsHidden()
                }
            }
            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass").font(.caption).foregroundStyle(.tertiary)
                TextField("Filter…", text: $store.dumpSearch)
                    .textFieldStyle(.plain)
                if !store.dumpSearch.isEmpty {
                    Button { store.dumpSearch = "" } label: {
                        Image(systemName: "xmark.circle.fill").foregroundStyle(.tertiary)
                    }
                    .buttonStyle(.plain).help("Clear the filter")
                }
                Text(countLabel)
                    .font(.caption2.monospacedDigit())
                    .foregroundStyle(.tertiary)
                    .fixedSize()
                Toggle(isOn: $store.dumpsAutoscroll) {
                    Image(systemName: store.dumpsAutoscroll ? "arrow.down.circle.fill" : "arrow.down.circle")
                }
                .toggleStyle(.button)
                .buttonStyle(.borderless)
                .help(store.dumpsAutoscroll
                      ? "Following new dumps — click to hold the scroll position"
                      : "Not following — click to resume following the newest dump")
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 7)
    }

    /// "15" when nothing is filtered out, "6 / 15" when something is.
    private var countLabel: String {
        let shown = store.filteredDumps.count, total = store.dumps.count
        return shown == total ? "\(total)" : "\(shown) / \(total)"
    }

    /// Type filter, annotated with how many entries of each kind survive the
    /// other filters — so you can see there are 40 queries before switching.
    private var typePicker: some View {
        let counts = store.dumpCounts
        return Picker("", selection: $store.dumpTypeFilter) {
            Text("All types").tag(String?.none)
            ForEach(Self.categories, id: \.id) { cat in
                let n = counts[cat.id] ?? 0
                Text(n > 0 ? "\(cat.label) (\(n))" : cat.label).tag(String?.some(cat.id))
            }
        }
        .labelsHidden().fixedSize()
    }
}

/// Right-click actions shared by the list row and the detail header.
struct DumpContextMenu: View {
    @EnvironmentObject var store: Store
    let dump: DumpEntry

    var body: some View {
        if let file = dump.file, let loc = dump.location {
            Button("Open \(loc)", systemImage: "arrow.up.forward.app") {
                store.openInEditor(file: file, line: dump.line)
            }
            Divider()
        }
        if let primary = dump.primaryCopyText, !primary.isEmpty {
            Button("Copy \(dump.kindLabel)", systemImage: "doc.on.doc") { store.copyToPasteboard(primary) }
        }
        Button("Copy as Text", systemImage: "doc.plaintext") { store.copyToPasteboard(dump.plainText) }
        if let raw = dump.raw {
            Button("Copy as JSON", systemImage: "curlybraces") { store.copyToPasteboard(raw) }
        }
        Divider()
        if let site = dump.site {
            Button("Only \(site)", systemImage: "line.3.horizontal.decrease.circle") { store.dumpSiteFilter = site }
        }
        if let screen = dump.screen {
            Button("Only screen \(screen)", systemImage: "rectangle.on.rectangle") { store.dumpScreenFilter = screen }
        }
        Button("Only \(dump.kindLabel)s", systemImage: "square.stack.3d.up") { store.dumpTypeFilter = dump.category }
        Divider()
        Button(role: .destructive) { store.hideDump(dump.id) } label: {
            Label("Hide", systemImage: "eye.slash")
        }
    }
}

/// A compact row in the dump list — value dump, SQL query, or N+1 warning.
struct DumpRow: View {
    let dump: DumpEntry
    /// Active search text, highlighted inside the title.
    var query: String = ""

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
        .help(dump.timeWithMillis)
    }

    @ViewBuilder private var title: some View {
        HStack(spacing: 6) {
            switch dump.kind {
            case "query", "http":
                highlight(dump.preview).font(.system(.callout, design: .monospaced)).lineLimit(1)
            case "n1":
                highlight("N+1 · \(dump.sql ?? "")").font(.system(.callout, design: .monospaced)).lineLimit(1)
            case "dump":
                if let label = dump.label, !label.isEmpty {
                    highlight(label).font(.callout.weight(.semibold))
                } else {
                    highlight(dump.preview).font(.callout).lineLimit(1)
                }
            default:
                highlight(dump.preview).font(.callout).lineLimit(1)
            }
            if let site = dump.site {
                Text(site).font(.caption2).padding(.horizontal, 5).padding(.vertical, 1)
                    .background(Color.secondary.opacity(0.15), in: Capsule()).foregroundStyle(.secondary)
            }
        }
    }

    /// The title with every occurrence of the search text tinted, so you can see
    /// *why* a row matched when the hit is deep in a long SQL string.
    private func highlight(_ text: String) -> Text {
        guard !query.isEmpty else { return Text(text) }
        var attributed = AttributedString(text)
        var search = attributed.startIndex..<attributed.endIndex
        while let found = attributed[search].range(of: query, options: .caseInsensitive) {
            attributed[found].backgroundColor = .yellow.opacity(0.28)
            attributed[found].foregroundColor = .primary
            guard found.upperBound < attributed.endIndex else { break }
            search = found.upperBound..<attributed.endIndex
        }
        return Text(attributed)
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
        .lineLimit(1)
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

/// A broadcast "expand/collapse everything" command. The token changes on each
/// press so `onChange` fires even when the direction repeats.
struct ExpandSignal: Equatable {
    var token: Int = 0
    var expand: Bool = false
}

/// Detail: metadata header (open-in-editor) + the value trees.
struct DumpDetailView: View {
    @EnvironmentObject var store: Store
    let dump: DumpEntry

    @State private var expansion = ExpandSignal()

    /// True when the body renders a value tree that expand/collapse-all applies to.
    private var hasTree: Bool {
        switch dump.kind {
        case "log", "event", "livewire": return dump.data?.isExpandable == true
        case "gate": return dump.arguments?.isExpandable == true
        case "query", "n1", "mail", "job", "http": return false
        default: return dump.diff != true && (dump.values ?? []).contains { $0.isExpandable }
        }
    }

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
                case "time": timeBody
                default:
                    if dump.diff == true, let vals = dump.values, vals.count >= 2 {
                        DiffView(before: vals[0], after: vals[1])
                    } else {
                        ForEach(Array((dump.values ?? []).enumerated()), id: \.offset) { _, node in
                            DumpNodeView(node: node, depth: 0, initiallyExpanded: true, signal: expansion)
                        }
                    }
                }
            }
            .padding(16)
        }
        .contextMenu { DumpContextMenu(dump: dump) }
    }

    @ViewBuilder private var timeBody: some View {
        HStack(spacing: 8) {
            Image(systemName: "timer").foregroundStyle(.mint)
            Text(dump.label ?? "timer").font(.callout.weight(.semibold))
            Spacer()
            if let ms = dump.timeMs {
                Text(String(format: "%.2f ms", ms))
                    .font(.system(.callout, design: .monospaced)).foregroundStyle(.secondary)
            }
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
                    DumpNodeView(node: data, depth: 0, initiallyExpanded: true, signal: expansion)
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
                    DumpNodeView(node: data, depth: 0, initiallyExpanded: true, signal: expansion)
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
                    DumpNodeView(node: args, depth: 0, initiallyExpanded: true, signal: expansion)
                }
            }
        }
    }

    @ViewBuilder private var livewireBody: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(dump.component ?? "").font(.title3.weight(.bold)).foregroundStyle(.pink)
            if let data = dump.data {
                DetailSection(title: "Properties") {
                    DumpNodeView(node: data, depth: 0, initiallyExpanded: true, signal: expansion)
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
                } else {
                    Text(dump.kindLabel).font(.title3.weight(.bold)).foregroundStyle(.secondary)
                }
                if let site = dump.site {
                    Text(site).font(.caption).padding(.horizontal, 6).padding(.vertical, 2)
                        .background(Color.secondary.opacity(0.15), in: Capsule()).foregroundStyle(.secondary)
                }
                Spacer()
                headerActions
                Text(dump.timeWithMillis)
                    .font(.system(.caption, design: .monospaced)).foregroundStyle(.secondary)
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

    @ViewBuilder private var headerActions: some View {
        if hasTree {
            Button { expansion = ExpandSignal(token: expansion.token + 1, expand: true) } label: {
                Image(systemName: "chevron.down.square")
            }
            .buttonStyle(.borderless).help("Expand all")
            Button { expansion = ExpandSignal(token: expansion.token + 1, expand: false) } label: {
                Image(systemName: "chevron.right.square")
            }
            .buttonStyle(.borderless).help("Collapse all")
        }
        Menu {
            DumpContextMenu(dump: dump)
        } label: {
            Image(systemName: "ellipsis.circle")
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .fixedSize()
        .help("Copy, filter, and open actions")
    }
}

/// Recursive collapsible tree row for a dumped value.
struct DumpNodeView: View {
    @EnvironmentObject var store: Store
    let node: DumpNode
    let depth: Int
    var initiallyExpanded: Bool = false
    /// Expand/collapse-all broadcast from the detail header.
    var signal: ExpandSignal = ExpandSignal()
    @State private var expanded: Bool = false

    var body: some View {
        if node.isExpandable {
            DisclosureGroup(isExpanded: $expanded) {
                ForEach(Array((node.children ?? []).enumerated()), id: \.offset) { _, child in
                    DumpNodeView(node: child, depth: depth + 1, signal: signal)
                }
            } label: {
                rowLabel
            }
            // A node revealed *by* expand-all is built after the signal fired, so
            // it never sees the onChange — it has to read the signal on appear or
            // the expansion stops one level deep.
            .onAppear { expanded = initiallyExpanded || signal.expand }
            .onChange(of: signal) { expanded = signal.expand }
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
            // `array(n)` already carries its size in the summary; objects don't.
            if node.type == "object", let n = node.children?.count, n > 0 {
                Text("\(n)")
                    .font(.caption2.monospacedDigit())
                    .padding(.horizontal, 4)
                    .background(Color.secondary.opacity(0.12), in: Capsule())
                    .foregroundStyle(.tertiary)
            }
            // Without this the leaf rows center themselves in the detail pane.
            Spacer(minLength: 0)
        }
        .lineLimit(1)
        .contextMenu {
            Button("Copy Value", systemImage: "doc.on.doc") {
                store.copyToPasteboard(node.value?.display ?? node.summary)
            }
            if let key = node.keyDisplay {
                Button("Copy Key", systemImage: "key") { store.copyToPasteboard(key) }
            }
            if node.isExpandable {
                Button("Copy Subtree", systemImage: "list.bullet.indent") {
                    store.copyToPasteboard(node.plainText())
                }
            }
        }
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

// `MailWebView` lives in MailWebView.swift — it blocks remote content, which
// this preview needs just as much as the Mail viewer does.

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
