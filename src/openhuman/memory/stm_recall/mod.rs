//! Phase 3 — Bounded cross-thread STM recall.
//!
//! Assembles a bounded, recency-weighted context block from two arms:
//!
//! - **Arm 1** — FTS5 over not-yet-compacted recent episodic entries from
//!   OTHER sessions. Reuses [`crate::openhuman::memory::store::fts5::episodic_cross_session_search`].
//!   When no user query is available (preemptive/session-start case), falls back to
//!   a recency selection of recent non-current-session episodic turns.
//!
//! - **Arm 2** — Brute-force cosine nearest-neighbour over `segment_embeddings`
//!   (per-model table from Phase 0+1). Single-user scale: no ANN index, no new
//!   deps. Loads candidate vectors, computes cosine, top-k. Filters by
//!   `model_signature` and excludes the current `session_id`.
//!
//! **Merge → dedup → bound:**
//! - Dedup: if a segment recap and its raw episodic rows (within
//!   `start_episodic_id..=end_episodic_id`) both appear, prefer the recap and
//!   drop the overlapping episodic hits.
//! - Recency-weight: scored by `updated_at` timestamp proximity.
//! - Hard-cap: tunable token budget ([`TOKEN_BUDGET`]) and top-k bounds
//!   ([`MAX_SEGMENT_RECAPS`] + [`MAX_EPISODIC_TURNS`]).
//!
//! ## Tunable consts (all in this file, all documented)
//! - [`RECENCY_WINDOW_DAYS`] — how many days back to search (STM/LTM boundary)
//! - [`RECENCY_WINDOW_MAX_SEGMENTS`] — max segments to load for vector search
//! - [`COSINE_GATE`] — minimum similarity for Arm 2 (medium gate)
//! - [`MAX_SEGMENT_RECAPS`] — top-k segment recaps to include
//! - [`MAX_EPISODIC_TURNS`] — max raw episodic turns to include
//! - [`TOKEN_BUDGET`] — hard token budget (chars / 4 approx)
//! - [`FTS5_LIMIT`] — how many FTS5 candidates to fetch before gating
//!
//! ## Scope boundary
//! Does NOT traverse `tree::*` (`SummaryNode`, `memory_tree_*`). The memory
//! tree is LTM; this module is strictly STM (recent episodic + segment layer).

pub mod recall;
pub mod tool;

pub use recall::{stm_recall, StmRecallBlock, StmRecallOpts};

// ─────────────────────────────────────────────────────────────────────────────
// Tunable constants — the STM/LTM knobs
// ─────────────────────────────────────────────────────────────────────────────

/// STM recency window in days. Segments or episodic entries older than this
/// are excluded — they belong in LTM (the memory tree).
pub const RECENCY_WINDOW_DAYS: f64 = 14.0;

/// Hard cap on segments loaded for vector search (Arm 2).
/// Keeps the brute-force cosine pass bounded at single-user scale.
pub const RECENCY_WINDOW_MAX_SEGMENTS: usize = 100;

/// Cosine similarity gate for Arm 2 (segment recaps).
/// Below this threshold a recap is excluded regardless of recency.
/// Range: [0.0, 1.0]; 0.65 is "medium gate" — confident topical overlap.
pub const COSINE_GATE: f32 = 0.65;

/// Maximum segment recaps to include in the output block.
pub const MAX_SEGMENT_RECAPS: usize = 5;

/// Maximum raw episodic turns to include in the output block.
pub const MAX_EPISODIC_TURNS: usize = 5;

/// Approximate token budget for the entire STM block (chars / 4 ≈ tokens).
/// ~1500 tokens × 4 chars/token = 6000 chars.
pub const TOKEN_BUDGET: usize = 6_000;

/// How many FTS5 candidates to fetch before applying the high-precision gate.
/// The gate is: only strong keyword matches survive — FTS5 rank threshold is
/// applied at the DB level via LIMIT; we over-fetch slightly and let dedup
/// finish trimming.
pub const FTS5_LIMIT: usize = 20;
