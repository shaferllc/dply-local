import Foundation

/// A decoded arbitrary JSON value. dply responses use several historical field
/// aliases and nested shapes, so — exactly like the CLI's dynamic rendering —
/// the GUI decodes rows into these and reads fields by fallback name / dotted
/// path rather than committing to 40 rigid structs.
enum JSONValue: Decodable, Hashable {
    case string(String)
    case number(Double)
    case bool(Bool)
    case object([String: JSONValue])
    case array([JSONValue])
    case null

    init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() {
            self = .null
        } else if let b = try? c.decode(Bool.self) {
            self = .bool(b)
        } else if let n = try? c.decode(Double.self) {
            self = .number(n)
        } else if let s = try? c.decode(String.self) {
            self = .string(s)
        } else if let o = try? c.decode([String: JSONValue].self) {
            self = .object(o)
        } else if let a = try? c.decode([JSONValue].self) {
            self = .array(a)
        } else {
            self = .null
        }
    }

    /// A human-readable cell string, mirroring the CLI's `cell` helper.
    var display: String {
        switch self {
        case .string(let s): return s
        case .number(let n):
            return n == n.rounded() ? String(Int(n)) : String(n)
        case .bool(let b): return b ? "true" : "false"
        case .null: return ""
        case .array(let a): return "(\(a.count) items)"
        case .object: return "{…}"
        }
    }

    var objectValue: [String: JSONValue]? {
        if case .object(let o) = self { return o }
        return nil
    }

    var arrayValue: [JSONValue]? {
        if case .array(let a) = self { return a }
        return nil
    }

    var stringValue: String? {
        if case .string(let s) = self { return s }
        return nil
    }

    var boolValue: Bool? {
        if case .bool(let b) = self { return b }
        return nil
    }

    /// JSON has one number type, so an integer field decodes as `.number`.
    var intValue: Int? {
        if case .number(let n) = self { return Int(n) }
        return nil
    }

    var isEmpty: Bool {
        switch self {
        case .null: return true
        case .string(let s): return s.isEmpty
        default: return false
        }
    }
}

/// A single object row (a site, server, deployment, …) with helpers for
/// fallback-name and dotted-path field access.
struct Row: Identifiable, Hashable {
    let fields: [String: JSONValue]

    init(_ fields: [String: JSONValue]) { self.fields = fields }

    /// Follow a dotted path (`build.framework`) through nested objects.
    func dig(_ path: String) -> JSONValue? {
        // Fast path: almost every lookup is a single key (`name`, `host`, …),
        // and the site list runs hundreds of these per render. Skip wrapping the
        // whole dict in a `.object` enum and splitting the string for those.
        // (An empty path split to zero segments and yielded the whole object;
        // preserve that even though nothing relies on it.)
        if path.isEmpty { return .object(fields) }
        if !path.contains(".") { return fields[path] }
        var obj = fields
        var current: JSONValue?
        for segment in path.split(separator: ".") {
            guard let next = obj[String(segment)] else { return nil }
            current = next
            obj = next.objectValue ?? [:]
        }
        return current
    }

    /// First present, non-empty value among `keys` (each may be a dotted path).
    func first(_ keys: [String]) -> String? {
        for key in keys {
            if let v = dig(key), !v.isEmpty {
                return v.display
            }
        }
        return nil
    }

    /// Convenience: `first` or empty string.
    func cell(_ keys: [String]) -> String {
        first(keys) ?? ""
    }

    /// Stable identity for lists: an id/name field, else a hash of the fields.
    var id: String {
        first(["id", "name", "hostname", "slug"]) ?? String(fields.hashValue)
    }
}
