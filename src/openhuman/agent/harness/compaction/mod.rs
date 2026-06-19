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
//! Compressors: search results, build/test logs, unified diffs, and JSON
//! arrays (tabular; large arrays additionally row-dropped with a reversible
//! CCR offload — see [`store`]). The system-prompt cache-aligner
//! ([`cache_align`]) runs warn-only from `ContextManager::build_system_prompt`.
//! Every lossy path is first/last/high-signal-preserving or recoverable via
//! the `retrieve_tool_output` tool, so it is safe under the always-on default.

pub mod cache_align;
pub mod detect;
pub mod diff;
pub mod json_crusher;
pub mod logs;
pub mod search;
pub mod signals;
pub mod store;

#[cfg(test)]
mod demo;
#[cfg(test)]
mod measure;

use detect::{hint_for_tool, resolve, ContentType};

/// Outputs below this many bytes are never compressed — they're already cheap
/// and the structural compressors add overhead (markers) that can outweigh the
/// saving. Matches the spirit of the plan's `min_bytes_to_compress`.
pub const MIN_BYTES_TO_COMPRESS: usize = 2048;

/// Compress a tool's output for the model context, routed by the tool name.
///
/// Returns the (possibly) compacted string. Always falls back to the original
/// when: compaction is disabled, the output is small, the content type isn't
/// one we compress, or compression wouldn't shrink it. The result still flows
/// through the downstream byte budget, so this can only ever *help*.
pub fn compact_tool_output(content: String, tool_name: &str, enabled: bool) -> String {
    if !enabled || content.len() < MIN_BYTES_TO_COMPRESS {
        return content;
    }

    let hint = hint_for_tool(tool_name);
    let content_type = resolve(hint, &content);

    let compressed = match content_type {
        ContentType::Search => search::compress(&content),
        ContentType::Log => logs::compress(&content),
        ContentType::Diff => diff::compress(&content),
        ContentType::JsonArray => json_crusher::compress(&content),
        // Plain text has no structural compressor (the ML text compressor is
        // intentionally out of scope) — pass through to the byte budget.
        ContentType::PlainText => None,
    };

    match compressed {
        Some(out) if out.len() < content.len() => {
            let ratio = 1.0 - (out.len() as f64 / content.len() as f64);
            // `::log` is the logging crate (the sibling `logs` module shadows
            // the bare `log` path inside this module).
            ::log::debug!(
                "[compaction] tool={tool_name} type={content_type:?} in_bytes={} out_bytes={} ratio={ratio:.2}",
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
    fn large_search_is_compacted() {
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
        assert!(out.len() < s.len(), "expected compaction");
        assert!(out.contains("more match(es) in"));
    }

    #[test]
    fn unknown_tool_plain_text_passthrough() {
        let prose = "lorem ipsum ".repeat(400); // > MIN, but plain text
        let out = compact_tool_output(prose.clone(), "some_tool", true);
        assert_eq!(out, prose);
    }
}
