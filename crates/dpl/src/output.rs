//! Terminal rendering for dply responses. Two shapes cover every command:
//! a **table** (list endpoints) and a **detail** two-column view (show
//! endpoints). Both read fields dynamically with fallback names via
//! [`dpl_dply::models`], matching how dply-cli displays the same payloads.
//! `--json` short-circuits to a pretty raw dump.

use dpl_dply::models;
use serde_json::Value;

/// A column: a header and the ordered fallback field names to read from each
/// row (later names used only if earlier ones are absent/null).
pub type Column = (&'static str, &'static [&'static str]);

/// Pretty-print the raw JSON payload (for `--json`).
pub fn json(value: &Value) {
    match serde_json::to_string_pretty(value) {
        Ok(s) => println!("{s}"),
        Err(_) => println!("{value}"),
    }
}

/// Render a list payload as an aligned table. Empty payloads print a hint.
pub fn table(value: &Value, columns: &[Column]) {
    let rows = models::rows(value);
    if rows.is_empty() {
        println!("(none)");
        return;
    }

    let headers: Vec<&str> = columns.iter().map(|(h, _)| *h).collect();
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();

    let mut cells: Vec<Vec<String>> = Vec::with_capacity(rows.len());
    for row in &rows {
        let line: Vec<String> = columns
            .iter()
            .map(|(_, names)| models::cell_of(row, names))
            .collect();
        for (i, c) in line.iter().enumerate() {
            widths[i] = widths[i].max(display_width(c));
        }
        cells.push(line);
    }

    print_row(&headers.iter().map(|h| h.to_string()).collect::<Vec<_>>(), &widths);
    println!(
        "{}",
        widths
            .iter()
            .map(|w| "-".repeat(*w))
            .collect::<Vec<_>>()
            .join("  ")
    );
    for line in &cells {
        print_row(line, &widths);
    }
}

/// Render a single object as a `key  value` detail block.
pub fn detail(value: &Value, fields: &[Column]) {
    let label_width = fields.iter().map(|(l, _)| l.len()).max().unwrap_or(0);
    for (label, names) in fields {
        let v = models::cell_of(value, names);
        if v.is_empty() {
            continue;
        }
        println!("{label:<label_width$}  {v}");
    }
}

/// Dump every scalar top-level key of an object as `key  value` — used by
/// `insights:summary`, `imports:migration`, and `edge:usage`, which display
/// whatever fields the server returns.
pub fn dump(value: &Value) {
    let Some(obj) = value.as_object() else {
        json(value);
        return;
    };
    let label_width = obj.keys().map(|k| k.len()).max().unwrap_or(0);
    for (key, v) in obj {
        let rendered = match v {
            Value::Array(a) => format!("({} items)", a.len()),
            Value::Object(_) => v.to_string(),
            _ => models::cell(Some(v)),
        };
        println!("{key:<label_width$}  {rendered}");
    }
}

fn print_row(cells: &[String], widths: &[usize]) {
    let mut out = String::new();
    for (i, cell) in cells.iter().enumerate() {
        if i > 0 {
            out.push_str("  ");
        }
        let pad = widths[i].saturating_sub(display_width(cell));
        out.push_str(cell);
        if i + 1 < cells.len() {
            out.push_str(&" ".repeat(pad));
        }
    }
    println!("{}", out.trim_end());
}

fn display_width(s: &str) -> usize {
    s.chars().count()
}
