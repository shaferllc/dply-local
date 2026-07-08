//! Small helpers for reading dply's loosely-typed JSON responses the same
//! way dply-cli does: pull a field by a list of fallback names, dig into
//! dotted paths (`build.framework`), and coerce a payload into display rows.
//!
//! dply endpoints are intentionally not modelled as 40 rigid structs — the
//! server returns `data`-wrapped or bare values with several historical
//! field aliases (`live_url|hostname`, `ip_address|ip`), so the CLI reads
//! them dynamically. These helpers centralise that.

use serde_json::Value;

/// Coerce an (already `data`-unwrapped) payload into a list of row objects:
/// an array stays as-is, a single object becomes a one-element list, null is
/// empty.
pub fn rows(value: &Value) -> Vec<Value> {
    match value {
        Value::Array(a) => a.clone(),
        Value::Null => Vec::new(),
        other => vec![other.clone()],
    }
}

/// First present, non-null field among `names`. Names may be dotted paths.
pub fn field<'a>(obj: &'a Value, names: &[&str]) -> Option<&'a Value> {
    for name in names {
        if let Some(v) = dig(obj, name) {
            if !v.is_null() {
                return Some(v);
            }
        }
    }
    None
}

/// Follow a dotted path like `build.framework` through nested objects.
pub fn dig<'a>(obj: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = obj;
    for seg in path.split('.') {
        cur = cur.as_object()?.get(seg)?;
    }
    Some(cur)
}

/// Render a scalar-ish [`Value`] for a table cell: strings unquoted, numbers
/// and bools as text, null as an empty string, arrays/objects compacted.
pub fn cell(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => n.to_string(),
        Some(other) => other.to_string(),
    }
}

/// Convenience: [`field`] then [`cell`], the common "show this column" path.
pub fn cell_of(obj: &Value, names: &[&str]) -> String {
    cell(field(obj, names))
}
