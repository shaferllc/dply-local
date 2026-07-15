import Foundation

/// A heterogeneous scalar (string / int / double / bool / null) used for a
/// node's key and value in the dump tree.
enum JSONScalar: Decodable, Hashable {
    case string(String), int(Int), double(Double), bool(Bool), null

    init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() { self = .null }
        else if let b = try? c.decode(Bool.self) { self = .bool(b) }
        else if let i = try? c.decode(Int.self) { self = .int(i) }
        else if let d = try? c.decode(Double.self) { self = .double(d) }
        else if let s = try? c.decode(String.self) { self = .string(s) }
        else { self = .null }
    }

    var display: String {
        switch self {
        case .string(let s): return s
        case .int(let i): return String(i)
        case .double(let d): return String(d)
        case .bool(let b): return b ? "true" : "false"
        case .null: return "null"
        }
    }
}

/// One node in a dumped value tree (mirrors the PHP Cloner's output).
struct DumpNode: Decodable, Hashable {
    var type: String
    var key: JSONScalar?
    var value: JSONScalar?
    var length: Int?
    var count: Int?
    var className: String?
    var visibility: String?
    var note: String?
    var children: [DumpNode]?

    enum CodingKeys: String, CodingKey {
        case type, key, value, length, count, visibility, note, children
        case className = "class"
    }

    var isExpandable: Bool { !(children?.isEmpty ?? true) }

    /// Right-hand summary text for this node.
    var summary: String {
        switch type {
        case "array": return "array(\(count ?? children?.count ?? 0))"
        case "object":
            let base = className ?? "object"
            return note == "recursion" ? "\(base) ↻" : base
        case "string":
            let v = value?.display ?? ""
            return "\"\(v)\"" + (length ?? v.count > 60 ? " (\(length ?? v.count))" : "")
        case "int", "float": return value?.display ?? ""
        case "bool": return value?.display ?? ""
        case "null": return "null"
        case "closure": return "Closure"
        case "resource": return value?.display ?? "resource"
        case "note": return value?.display ?? (note ?? "")
        default: return value?.display ?? note ?? type
        }
    }

    var keyDisplay: String? {
        guard let key else { return nil }
        if case .string(let s) = key { return s }
        return key.display
    }

    /// This node and its descendants as indented `key: value` lines, for copying.
    func plainText(depth: Int = 0) -> String {
        let pad = String(repeating: "  ", count: depth)
        let head = keyDisplay.map { "\($0): \(summary)" } ?? summary
        guard isExpandable else { return pad + head }
        return ([pad + head] + (children ?? []).map { $0.plainText(depth: depth + 1) })
            .joined(separator: "\n")
    }

    /// Flattened key/value text of the whole subtree, for the search index.
    /// Capped in depth so a deep object graph can't blow up the index.
    func searchText(depth: Int = 0) -> String {
        guard depth < 6 else { return "" }
        var parts = [keyDisplay, summary, className].compactMap { $0 }
        for child in children ?? [] { parts.append(child.searchText(depth: depth + 1)) }
        return parts.joined(separator: " ")
    }
}

/// One received event — a value dump (`type:"dump"`), a SQL query
/// (`type:"query"`), or an N+1 warning (`type:"n1"`).
struct DumpEntry: Decodable, Identifiable, Hashable {
    var id: Int
    var type: String?
    var site: String?
    var screen: String?
    var label: String?
    var color: String?
    var file: String?
    var line: Int?
    var context: String?
    var received_at: Double?
    var values: [DumpNode]?

    // Query / N+1 fields.
    var sql: String?
    var rawSql: String?
    var bindings: [JSONScalar]?
    var timeMs: Double?
    var connection: String?
    var slow: Bool?
    var count: Int?

    // Log fields.
    var level: String?
    var message: String?
    var data: DumpNode?

    // Mail fields.
    var subject: String?
    var from: [String]?
    var to: [String]?
    var cc: [String]?
    var bcc: [String]?
    var html: String?
    var text: String?

    // Job fields.
    var name: String?
    var queue: String?
    var status: JSONScalar?

    // HTTP fields.
    var method: String?
    var url: String?

    // Event / gate / livewire fields.
    var ability: String?
    var result: String?
    var user: String?
    var component: String?
    var arguments: DumpNode?

    // Diff / pause flags.
    var diff: Bool?
    var pause: Bool?
    var token: String?

    /// The original NDJSON line, kept so "Copy as JSON" is lossless. Not decoded.
    var raw: String?
    /// Lowercased haystack built once on arrival — `filteredDumps` re-runs on
    /// every keystroke and every appended entry, so this must not be recomputed.
    var searchIndex: String = ""

    enum CodingKeys: String, CodingKey {
        case id, type, site, screen, label, color, file, line, context, received_at, values
        case sql, bindings, connection, slow, count
        case level, message, data
        case subject, from, to, cc, bcc, html, text
        case name, queue, status, method, url
        case ability, result, user, component, arguments
        case diff, pause, token
        case rawSql = "raw_sql"
        case timeMs = "time_ms"
    }

    /// Event kind, defaulting to a value dump.
    var kind: String { type ?? "dump" }

    /// `file.php:line` (basename) for the list/detail.
    var location: String? {
        guard let file, !file.isEmpty else { return nil }
        let name = (file as NSString).lastPathComponent
        return line.map { "\(name):\($0)" } ?? name
    }

    /// A short one-line preview for the list.
    var preview: String {
        switch kind {
        case "query": return rawSql ?? sql ?? "query"
        case "n1": return sql ?? "N+1"
        case "log": return message ?? "log"
        case "mail": return subject ?? "(no subject)"
        case "job": return name ?? "job"
        case "http": return "\(method ?? "") \(url ?? "")"
        case "event": return name?.components(separatedBy: "\\").last ?? name ?? "event"
        case "gate": return ability ?? "gate"
        case "livewire": return component ?? "livewire"
        case "time": return label ?? "timer"
        default:
            return (values ?? []).map { node in
                switch node.type {
                case "object": return node.className ?? "object"
                default: return node.summary
                }
            }.joined(separator: ", ")
        }
    }

    /// Broad category for the type filter (queries groups query + n1).
    var category: String {
        switch kind {
        case "query", "n1": return "query"
        default: return kind
        }
    }

    var time: String { Self.clock.string(from: date) }
    var timeWithMillis: String { Self.clockMillis.string(from: date) }

    private var date: Date { Date(timeIntervalSince1970: (received_at ?? 0) / 1000) }

    private static let clock: DateFormatter = {
        let f = DateFormatter(); f.dateFormat = "HH:mm:ss"; return f
    }()
    private static let clockMillis: DateFormatter = {
        let f = DateFormatter(); f.dateFormat = "HH:mm:ss.SSS"; return f
    }()

    /// Human name for the kind, used in menus and the type filter.
    var kindLabel: String {
        switch kind {
        case "query": return "Query"
        case "n1": return "N+1"
        case "log": return "Log"
        case "mail": return "Mail"
        case "job": return "Job"
        case "http": return "HTTP"
        case "event": return "Event"
        case "gate": return "Gate"
        case "livewire": return "Livewire"
        case "time": return "Timer"
        default: return "Dump"
        }
    }

    /// Everything worth searching, lowercased. Built once in `Store.appendDump`.
    /// Covers the payload bodies (SQL, log message, mail subject, node values),
    /// not just the one-line preview.
    mutating func buildSearchIndex() {
        var parts: [String?] = [
            label, location, file, site, screen, kind, preview,
            sql, rawSql, connection, message, level, subject, name, queue,
            method, url, ability, result, user, component,
            status?.display,
        ]
        // One list at a time — chaining these with `+` sends the type-checker
        // into overload-resolution hell (CI's Xcode times out on it).
        parts.append(contentsOf: from ?? [])
        parts.append(contentsOf: to ?? [])
        parts.append(contentsOf: cc ?? [])
        parts.append(contentsOf: bcc ?? [])
        parts.append(contentsOf: (bindings ?? []).map { $0.display })
        parts.append(contentsOf: (values ?? []).map { $0.searchText() })
        parts.append(data?.searchText())
        parts.append(arguments?.searchText())
        if let text, text.count <= 4096 { parts.append(text) }
        searchIndex = parts.compactMap { $0 }.joined(separator: " ").lowercased()
    }

    /// A plain-text rendering of the whole entry, for ⌘C / "Copy".
    var plainText: String {
        var out = ["[\(timeWithMillis)] \(kindLabel)"]
        if let site { out[0] += " · \(site)" }
        if let loc = location { out[0] += " · \(loc)" }
        switch kind {
        case "query", "n1":
            out.append(rawSql ?? sql ?? "")
            if kind == "n1" { out.append("ran \(count ?? 0) times — likely N+1") }
            if let ms = timeMs { out.append(String(format: "%.2f ms", ms)) }
        case "log":
            out.append("\((level ?? "info").uppercased()): \(message ?? "")")
            if let data { out.append(data.plainText()) }
        case "mail":
            out.append("Subject: \(subject ?? "")")
            if let v = from, !v.isEmpty { out.append("From: \(v.joined(separator: ", "))") }
            if let v = to, !v.isEmpty { out.append("To: \(v.joined(separator: ", "))") }
            if let text { out.append(text) }
        case "http":
            out.append("\(method ?? "") \(url ?? "") \(status?.display ?? "")")
        case "event":
            out.append(name ?? "")
            if let data { out.append(data.plainText()) }
        case "gate":
            out.append("\(ability ?? "") → \(result ?? "")")
            if let arguments { out.append(arguments.plainText()) }
        case "livewire":
            out.append(component ?? "")
            if let data { out.append(data.plainText()) }
        case "job":
            out.append("\(name ?? "") \(status?.display ?? "")")
        default:
            if let label, !label.isEmpty { out.append(label) }
            out.append(contentsOf: (values ?? []).map { $0.plainText() })
        }
        return out.filter { !$0.isEmpty }.joined(separator: "\n")
    }

    /// The single most useful string to copy for this kind — bound to ⌘⇧C.
    var primaryCopyText: String? {
        switch kind {
        case "query", "n1": return rawSql ?? sql
        case "log": return message
        case "http": return url
        case "mail": return subject
        case "event": return name
        case "livewire": return component
        case "gate": return ability
        default: return nil
        }
    }
}
