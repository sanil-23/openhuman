//! JSON-array crusher.
//!
//! Clean-room port of the *lossless* core of headroom's `SmartCrusher`
//! (Apache-2.0): an array of objects that repeat the same keys is the single
//! most common bloated tool output (API list responses, DB rows, search
//! manifests). Re-rendering it as a table emits each key **once** instead of
//! per row, dropping the repeated key names and JSON punctuation.
//!
//! Up to [`ROW_DROP_THRESHOLD`] rows the transform is **lossless** — every
//! value is preserved (nested values render as compact JSON in their cell).
//! Above it, the table is additionally **row-dropped** (head + tail kept) and
//! the full original is offloaded to the CCR store ([`super::store`]) behind a
//! `retrieve_tool_output("<hash>")` marker, so the dropped middle stays
//! recoverable on demand and aggressive compaction is safe always-on.

use serde_json::Value;
use std::fmt::Write as _;

/// Minimum rows before tabular rendering is worth the header overhead.
pub const MIN_ROWS: usize = 3;
/// Above this many rows the table is *also* row-dropped: head + tail rows are
/// kept and the full original is offloaded to CCR behind a retrieval marker.
pub const ROW_DROP_THRESHOLD: usize = 40;
/// Rows kept from the head when row-dropping.
pub const HEAD_ROWS: usize = 20;
/// Rows kept from the tail when row-dropping.
pub const TAIL_ROWS: usize = 10;

/// Compress a JSON array-of-objects into a compact table. Returns `None` when
/// the content isn't a uniform-enough array of objects or wouldn't shrink.
pub fn compress(content: &str) -> Option<String> {
    let value: Value = serde_json::from_str(content.trim()).ok()?;
    let array = value.as_array()?;
    if array.len() < MIN_ROWS {
        return None;
    }
    // Every element must be an object for a clean table; mixed arrays bail.
    if !array.iter().all(Value::is_object) {
        return None;
    }

    // Column order = first-seen key order across all rows (union, stable).
    let mut columns: Vec<String> = Vec::new();
    for item in array {
        if let Some(obj) = item.as_object() {
            for key in obj.keys() {
                if !columns.iter().any(|c| c == key) {
                    columns.push(key.clone());
                }
            }
        }
    }
    if columns.len() < 2 {
        return None;
    }

    // Render every row's cells up front so we can choose full vs. row-dropped.
    let mut rows: Vec<String> = Vec::with_capacity(array.len());
    for item in array {
        let obj = item.as_object()?;
        let cells: Vec<String> = columns
            .iter()
            .map(|col| match obj.get(col) {
                None | Some(Value::Null) => String::new(),
                Some(v) => render_cell(v),
            })
            .collect();
        rows.push(cells.join(" | "));
    }

    let lossy = rows.len() > ROW_DROP_THRESHOLD;
    let mut out = String::with_capacity(content.len());
    let _ = writeln!(
        out,
        "[json table: {} rows × {} columns — values are JSON; empty = key absent]",
        rows.len(),
        columns.len()
    );
    let _ = writeln!(out, "{}", columns.join(" | "));

    if lossy {
        // Keep head + tail; offload the full original so the dropped middle is
        // recoverable via the retrieve_tool_output tool.
        let hash = super::store::offload(content);
        let dropped = rows.len() - HEAD_ROWS - TAIL_ROWS;
        for row in rows.iter().take(HEAD_ROWS) {
            let _ = writeln!(out, "{row}");
        }
        let _ = writeln!(
            out,
            "[... {dropped} rows omitted · full original via retrieve_tool_output(\"{hash}\") ...]"
        );
        for row in rows.iter().skip(rows.len() - TAIL_ROWS) {
            let _ = writeln!(out, "{row}");
        }
    } else {
        for row in &rows {
            let _ = writeln!(out, "{row}");
        }
    }

    let out = out.trim_end().to_string();
    if out.len() >= content.len() {
        None
    } else {
        Some(out)
    }
}

/// Render a single cell. Scalars print bare-ish (strings unquoted unless they
/// contain the column separator); nested values stay as compact JSON so the
/// table remains lossless.
fn render_cell(v: &Value) -> String {
    match v {
        Value::String(s) if !s.contains('|') && !s.contains('\n') => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crushes_uniform_array() {
        let mut rows = Vec::new();
        for i in 0..20 {
            rows.push(format!(
                r#"{{"id":{i},"name":"item number {i}","status":"active","owner":"team-alpha"}}"#
            ));
        }
        let input = format!("[{}]", rows.join(","));
        let out = compress(&input).expect("compresses");
        // Header + key names appear once, not 20× (column order is whatever
        // serde_json yields — don't assume insertion order here).
        assert_eq!(out.matches("status").count(), 1, "{out}");
        for col in ["id", "name", "status", "owner"] {
            assert!(out.lines().nth(1).unwrap().contains(col), "missing {col}");
        }
        // Data preserved.
        assert!(out.contains("item number 7"));
        assert!(out.contains("19"));
        assert!(out.len() < input.len(), "expected shrink");
    }

    #[test]
    fn preserves_nested_values_losslessly() {
        let input = r#"[
          {"id":1,"tags":["a","b"],"meta":{"k":1}},
          {"id":2,"tags":["c"],"meta":{"k":2}},
          {"id":3,"tags":[],"meta":{"k":3}}
        ]"#;
        let out = compress(input).expect("compresses");
        assert!(out.contains(r#"["a","b"]"#), "{out}");
        assert!(out.contains(r#"{"k":1}"#), "{out}");
    }

    #[test]
    fn handles_missing_keys() {
        // Enough rows with longish values that dropping repeated keys shrinks.
        let mut rows = Vec::new();
        for i in 0..12 {
            rows.push(format!(
                r#"{{"alpha":{i},"bravo":"value string {i}","charlie":"another value {i}"}}"#
            ));
        }
        rows.push(r#"{"alpha":99}"#.to_string()); // missing bravo/charlie
        let input = format!("[{}]", rows.join(","));
        let out = compress(&input).expect("compresses");
        let header = out.lines().nth(1).unwrap();
        for col in ["alpha", "bravo", "charlie"] {
            assert!(header.contains(col), "header missing {col}: {header}");
        }
        assert!(out.len() < input.len());
    }

    #[test]
    fn large_array_row_drops_with_recoverable_ccr() {
        let mut rows = Vec::new();
        for i in 0..200 {
            rows.push(format!(
                r#"{{"id":{i},"name":"record number {i}","status":"active","note":"some detail {i}"}}"#
            ));
        }
        let input = format!("[{}]", rows.join(","));
        let out = compress(&input).expect("compresses");

        // Row-drop marker present with a retrieval hash.
        assert!(out.contains("rows omitted"), "{out}");
        let marker = out
            .lines()
            .find(|l| l.contains("retrieve_tool_output("))
            .expect("has retrieval marker");
        let hash = marker
            .split("retrieve_tool_output(\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();

        // Head + tail rows survive; the middle is dropped.
        assert!(out.contains("record number 0"), "{out}");
        assert!(out.contains("record number 199"), "{out}");
        assert!(
            !out.contains("record number 100"),
            "middle should be dropped"
        );

        // CCR round-trip returns the exact original.
        assert_eq!(
            super::super::store::retrieve(hash).as_deref(),
            Some(input.as_str())
        );
        assert!(out.len() < input.len());
    }

    #[test]
    fn non_array_returns_none() {
        assert!(compress(r#"{"a":1}"#).is_none());
        assert!(compress("[1,2,3]").is_none());
        assert!(compress(r#"[{"a":1}]"#).is_none()); // too few rows
    }
}
