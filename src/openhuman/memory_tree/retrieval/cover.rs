//! `memory_tree_cover_window` — the minimum-node cover of a time window.
//!
//! Given an explicit `[since_ms, until_ms]` window (and optional source
//! filter), return the **smallest set of nodes that covers every in-window
//! chunk** — a heterogeneous mix of summary nodes (where a whole subtree is
//! in-window) and raw leaf chunks (everything else). This is the read path the
//! morning brief uses for "last 24h" so it summarises only fresh content
//! instead of the all-time root blob.
//!
//! The cover is **purely structural** — it does not assume the tree is
//! calendar-bucketed (it isn't: `bucket_seal` groups by `SUMMARY_FANOUT`, so
//! `level` is a depth integer, not hour/day). Two passes, per source tree:
//!
//! 1. **Eligible summaries** — every non-deleted summary whose time-range
//!    envelope is fully inside the window
//!    (`store::list_summaries_in_window`). Because seal sets the envelope to
//!    `MIN/MAX` of children, "envelope ⊆ window" ⇔ "all descendant leaves are
//!    in the window", i.e. using the summary drags in no out-of-window
//!    content.
//! 2. **Frontier + raw fallback** — keep the topmost eligible summaries
//!    (`maximal` = eligible whose `parent_id` is not itself eligible); mark
//!    the chunks they cover (transitive descent through `child_ids`); emit any
//!    remaining in-window chunk raw. Raw chunks are the floor and cover
//!    boundary slices and not-yet-sealed content for free.
//!
//! Output is grouped by source and ordered ascending by start time so the
//! brief reads it as a per-source timeline.

use std::collections::{HashMap, HashSet};

use anyhow::Result;

use crate::openhuman::config::Config;
use crate::openhuman::memory_store::chunks::store::{list_chunks, ListChunksQuery};
use crate::openhuman::memory_store::chunks::types::{Chunk, SourceKind};
use crate::openhuman::memory_store::content::read as content_read;
use crate::openhuman::memory_store::trees::types::{SummaryNode, TreeKind};
use crate::openhuman::memory_tree::retrieval::types::{
    hit_from_chunk, hit_from_summary, QueryResponse, RetrievalHit,
};
use crate::openhuman::memory_tree::tree::store;

/// Default cap on returned cover items when the caller passes `limit = 0`.
const DEFAULT_LIMIT: usize = 200;

/// Upper bound on in-window chunks scanned across all sources. A 24h window
/// rarely exceeds this; if it does we log and truncate (the excess simply
/// doesn't appear — never silently mis-covered, since a frontier summary still
/// stands in for a whole sealed subtree).
const MAX_WINDOW_CHUNKS: usize = 5_000;

/// Entrypoint for `memory_tree_cover_window`. Blocking SQLite work runs on
/// `spawn_blocking`; the async caller stays on its runtime. Results are
/// grouped by source (`tree_scope`) and ordered ascending by start time, then
/// truncated to `limit` (`DEFAULT_LIMIT` when 0).
pub async fn cover_window(
    config: &Config,
    since_ms: i64,
    until_ms: i64,
    source_id: Option<&str>,
    source_kind: Option<SourceKind>,
    limit: usize,
) -> Result<QueryResponse> {
    let limit = if limit == 0 { DEFAULT_LIMIT } else { limit };
    if until_ms < since_ms {
        return Err(anyhow::anyhow!(
            "cover_window: until_ms ({until_ms}) precedes since_ms ({since_ms})"
        ));
    }
    log::debug!(
        "[retrieval::cover] cover_window since_ms={since_ms} until_ms={until_ms} \
         has_source_id={} source_kind={:?} limit={limit}",
        source_id.is_some(),
        source_kind.map(|k| k.as_str()),
    );

    let config_owned = config.clone();
    let source_id_owned = source_id.map(|s| s.to_string());
    let mut hits = tokio::task::spawn_blocking(move || -> Result<Vec<RetrievalHit>> {
        collect_cover(
            &config_owned,
            since_ms,
            until_ms,
            source_id_owned.as_deref(),
            source_kind,
        )
    })
    .await
    .map_err(|e| anyhow::anyhow!("cover_window join error: {e}"))??;

    // Group by source, then chronological ascending within each source. The
    // brief consumes this as a per-source timeline; ascending puts the
    // freshest content last (tool result lands at the context tail → recency).
    hits.sort_by(|a, b| {
        a.tree_scope
            .cmp(&b.tree_scope)
            .then(a.time_range_start.cmp(&b.time_range_start))
    });
    let total = hits.len();
    hits.truncate(limit);

    log::debug!(
        "[retrieval::cover] returning hits={} total={}",
        hits.len(),
        total
    );
    Ok(QueryResponse::new(hits, total))
}

/// Blocking: build the cover. **Chunk-driven, not tree-driven** — chunks are
/// written to `mem_tree_chunks` at ingest, but the per-source `Tree` row is
/// only created later by the seal worker. Iterating trees would therefore miss
/// freshly-ingested (un-sealed) sources entirely — exactly the case the raw
/// fallback exists for. So we pull the authoritative in-window chunk set first,
/// group by source, and look up each source's tree (if any) for summaries.
fn collect_cover(
    config: &Config,
    since_ms: i64,
    until_ms: i64,
    source_id: Option<&str>,
    source_kind: Option<SourceKind>,
) -> Result<Vec<RetrievalHit>> {
    let chunks = list_chunks(
        config,
        &ListChunksQuery {
            source_id: source_id.map(|s| s.to_string()),
            source_kind,
            since_ms: Some(since_ms),
            until_ms: Some(until_ms),
            limit: Some(MAX_WINDOW_CHUNKS),
            ..Default::default()
        },
    )?;
    if chunks.len() == MAX_WINDOW_CHUNKS {
        log::warn!(
            "[retrieval::cover] in-window chunk cap {MAX_WINDOW_CHUNKS} hit — \
             some raw leaves may be omitted"
        );
    }

    // Group by source (the chunk's `source_id` == its source tree's scope).
    let mut by_source: HashMap<String, Vec<Chunk>> = HashMap::new();
    for chunk in chunks {
        by_source
            .entry(chunk.metadata.source_id.clone())
            .or_default()
            .push(chunk);
    }
    log::debug!("[retrieval::cover] in-window sources n={}", by_source.len());

    let mut hits: Vec<RetrievalHit> = Vec::new();
    for (source, src_chunks) in by_source {
        cover_one_source(config, &source, since_ms, until_ms, src_chunks, &mut hits)?;
    }
    Ok(hits)
}

/// Minimum cover for one source: frontier summaries (when the source has a
/// sealed tree) plus every in-window chunk they don't already cover, raw. A
/// source with no `Tree` row (not yet processed by the seal worker) has no
/// eligible summaries, so all its in-window chunks are emitted raw.
fn cover_one_source(
    config: &Config,
    source: &str,
    since_ms: i64,
    until_ms: i64,
    chunks: Vec<Chunk>,
    out: &mut Vec<RetrievalHit>,
) -> Result<()> {
    // Look up the source's summary tree (scope == source id). Absent until the
    // seal worker first processes this source.
    let tree = store::get_tree_by_scope(config, TreeKind::Source, source)?;
    let (tree_id, eligible) = match &tree {
        Some(t) => (
            t.id.as_str(),
            store::list_summaries_in_window(config, &t.id, since_ms, until_ms)?,
        ),
        None => ("", Vec::new()),
    };
    let plan = plan_cover(&eligible);

    // Frontier summaries (hydrated to full body).
    let by_id: HashMap<&str, &SummaryNode> = eligible.iter().map(|s| (s.id.as_str(), s)).collect();
    for id in &plan.maximal_ids {
        let Some(node) = by_id.get(id.as_str()) else {
            continue;
        };
        let mut node = (*node).clone();
        match content_read::read_summary_body(config, &node.id) {
            Ok(body) => node.content = body,
            Err(e) => {
                log::warn!("[retrieval::cover] read_summary_body failed — serving preview: {e:#}")
            }
        }
        out.push(hit_from_summary(&node, source));
    }

    // In-window chunks not covered by a frontier summary, raw.
    for chunk in &chunks {
        if plan.covered_chunk_ids.contains(&chunk.id) {
            continue;
        }
        let mut chunk = chunk.clone();
        match content_read::read_chunk_body(config, &chunk.id) {
            Ok(body) => chunk.content = body,
            Err(e) => {
                log::warn!("[retrieval::cover] read_chunk_body failed — serving preview: {e:#}")
            }
        }
        out.push(hit_from_chunk(&chunk, tree_id, source, 0.0));
    }
    Ok(())
}

/// The structural result of the cover for one tree's eligible summaries:
/// which summaries to emit (the frontier) and which chunk ids they already
/// cover (so the caller can emit the rest raw). Pure — no I/O — so the cover
/// logic is unit-testable without a database.
struct CoverPlan {
    /// Topmost eligible summary ids (eligible nodes whose parent is not
    /// eligible). These stand in for their whole subtree.
    maximal_ids: Vec<String>,
    /// Leaf chunk ids transitively covered by the `maximal` summaries.
    covered_chunk_ids: HashSet<String>,
}

/// Compute the frontier + covered-chunk set from a tree's eligible summaries.
///
/// `eligible` must be exactly the summaries whose envelope is inside the
/// window (all levels). A summary is **maximal** when its `parent_id` is not
/// itself eligible. Coverage descends `child_ids`: an id that is not another
/// eligible summary is a leaf chunk id (because every descendant of a
/// fully-covered node is itself fully covered, hence eligible — so any child
/// not in the eligible set is a leaf).
fn plan_cover(eligible: &[SummaryNode]) -> CoverPlan {
    let eligible_ids: HashSet<&str> = eligible.iter().map(|s| s.id.as_str()).collect();
    let by_id: HashMap<&str, &SummaryNode> = eligible.iter().map(|s| (s.id.as_str(), s)).collect();

    let maximal: Vec<&SummaryNode> = eligible
        .iter()
        .filter(|s| match &s.parent_id {
            Some(parent) => !eligible_ids.contains(parent.as_str()),
            None => true,
        })
        .collect();

    let mut covered_chunk_ids: HashSet<String> = HashSet::new();
    for node in &maximal {
        collect_descendant_chunks(node, &by_id, &mut covered_chunk_ids);
    }

    CoverPlan {
        maximal_ids: maximal.iter().map(|s| s.id.clone()).collect(),
        covered_chunk_ids,
    }
}

/// Walk a summary's subtree (within the eligible set) collecting leaf chunk
/// ids. A child id present in `by_id` is a lower-level summary → recurse; a
/// child id absent from `by_id` is a leaf chunk → record it.
fn collect_descendant_chunks(
    node: &SummaryNode,
    by_id: &HashMap<&str, &SummaryNode>,
    covered: &mut HashSet<String>,
) {
    for child in &node.child_ids {
        match by_id.get(child.as_str()) {
            Some(child_summary) => collect_descendant_chunks(child_summary, by_id, covered),
            None => {
                covered.insert(child.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openhuman::memory_store::trees::types::TreeKind;
    use chrono::Utc;

    fn summary(id: &str, parent: Option<&str>, level: u32, children: &[&str]) -> SummaryNode {
        SummaryNode {
            id: id.to_string(),
            tree_id: "t1".to_string(),
            tree_kind: TreeKind::Source,
            level,
            parent_id: parent.map(|p| p.to_string()),
            child_ids: children.iter().map(|c| c.to_string()).collect(),
            content: format!("summary {id}"),
            token_count: 10,
            entities: vec![],
            topics: vec![],
            time_range_start: Utc::now(),
            time_range_end: Utc::now(),
            score: 0.0,
            sealed_at: Utc::now(),
            deleted: false,
            embedding: None,
            doc_id: None,
            version_ms: None,
        }
    }

    #[test]
    fn single_eligible_summary_covers_its_leaves() {
        // L1 node over two leaf chunks, no parent → it's the frontier and
        // covers both leaves.
        let eligible = vec![summary("s1", None, 1, &["chunk-a", "chunk-b"])];
        let plan = plan_cover(&eligible);
        assert_eq!(plan.maximal_ids, vec!["s1"]);
        assert!(plan.covered_chunk_ids.contains("chunk-a"));
        assert!(plan.covered_chunk_ids.contains("chunk-b"));
        assert_eq!(plan.covered_chunk_ids.len(), 2);
    }

    #[test]
    fn parent_subsumes_child_only_parent_is_maximal() {
        // s2 (L2) → s1 (L1) → leaves. Both eligible: only s2 is maximal, and
        // it transitively covers the leaves under s1.
        let eligible = vec![
            summary("s2", None, 2, &["s1"]),
            summary("s1", Some("s2"), 1, &["chunk-a", "chunk-b"]),
        ];
        let plan = plan_cover(&eligible);
        assert_eq!(plan.maximal_ids, vec!["s2"]);
        assert!(plan.covered_chunk_ids.contains("chunk-a"));
        assert!(plan.covered_chunk_ids.contains("chunk-b"));
    }

    #[test]
    fn ineligible_parent_leaves_child_as_frontier() {
        // s1 is eligible but its parent s2 is NOT in the eligible set (it
        // straddles the window / not sealed) → s1 is maximal. Leaves under s1
        // are covered; nothing else.
        let eligible = vec![summary("s1", Some("s2-not-eligible"), 1, &["chunk-a"])];
        let plan = plan_cover(&eligible);
        assert_eq!(plan.maximal_ids, vec!["s1"]);
        assert!(plan.covered_chunk_ids.contains("chunk-a"));
    }

    #[test]
    fn empty_eligible_set_covers_nothing() {
        // No sealed/eligible summaries → frontier empty, no chunk covered, so
        // the caller emits every in-window chunk raw.
        let plan = plan_cover(&[]);
        assert!(plan.maximal_ids.is_empty());
        assert!(plan.covered_chunk_ids.is_empty());
    }

    #[test]
    fn sibling_frontier_nodes_each_emitted() {
        // Two L1 siblings, both eligible, parent NOT eligible → both maximal.
        let eligible = vec![
            summary("s1", Some("root-x"), 1, &["chunk-a"]),
            summary("s2", Some("root-x"), 1, &["chunk-b"]),
        ];
        let mut plan = plan_cover(&eligible);
        plan.maximal_ids.sort();
        assert_eq!(plan.maximal_ids, vec!["s1", "s2"]);
        assert_eq!(plan.covered_chunk_ids.len(), 2);
    }
}
