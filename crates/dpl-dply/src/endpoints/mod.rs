//! Typed thin wrappers over the dply v1 API, grouped exactly like the CLI's
//! command tree. Every function takes a [`DplyClient`](crate::DplyClient) and
//! returns the `data`-unwrapped [`serde_json::Value`]; the CLI's output layer
//! renders it. Query/body shapes mirror `API_REFERENCE.md`.

pub mod edge;
pub mod imports;
pub mod insights;
pub mod operator;
pub mod servers;
pub mod site;
pub mod sites;

/// Build a query vec, dropping entries whose value is empty. dply-cli omits
/// empty query params rather than sending `?status=`.
pub(crate) fn query(pairs: &[(&'static str, Option<String>)]) -> Vec<(&'static str, String)> {
    pairs
        .iter()
        .filter_map(|(k, v)| match v {
            Some(s) if !s.is_empty() => Some((*k, s.clone())),
            _ => None,
        })
        .collect()
}

/// Build a JSON object body, dropping null/empty-string fields — matching the
/// PHP CLI's `array_filter` before POST/PATCH.
pub(crate) fn body(pairs: Vec<(&str, serde_json::Value)>) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (k, v) in pairs {
        let keep = match &v {
            serde_json::Value::Null => false,
            serde_json::Value::String(s) => !s.is_empty(),
            _ => true,
        };
        if keep {
            map.insert(k.to_string(), v);
        }
    }
    serde_json::Value::Object(map)
}
