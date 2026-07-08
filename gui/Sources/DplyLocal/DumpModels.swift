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

    var time: String {
        guard let ms = received_at else { return "" }
        let date = Date(timeIntervalSince1970: ms / 1000)
        let f = DateFormatter()
        f.dateFormat = "HH:mm:ss"
        return f.string(from: date)
    }
}
