//! Identity-based, status-aware dedup for task-source cards.
//!
//! The per-source ledger (`store::ingested_tasks`, keyed on
//! `(source_id, external_id)`) only catches the *same* upstream item
//! re-fetched by the *same* source. It does **not** catch the same item
//! arriving through two overlapping sources, or through a source that was
//! deleted and recreated (new `source_id`) — each produces a duplicate
//! card on the `task-sources` board.
//!
//! This module supplies the cross-source half: a stable [`TaskIdentity`]
//! that is independent of `source_id`, a predicate to recognise a board
//! card that already represents the same upstream item, and the rule for
//! when such a card may be safely replaced.
//!
//! **Safety rule:** dedup may only ever remove/replace a card that is
//! still *untouched* — status [`TaskCardStatus::Todo`] **and** unclaimed
//! by a run. A card that has progressed (`Ready` / `InProgress` /
//! `AwaitingApproval` / `Blocked`) or resolved (`Done` / `Rejected`) is
//! either being worked or already decided; yanking it would destroy
//! in-flight work or resurface a rejected item. Those cases skip silently
//! instead.

use serde_json::Value;

use crate::openhuman::agent::task_board::{TaskBoardCard, TaskCardStatus};
use crate::openhuman::memory_sync::composio::providers::NormalizedTask;

/// Stable, source-independent identity for an upstream task. Two fetches
/// of the same upstream item — even via different sources or providers —
/// share an identity, so the board can carry exactly one card for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskIdentity {
    pub provider: String,
    pub external_id: String,
    pub url: Option<String>,
}

/// Derive the identity of a normalized task. Blank fields are trimmed and
/// a whitespace-only url is dropped to `None` so it can't spuriously match.
pub fn task_identity(task: &NormalizedTask) -> TaskIdentity {
    TaskIdentity {
        provider: task.provider.trim().to_string(),
        external_id: task.external_id.trim().to_string(),
        url: task
            .url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
    }
}

/// True when `card` already represents the same upstream item as
/// `identity`. Matches on `(provider, external_id)` — the stable
/// per-provider key — or, as a cross-provider fallback, an identical
/// non-empty `url`. Cards without `source_metadata` (agent/UI-authored)
/// never match.
pub fn card_matches_identity(card: &TaskBoardCard, identity: &TaskIdentity) -> bool {
    let Some(meta) = card.source_metadata.as_ref() else {
        return false;
    };
    provider_external_match(meta, identity) || url_match(meta, identity)
}

/// A card may be replaced/removed by dedup only while still untouched.
/// Run-claim state is checked separately by the caller (it needs board I/O).
pub fn is_replaceable_status(status: &TaskCardStatus) -> bool {
    matches!(status, TaskCardStatus::Todo)
}

fn meta_str<'a>(meta: &'a Value, key: &str) -> Option<&'a str> {
    meta.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn provider_external_match(meta: &Value, identity: &TaskIdentity) -> bool {
    if identity.external_id.is_empty() {
        return false;
    }
    meta_str(meta, "provider") == Some(identity.provider.as_str())
        && meta_str(meta, "external_id") == Some(identity.external_id.as_str())
}

fn url_match(meta: &Value, identity: &TaskIdentity) -> bool {
    match identity.url.as_deref() {
        Some(url) => meta_str(meta, "url") == Some(url),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn task(provider: &str, external_id: &str, url: Option<&str>) -> NormalizedTask {
        NormalizedTask {
            external_id: external_id.into(),
            provider: provider.into(),
            url: url.map(str::to_string),
            ..Default::default()
        }
    }

    fn card(status: TaskCardStatus, meta: Option<Value>) -> TaskBoardCard {
        TaskBoardCard {
            id: "task-x".into(),
            title: "t".into(),
            status,
            objective: None,
            plan: vec![],
            assigned_agent: None,
            allowed_tools: vec![],
            approval_mode: None,
            acceptance_criteria: vec![],
            evidence: vec![],
            notes: None,
            blocker: None,
            source_metadata: meta,
            order: 0,
            updated_at: String::new(),
        }
    }

    #[test]
    fn identity_trims_and_drops_blank_url() {
        let id = task_identity(&task(" github ", " 42 ", Some("   ")));
        assert_eq!(id.provider, "github");
        assert_eq!(id.external_id, "42");
        assert_eq!(id.url, None);
    }

    #[test]
    fn matches_on_provider_and_external_id() {
        let id = task_identity(&task("github", "42", None));
        let c = card(
            TaskCardStatus::Todo,
            Some(json!({ "provider": "github", "external_id": "42" })),
        );
        assert!(card_matches_identity(&c, &id));
    }

    #[test]
    fn different_external_id_does_not_match() {
        let id = task_identity(&task("github", "42", None));
        let c = card(
            TaskCardStatus::Todo,
            Some(json!({ "provider": "github", "external_id": "99" })),
        );
        assert!(!card_matches_identity(&c, &id));
    }

    #[test]
    fn matches_cross_provider_on_identical_url() {
        let id = task_identity(&task("linear", "LIN-7", Some("https://x/issues/42")));
        // Same upstream item surfaced under a different provider/external_id but
        // the same canonical url.
        let c = card(
            TaskCardStatus::Todo,
            Some(json!({
                "provider": "github",
                "external_id": "42",
                "url": "https://x/issues/42"
            })),
        );
        assert!(card_matches_identity(&c, &id));
    }

    #[test]
    fn empty_external_id_without_url_never_matches() {
        let id = task_identity(&task("github", "  ", None));
        let c = card(
            TaskCardStatus::Todo,
            Some(json!({ "provider": "github", "external_id": "" })),
        );
        assert!(!card_matches_identity(&c, &id));
    }

    #[test]
    fn card_without_metadata_never_matches() {
        let id = task_identity(&task("github", "42", None));
        let c = card(TaskCardStatus::Todo, None);
        assert!(!card_matches_identity(&c, &id));
    }

    #[test]
    fn only_todo_is_replaceable() {
        assert!(is_replaceable_status(&TaskCardStatus::Todo));
        for s in [
            TaskCardStatus::Ready,
            TaskCardStatus::InProgress,
            TaskCardStatus::AwaitingApproval,
            TaskCardStatus::Blocked,
            TaskCardStatus::Done,
            TaskCardStatus::Rejected,
        ] {
            assert!(!is_replaceable_status(&s), "{s:?} must not be replaceable");
        }
    }
}
