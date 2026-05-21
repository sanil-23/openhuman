//! Two-lane explicit user preferences — namespaces + read helpers.
//!
//! Preferences written by the `save_preference` tool live in one of two
//! namespaces depending on their relevance scope:
//!
//! - [`USER_PREF_GENERAL_NAMESPACE`] — always-on; injected into the system
//!   prompt at thread start (Lane A).
//! - [`USER_PREF_SITUATIONAL_NAMESPACE`] — topic-scoped; recalled per-turn by
//!   semantic similarity to the user's message (Lane B).
//!
//! Keeping the namespace constants and read helpers here (rather than in the
//! tool module) lets the write path, the system-prompt builder, and the
//! per-turn recall path all share one definition.

use std::sync::Arc;

use super::Memory;

/// Always-on preferences — injected into the system prompt every thread.
pub const USER_PREF_GENERAL_NAMESPACE: &str = "user_pref_general";

/// Topic-scoped preferences — recalled per query against the user's message.
pub const USER_PREF_SITUATIONAL_NAMESPACE: &str = "user_pref_situational";

/// Default cap on general preferences injected into the system prompt. Keeps
/// the always-on block bounded so it can't blow a small model's context window
/// (see the legacy `gpt-4` 8K overflow).
pub const STANDING_PREFS_LIMIT: usize = 10;

/// Load the latest-`limit` general preferences as plain-language strings,
/// newest-first (by `updated_at`). This is the Lane-A system-prompt block.
///
/// `list()` returns entries ordered newest-first but with `content` set to the
/// title (= topic key), so the body value is fetched via `get()`.
pub async fn load_general_preferences(memory: &Arc<dyn Memory>, limit: usize) -> Vec<String> {
    let entries = memory
        .list(Some(USER_PREF_GENERAL_NAMESPACE), None, None)
        .await
        .unwrap_or_default();

    let mut out = Vec::new();
    for entry in entries.into_iter().take(limit) {
        if let Ok(Some(full)) = memory.get(USER_PREF_GENERAL_NAMESPACE, &entry.key).await {
            let value = full.content.trim();
            if !value.is_empty() {
                out.push(value.to_string());
            }
        }
    }
    out
}
