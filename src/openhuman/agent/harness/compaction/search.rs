//! Search-result compressor (grep / ripgrep output).
//!
//! Clean-room port of headroom's `SearchCompressor` (Apache-2.0):
//!
//! 1. Parse `path:line:content` lines (Windows drive + dashes safe, via
//!    [`super::detect::parse_search_line`]).
//! 2. Group by file; score each match with [`super::signals::line_score`].
//! 3. Sort files by total match score; cap to [`MAX_FILES`].
//! 4. Per file: always keep first + last match, fill remaining slots by score
//!    up to [`MAX_PER_FILE`], then re-sort survivors back into line order.
//! 5. Cap total matches at [`MAX_TOTAL`]; replace dropped matches with a
//!    `[... and N more matches in <file>]` summary so the model knows there's
//!    more and can re-run with a narrower query.
//!
//! Lossy-but-bounded: first/last/highest-signal hits per file always survive,
//! and the omitted count is always surfaced. No CCR needed at this stage.

use super::detect::parse_search_line;
use super::signals::line_score;
use std::fmt::Write as _;

/// Max matches kept per file before the rest collapse into a summary line.
pub const MAX_PER_FILE: usize = 5;
/// Max matches kept across the whole result.
pub const MAX_TOTAL: usize = 30;
/// Max files kept before lower-scoring files are dropped entirely.
pub const MAX_FILES: usize = 15;

struct Match {
    line_number: u64,
    body: String,
    score: f32,
    order: usize,
}

struct FileGroup {
    path: String,
    matches: Vec<Match>,
    total_seen: usize,
}

impl FileGroup {
    fn total_score(&self) -> f32 {
        self.matches.iter().map(|m| m.score).sum()
    }
}

/// Compress grep/ripgrep output. Returns `None` when the content has no
/// parseable matches (caller passes it through unchanged) or when compression
/// would not shrink it.
pub fn compress(content: &str) -> Option<String> {
    // Preserve any header line the tool emitted before the matches (grep.rs
    // prints a "N match(es); scanned M file(s)" header) so the count survives.
    let mut header: Option<&str> = None;
    let mut groups: Vec<FileGroup> = Vec::new();
    let mut order = 0usize;

    for line in content.lines() {
        match parse_search_line(line) {
            Some((path, line_number, body)) => {
                order += 1;
                let score = line_score(body);
                let m = Match {
                    line_number,
                    body: body.to_string(),
                    score,
                    order,
                };
                if let Some(g) = groups.iter_mut().find(|g| g.path == path) {
                    g.matches.push(m);
                    g.total_seen += 1;
                } else {
                    groups.push(FileGroup {
                        path: path.to_string(),
                        matches: vec![m],
                        total_seen: 1,
                    });
                }
            }
            None => {
                // Keep the first non-match, non-blank line as a header (the
                // grep summary). Everything else that isn't a match is dropped.
                if header.is_none() && !line.trim().is_empty() {
                    header = Some(line);
                }
            }
        }
    }

    let total_matches: usize = groups.iter().map(|g| g.total_seen).sum();
    if groups.is_empty() {
        return None;
    }

    // Sort files by aggregate score (desc), then by first appearance for
    // stability; cap to MAX_FILES.
    groups.sort_by(|a, b| {
        b.total_score()
            .partial_cmp(&a.total_score())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                a.matches
                    .first()
                    .map(|m| m.order)
                    .cmp(&b.matches.first().map(|m| m.order))
            })
    });
    let files_dropped = groups.len().saturating_sub(MAX_FILES);
    groups.truncate(MAX_FILES);

    // Restore file order by first appearance for readable output.
    groups.sort_by_key(|g| g.matches.iter().map(|m| m.order).min().unwrap_or(0));

    // Per-file selection under a shared total budget.
    let mut remaining_total = MAX_TOTAL;
    let mut out = String::with_capacity(content.len() / 2 + 64);
    if let Some(h) = header {
        let _ = writeln!(out, "{h}");
    }

    for g in &mut groups {
        if remaining_total == 0 {
            break;
        }
        let per_file_cap = MAX_PER_FILE.min(remaining_total);
        let kept = select_matches(&mut g.matches, per_file_cap);
        remaining_total = remaining_total.saturating_sub(kept.len());

        for m in &kept {
            let _ = writeln!(out, "{}:{}:{}", g.path, m.line_number, m.body);
        }
        let omitted = g.total_seen.saturating_sub(kept.len());
        if omitted > 0 {
            let _ = writeln!(out, "[... and {omitted} more match(es) in {}]", g.path);
        }
    }

    if files_dropped > 0 {
        let _ = writeln!(
            out,
            "[... and {files_dropped} more file(s) with lower-signal matches omitted]"
        );
    }

    let _ = total_matches; // (kept for clarity; counts surfaced per-file)

    // Guard: never emit something larger than the input.
    if out.len() >= content.len() {
        None
    } else {
        Some(out.trim_end().to_string())
    }
}

/// Pick up to `cap` matches from a file's matches: always keep the first and
/// last by line order, fill the rest by descending score, then return them
/// sorted back into line order.
fn select_matches(matches: &mut [Match], cap: usize) -> Vec<&Match> {
    // Stable order by appearance for first/last determination.
    let mut by_order: Vec<usize> = (0..matches.len()).collect();
    by_order.sort_by_key(|&i| matches[i].order);

    if matches.len() <= cap {
        return by_order.iter().map(|&i| &matches[i]).collect();
    }
    if cap == 0 {
        return Vec::new();
    }

    let mut keep: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
    // Always keep first; keep last when cap allows more than one.
    keep.insert(by_order[0]);
    if cap >= 2 {
        keep.insert(by_order[by_order.len() - 1]);
    }

    // Fill remaining slots by descending score (ties broken by order).
    let mut by_score: Vec<usize> = (0..matches.len()).collect();
    by_score.sort_by(|&a, &b| {
        matches[b]
            .score
            .partial_cmp(&matches[a].score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| matches[a].order.cmp(&matches[b].order))
    });
    for &i in &by_score {
        if keep.len() >= cap {
            break;
        }
        keep.insert(i);
    }

    // Return sorted by line order.
    let mut kept: Vec<usize> = keep.into_iter().collect();
    kept.sort_by_key(|&i| matches[i].order);
    kept.iter().map(|&i| &matches[i]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn big_search() -> String {
        let mut s = String::from("40 match(es); scanned 3 file(s)\n");
        for i in 1..=20 {
            let _ = writeln!(s, "src/a.rs:{i}:let value_{i} = compute();");
        }
        for i in 1..=20 {
            let _ = writeln!(s, "src/b.rs:{i}:fn helper_{i}() {{}}");
        }
        s
    }

    #[test]
    fn caps_per_file_and_keeps_first_last() {
        let out = compress(&big_search()).expect("compresses");
        // First and last match of a.rs must survive.
        assert!(out.contains("src/a.rs:1:"), "{out}");
        assert!(out.contains("src/a.rs:20:"), "{out}");
        // Per-file cap enforced.
        let a_lines = out.matches("src/a.rs:").count();
        assert!(a_lines <= MAX_PER_FILE, "a.rs kept {a_lines}");
        // Omission surfaced.
        assert!(out.contains("more match(es) in src/a.rs"), "{out}");
        // Header preserved.
        assert!(out.contains("scanned 3 file(s)"));
        // Smaller than input.
        assert!(out.len() < big_search().len());
    }

    #[test]
    fn total_cap_enforced() {
        let mut s = String::new();
        for f in 0..20 {
            for i in 1..=10 {
                let _ = writeln!(s, "f{f}.rs:{i}:line {i}");
            }
        }
        let out = compress(&s).expect("compresses");
        let kept: usize = out
            .lines()
            .filter(|l| parse_search_line(l).is_some())
            .count();
        assert!(kept <= MAX_TOTAL, "kept {kept} > {MAX_TOTAL}");
    }

    #[test]
    fn error_lines_preferred() {
        // Enough plain matches that dropping some genuinely shrinks the output,
        // with a single high-signal ERROR line buried in the middle. The error
        // line must survive selection over the surrounding baseline lines.
        let mut s = String::new();
        for i in 1..=20 {
            if i == 10 {
                let _ = writeln!(s, "x.rs:{i}:ERROR something went badly wrong right here");
            } else {
                let _ = writeln!(
                    s,
                    "x.rs:{i}:ordinary matching line number {i} nothing notable"
                );
            }
        }
        let out = compress(&s).expect("compresses");
        assert!(out.contains("ERROR something went badly wrong"), "{out}");
    }

    #[test]
    fn no_matches_returns_none() {
        assert!(compress("just prose\nmore prose").is_none());
    }
}
