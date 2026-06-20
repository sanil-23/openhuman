//! Native tool-output compaction (Stage 1a).
//!
//! Content-aware compression of large tool outputs, applied in
//! `Agent::execute_tool_call` **before** the byte-cap truncation in
//! [`crate::openhuman::context::tool_result_budget`] (Stage 1) and before the
//! result enters conversation history. Operates on fresh bytes that have not
//! been sent to the backend, so — like Stage 1 — it never mutates
//! previously-sent history and cannot bust the provider KV-cache prefix.
//!
//! This is a clean-room Rust port of the deterministic (non-ML) compressors
//! from headroom (<https://github.com/chopratejas/headroom>, Apache-2.0):
//! content routing + grep/log/diff compaction. The ML text/image compressors
//! are intentionally out of scope (no Python, no ONNX, no model download).
//!
//! See `compaction-plan.md` for the full design. The downstream byte cap lives
//! in [`crate::openhuman::context::tool_result_budget`]; this stage runs just
//! ahead of it in `agent_tool_exec::run_agent_tool_call`.
//!
//! Compressors: build/test logs, unified diffs, and JSON arrays (tabular;
//! large arrays additionally row-dropped with a reversible CCR offload — see
//! [`store`]). The system-prompt cache-aligner ([`cache_align`]) runs warn-only
//! from `ContextManager::build_system_prompt`. Every lossy path is recoverable
//! via the `retrieve_tool_output` tool, so it is safe under the always-on
//! default.
//!
//! **Search/grep output is intentionally not compacted** — see the router in
//! [`compact_tool_output`]. It's a completeness tool; structured match-dropping
//! does more harm than the tokens it saves.

pub mod cache_align;
pub mod detect;
pub mod diff;
pub mod json_crusher;
pub mod logs;
pub mod signals;
pub mod store;

#[cfg(test)]
mod demo;
#[cfg(test)]
mod measure;

use detect::{hint_for_tool, resolve, ContentType};
use std::fmt::Write as _;

/// Outputs below this many bytes are never compressed — they're already cheap
/// and the structural compressors add overhead (markers) that can outweigh the
/// saving. Matches the spirit of the plan's `min_bytes_to_compress`.
pub const MIN_BYTES_TO_COMPRESS: usize = 2048;

/// Tools whose output must never be compacted. `retrieve_tool_output` exists
/// precisely to return a previously-compacted original *in full* — running it
/// back through Stage 1a would re-compact the recovery and the agent could
/// never actually get the dropped data. Keep this in sync with the tool's
/// `name()` in `tools/impl/system/retrieve_tool_output.rs`.
pub const NEVER_COMPACT_TOOLS: &[&str] = &["retrieve_tool_output"];

/// Result of a single compressor: the compacted body plus whether any data was
/// dropped from it. `lossy` drives reversibility — a lossy body has its
/// original offloaded to the CCR store and gets the retrieval footer, a
/// lossless one (e.g. the JSON table, which only reformats) does not.
pub struct Compacted {
    pub text: String,
    pub lossy: bool,
}

impl Compacted {
    /// All information preserved — only formatting/whitespace changed.
    pub fn lossless(text: String) -> Self {
        Self { text, lossy: false }
    }
    /// Data was dropped — the original must be offloaded for recovery.
    pub fn lossy(text: String) -> Self {
        Self { text, lossy: true }
    }
}

/// Compress a tool's output for the model context, routed by the tool name.
///
/// Returns the (possibly) compacted string. Always falls back to the original
/// when: compaction is disabled, the output is small, the content type isn't
/// one we compress, or compression wouldn't shrink it. The result still flows
/// through the downstream byte budget, so this can only ever *help*.
///
/// **Reversibility / honesty:** whenever a compressor drops data (`lossy`), the
/// full original is stashed in the [`store`] (CCR) and the returned text ends
/// with an explicit footer telling the model this is a partial view and how to
/// fetch the original via `retrieve_tool_output("<hash>")`. So the model is
/// never silently handed a truncated result, and nothing is unrecoverable.
pub fn compact_tool_output(content: String, tool_name: &str, enabled: bool) -> String {
    if !enabled || content.len() < MIN_BYTES_TO_COMPRESS || NEVER_COMPACT_TOOLS.contains(&tool_name)
    {
        return content;
    }

    let hint = hint_for_tool(tool_name);
    let content_type = resolve(hint, &content);

    let compressed = match content_type {
        // Search/grep output is deliberately NOT compacted. grep is a
        // completeness tool — the agent runs it to find *every* call site —
        // and dropping matches (even with a recovery footer) risks it acting
        // on a partial set, with a relevance heuristic that doesn't apply to
        // search results anyway. Large grep output is left to the downstream
        // byte budget, which persists the full result to a `file_read`-able
        // artifact rather than dropping it. See the design note in the PR.
        ContentType::Search => None,
        ContentType::Log => logs::compress(&content),
        ContentType::Diff => diff::compress(&content),
        ContentType::JsonArray => json_crusher::compress(&content),
        // Plain text has no structural compressor (the ML text compressor is
        // intentionally out of scope) — pass through to the byte budget.
        ContentType::PlainText => None,
    };

    match compressed {
        Some(c) if c.text.len() < content.len() => {
            let mut out = c.text;
            if c.lossy {
                // Offload the full original and tell the model — explicitly —
                // that this is a partial view it can expand on demand.
                let hash = store::offload(&content);
                let _ = write!(
                    out,
                    "\n\n[compacted tool output — this is a partial view; \
                     the full original ({} bytes) is available by calling \
                     retrieve_tool_output(\"{hash}\")]",
                    content.len()
                );
            }
            let ratio = 1.0 - (out.len() as f64 / content.len() as f64);
            // `::log` is the logging crate (the sibling `logs` module shadows
            // the bare `log` path inside this module).
            ::log::debug!(
                "[compaction] tool={tool_name} type={content_type:?} lossy={} in_bytes={} out_bytes={} ratio={ratio:.2}",
                c.lossy,
                content.len(),
                out.len(),
            );
            out
        }
        _ => content,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;

    #[test]
    fn disabled_is_passthrough() {
        let big = "x".repeat(MIN_BYTES_TO_COMPRESS + 10);
        assert_eq!(compact_tool_output(big.clone(), "grep", false), big);
    }

    #[test]
    fn small_output_passthrough() {
        let small = "a.rs:1:hit\nb.rs:2:hit".to_string();
        assert_eq!(compact_tool_output(small.clone(), "grep", true), small);
    }

    #[test]
    fn search_output_is_not_compacted() {
        // grep is a completeness tool — its output must pass through untouched
        // even when large, so the agent never acts on a silently-dropped subset.
        let mut s = String::from("80 match(es); scanned 2 file(s)\n");
        for i in 1..=40 {
            let _ = writeln!(
                s,
                "src/a.rs:{i}:let value_{i} = compute_something_long_{i}();"
            );
        }
        for i in 1..=40 {
            let _ = writeln!(
                s,
                "src/b.rs:{i}:fn helper_function_number_{i}() {{ /* body */ }}"
            );
        }
        assert!(s.len() >= MIN_BYTES_TO_COMPRESS);
        let out = compact_tool_output(s.clone(), "grep", true);
        assert_eq!(out, s, "search output must pass through unchanged");
    }

    #[test]
    fn unknown_tool_plain_text_passthrough() {
        let prose = "lorem ipsum ".repeat(400); // > MIN, but plain text
        let out = compact_tool_output(prose.clone(), "some_tool", true);
        assert_eq!(out, prose);
    }

    /// Pull the CCR hash out of the retrieval footer the model is shown.
    fn footer_hash(out: &str) -> Option<&str> {
        out.split("retrieve_tool_output(\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
    }

    #[test]
    fn retrieval_returns_the_full_original_uncompacted() {
        // End-to-end recovery: a large JSON result is compacted (lossy) and its
        // original offloaded; the model reads the footer hash and calls
        // retrieve_tool_output. That tool's output flows back through Stage 1a —
        // and must NOT be re-compacted, or the agent could never see the full
        // data it asked for.
        let mut rows = Vec::new();
        for i in 0..120 {
            rows.push(format!(
                r#"{{"id":{i},"name":"account_{i}","email":"a{i}@ex.com","tier":"gold"}}"#
            ));
        }
        let original = format!("[{}]", rows.join(","));

        // 1. First pass compacts + offloads.
        let compacted = compact_tool_output(original.clone(), "list_accounts", true);
        assert!(compacted.contains("retrieve_tool_output("));
        let hash = footer_hash(&compacted).expect("footer hash");

        // 2. The retrieve tool fetches the original from CCR.
        let fetched = store::retrieve(hash).expect("CCR has it");
        assert_eq!(fetched, original);

        // 3. That fetched output passes through Stage 1a under the retrieve
        //    tool's name — and must come back byte-for-byte, NOT re-compacted.
        let delivered = compact_tool_output(fetched, "retrieve_tool_output", true);
        assert_eq!(
            delivered, original,
            "recovery must deliver the full original"
        );
        assert!(!delivered.contains("partial view"));
    }

    #[test]
    fn every_lossy_output_tells_the_model_and_is_recoverable() {
        // One representative input per lossy compressor. Each must (a) carry the
        // explicit "partial view / retrieve_tool_output" footer, and (b) have
        // its full original recoverable byte-for-byte from the CCR store — i.e.
        // no information is actually lost, only deferred. (grep is excluded —
        // search output is intentionally never compacted.)
        let mut log = String::new();
        for i in 0..200 {
            let _ = writeln!(log, "   Compiling crate_{i} v0.{i}.0");
        }
        let _ = writeln!(log, "error: aborting");
        let mut diff = String::from("diff --git a/x.rs b/x.rs\n@@ -1,60 +1,61 @@\n");
        for i in 0..50 {
            let _ = writeln!(
                diff,
                " unchanged context line {i} carried along by the diff"
            );
        }
        let _ = writeln!(diff, "+changed");
        let mut jrows = Vec::new();
        for i in 0..120 {
            jrows.push(format!(
                r#"{{"id":{i},"name":"account_{i}","email":"a{i}@ex.com","tier":"gold"}}"#
            ));
        }
        let json = format!("[{}]", jrows.join(","));

        for (tool, input) in [
            ("run_tests", log),
            ("read_diff", diff),
            ("list_accounts", json),
        ] {
            let out = compact_tool_output(input.clone(), tool, true);
            assert!(out.len() < input.len(), "{tool}: not compacted");
            // (a) the model is explicitly told this is a partial view.
            assert!(
                out.contains("partial view") && out.contains("retrieve_tool_output("),
                "{tool}: missing retrieval footer:\n{out}"
            );
            // (b) the full original is recoverable byte-for-byte.
            let hash = footer_hash(&out).expect("footer has a hash");
            assert_eq!(
                store::retrieve(hash).as_deref(),
                Some(input.as_str()),
                "{tool}: CCR did not round-trip"
            );
        }
    }

    #[test]
    fn lossless_table_preserves_every_value_and_has_no_footer() {
        // A JSON list under the row-drop threshold is reformatted, not dropped:
        // every value must still be present, and there must be NO retrieval
        // footer (nothing was lost, so nothing to retrieve).
        // 38 rows: above the 2048-byte floor, below the 40-row drop threshold
        // → a full lossless table.
        let mut rows = Vec::new();
        for i in 0..38 {
            rows.push(format!(
                r#"{{"id":{i},"sku":"SKU-{i:05}","name":"widget number {i} in the catalog","warehouse":"east-region-1"}}"#
            ));
        }
        let input = format!("[{}]", rows.join(","));
        assert!(input.len() >= MIN_BYTES_TO_COMPRESS);
        let out = compact_tool_output(input.clone(), "list_inventory", true);
        assert!(out.len() < input.len(), "table should shrink");
        assert!(
            !out.contains("retrieve_tool_output("),
            "lossless ⇒ no footer"
        );
        assert!(!out.contains("omitted"), "lossless ⇒ nothing omitted");
        // Every row's identifying values survive the reformat.
        for i in 0..38 {
            assert!(out.contains(&format!("SKU-{i:05}")), "lost SKU {i}");
            assert!(
                out.contains(&format!("widget number {i} in the catalog")),
                "lost name {i}"
            );
        }
    }
}
