//! Over the markup captured from a live seat, verbatim.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use super::{looks_like_markup, parse_markup};
use serde_json::json;

/// Captured from `industry_analyst`, episode `98ed172b`, after ten
/// compressions. Two invokes in one block, a string parameter and a
/// non-string one, prose in front.
const CAPTURED: &str = "\n\nI have the Gartner data. Let me check the deck.\n\n\
<｜DSML｜tool_calls>\n\
<｜DSML｜invoke name=\"web_fetch\">\n\
<｜DSML｜parameter name=\"url\" string=\"true\">https://example.com/report</｜DSML｜parameter>\n\
<｜DSML｜parameter name=\"max_bytes\" string=\"false\">4000</｜DSML｜parameter>\n\
</｜DSML｜invoke>\n\
<｜DSML｜invoke name=\"file_read\">\n\
<｜DSML｜parameter name=\"path\" string=\"true\">artifacts/call_089.txt</｜DSML｜parameter>\n\
</｜DSML｜invoke>\n\
</｜DSML｜tool_calls>";

#[test]
fn reads_every_invoke_in_a_captured_block() {
    assert!(looks_like_markup(CAPTURED));
    let calls = parse_markup(CAPTURED);
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert_eq!(calls[0].name, "web_fetch");
    assert_eq!(calls[0].arguments["url"], json!("https://example.com/report"));
    assert_eq!(
        calls[0].arguments["max_bytes"],
        json!(4000),
        "`string=\"false\"` is a JSON value, not text"
    );
    assert_eq!(calls[1].name, "file_read");
    assert_eq!(calls[1].arguments["path"], json!("artifacts/call_089.txt"));
    assert_ne!(calls[0].id, calls[1].id, "each call needs its own id");
}

#[test]
fn ordinary_prose_is_not_a_call() {
    for text in [
        "I could not verify that claim.",
        "Use `file_read` next time.",
        "",
    ] {
        assert!(!looks_like_markup(text), "{text}");
        assert!(parse_markup(text).is_empty(), "{text}");
    }
}

/// A block cut off by the token cap still names its tool, and the arguments it
/// finished are worth more than the silence.
#[test]
fn a_truncated_block_keeps_what_it_finished() {
    let cut = "<｜DSML｜tool_calls>\n\
<｜DSML｜invoke name=\"file_write\">\n\
<｜DSML｜parameter name=\"path\" string=\"true\">out.md</｜DSML｜parameter>\n\
<｜DSML｜parameter name=\"content\" string=\"true\"># half a fi";
    let calls = parse_markup(cut);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "file_write");
    assert_eq!(calls[0].arguments["path"], json!("out.md"));
    assert!(
        calls[0].arguments.get("content").is_none(),
        "an unterminated parameter is dropped, not guessed at"
    );
}

/// The fullwidth delimiter is the whole point: the ASCII spelling is a
/// different string and must not match.
#[test]
fn the_ascii_spelling_is_not_this_dialect() {
    let ascii = "<|DSML|tool_calls>\n<|DSML|invoke name=\"file_read\">\n";
    assert!(!looks_like_markup(ascii));
    assert!(parse_markup(ascii).is_empty());
}
