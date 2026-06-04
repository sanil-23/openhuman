//! Route an [`EnrichedTask`] onto the agent's work surface.
//!
//! Every enriched task lands as a card on the dedicated `task-sources`
//! thread board (reusing the thread-scoped `todos` store). Sources with
//! the [`SourceTarget::AgentTodoProactive`] target additionally dispatch
//! a triage turn — the same `TriggerEnvelope` → `run_triage` →
//! `apply_decision` path Composio webhooks use — so an agent can start
//! working immediately. Triage's classifier (drop / acknowledge / react
//! / escalate) gates noise, and the proactive turn is held behind the
//! `scheduler_gate` capacity semaphore so background AI throttling is
//! respected.

use serde_json::json;

use crate::openhuman::agent::task_board::TaskBoardCard;
use crate::openhuman::agent::triage::{apply_decision, run_triage, TriageOutcome, TriggerEnvelope};
use crate::openhuman::config::Config;
use crate::openhuman::todos::ops::{
    add as todo_add, remove as todo_remove, BoardLocation, CardPatch,
};
use crate::openhuman::todos::runs;
use crate::openhuman::{scheduler_gate, todos};

use super::types::{EnrichedTask, FilterSpec, SourceTarget, TaskSource};
use super::{dedup, TaskKind};

/// Stable thread id whose board collects every ingested task.
pub const TASK_SOURCES_THREAD_ID: &str = "task-sources";

/// Outcome of routing one enriched task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteOutcome {
    /// A fresh card was created with this id (and, for proactive sources, a
    /// triage turn was dispatched). The pipeline records it in the ledger.
    Routed(String),
    /// An existing card already represents this upstream item, so nothing was
    /// created and no triage ran. Carries that card's id so the pipeline can
    /// point this source's ledger row at it and stop re-evaluating the item
    /// every poll.
    Deduped { existing_card_id: String },
}

/// The `task-sources` board for this workspace.
fn board_location(config: &Config) -> BoardLocation {
    BoardLocation::Thread {
        workspace_dir: config.workspace_dir.clone(),
        thread_id: TASK_SOURCES_THREAD_ID.to_string(),
    }
}

/// Route an enriched task with per-source, status-aware dedup, then (for
/// proactive sources whose card was freshly created) dispatch a triage turn.
///
/// Dedup is scoped to the *originating* source: a source never extracts a card
/// for an item it already has on the board. (Two different sources that both
/// match the same upstream item each keep their own card — dedup deliberately
/// does not collapse across sources.)
///
/// Dedup never removes a card that has progressed past `Todo` or is claimed by
/// a run: such a card is in-flight or already resolved, so the new fetch skips
/// silently ([`RouteOutcome::Deduped`]) and the existing card is left intact.
pub async fn route_enriched(
    config: &Config,
    source: &TaskSource,
    enriched: &EnrichedTask,
) -> Result<RouteOutcome, String> {
    match decide_and_add(config, source, enriched)? {
        Decision::Deduped(existing_card_id) => Ok(RouteOutcome::Deduped { existing_card_id }),
        Decision::Created(card_id) => match source.target {
            SourceTarget::TodoOnly => {
                tracing::debug!(
                    source_id = %source.id,
                    external_id = %enriched.task.external_id,
                    "[task_sources:route] todo-only target, card added (no agent turn)"
                );
                Ok(RouteOutcome::Routed(card_id))
            }
            SourceTarget::AgentTodoProactive => {
                dispatch_triage(config, source, enriched, &card_id).await?;
                Ok(RouteOutcome::Routed(card_id))
            }
        },
    }
}

/// Internal result of the dedup decision: either an existing card already
/// covers the item, or a new card was created.
enum Decision {
    Created(String),
    Deduped(String),
}

/// True when `card` may be safely removed/replaced by dedup: it is still an
/// untouched `Todo` **and** no run is actively claiming it. Progressed,
/// resolved, or claimed cards must never be yanked.
fn is_replaceable(location: &BoardLocation, card: &TaskBoardCard) -> bool {
    dedup::is_replaceable_status(&card.status) && !has_active_run(location, &card.id)
}

/// Whether a non-completed run is currently claiming `card_id`. Errors are
/// treated as "active" (fail-safe: never remove a card we can't prove is idle).
fn has_active_run(location: &BoardLocation, card_id: &str) -> bool {
    match runs::list_runs(location, Some(card_id)) {
        Ok(records) => records.iter().any(|r| r.is_active()),
        Err(e) => {
            tracing::warn!(
                card_id,
                error = %e,
                "[task_sources:route] run lookup failed; treating card as active (won't remove)"
            );
            true
        }
    }
}

/// Decide whether to create a card for `enriched` or dedup against one this
/// **same source** already has on the board, then perform the chosen action.
///
/// Only cards owned by `source` (matched via `source_metadata.source_id`) are
/// considered — a different source's card for the same item is left alone.
///
/// Rules:
/// 1. **In-flight / resolved** — this source already has a card for the item
///    that has progressed past `Todo` (or is claimed by a run) → skip silently,
///    sweeping any redundant untouched `Todo` duplicates this source leaked.
/// 2. **Untouched refresh** — this source's only cards for the item are still
///    `Todo` → remove them and add one fresh card (heals accumulated dupes and
///    picks up edited content).
/// 3. **New item** — this source has no card for the item → create one.
fn decide_and_add(
    config: &Config,
    source: &TaskSource,
    enriched: &EnrichedTask,
) -> Result<Decision, String> {
    let location = board_location(config);
    let identity = dedup::task_identity(&enriched.task);
    let cards = todos::ops::list(&location)
        .map_err(|e| format!("[task_sources:route] failed to list board: {e}"))?
        .cards;

    // This source's existing cards for this item: the untouched `Todo` ones we
    // may replace, and the first in-flight/resolved one that blocks creation.
    let mut replaceable_todo_ids: Vec<String> = Vec::new();
    let mut blocking_id: Option<String> = None;
    for card in &cards {
        if !card_owned_by(card, &source.id) || !dedup::card_matches_identity(card, &identity) {
            continue;
        }
        if is_replaceable(&location, card) {
            replaceable_todo_ids.push(card.id.clone());
        } else if blocking_id.is_none() {
            blocking_id = Some(card.id.clone());
        }
    }

    // (1) An in-flight/resolved card from this source already covers the item.
    // Skip, but sweep any redundant untouched-Todo duplicates left behind.
    if let Some(existing_id) = blocking_id {
        for id in &replaceable_todo_ids {
            let _ = todo_remove(&location, id);
        }
        tracing::info!(
            source_id = %source.id,
            external_id = %identity.external_id,
            existing_card_id = %existing_id,
            swept_dupes = replaceable_todo_ids.len(),
            "[task_sources:route] source already has an in-flight/resolved card; skipping"
        );
        return Ok(Decision::Deduped(existing_id));
    }

    // (2) Only untouched `Todo` cards (or none) → replace them with one fresh
    // card (refreshes edited content and collapses any leaked duplicates).
    for id in &replaceable_todo_ids {
        let _ = todo_remove(&location, id);
    }
    if !replaceable_todo_ids.is_empty() {
        tracing::debug!(
            source_id = %source.id,
            external_id = %identity.external_id,
            replaced = replaceable_todo_ids.len(),
            "[task_sources:route] replacing this source's untouched Todo card(s) with fresh ingest"
        );
    }

    // (3) Create the fresh card.
    let card_id = create_card(config, source, enriched, &location)?;
    Ok(Decision::Created(card_id))
}

/// True when `card` was ingested by `source_id` (per its `source_metadata`).
fn card_owned_by(card: &TaskBoardCard, source_id: &str) -> bool {
    card.source_metadata
        .as_ref()
        .and_then(|m| m.get("source_id"))
        .and_then(|v| v.as_str())
        == Some(source_id)
}

/// Remove every still-untouched (`Todo`, unclaimed) card belonging to
/// `source_id` from the board. Cards that have progressed or are claimed by a
/// run are preserved so deleting a source never destroys in-flight work.
/// Returns `(removed, preserved)` counts.
pub fn remove_source_cards(config: &Config, source_id: &str) -> Result<(usize, usize), String> {
    let location = board_location(config);
    let cards = todos::ops::list(&location)
        .map_err(|e| format!("[task_sources:route] failed to list board: {e}"))?
        .cards;

    let mut removed = 0usize;
    let mut preserved = 0usize;
    for card in &cards {
        if !card_owned_by(card, source_id) {
            continue;
        }
        if is_replaceable(&location, card) {
            if todo_remove(&location, &card.id).is_ok() {
                removed += 1;
            }
        } else {
            preserved += 1;
        }
    }

    tracing::info!(
        source_id,
        removed,
        preserved,
        "[task_sources:route] cleaned board cards for deleted source"
    );
    Ok((removed, preserved))
}

/// Build and append a fresh card for `enriched` on the board. Returns the new
/// card id. (Dedup/stale-card handling happens in [`decide_and_add`].)
fn create_card(
    config: &Config,
    source: &TaskSource,
    enriched: &EnrichedTask,
    location: &BoardLocation,
) -> Result<String, String> {
    let task = &enriched.task;
    let label = provider_label(&task.provider);
    let content = format!("[{label}] {}", task.title.trim());

    let mut notes_parts: Vec<String> = Vec::new();
    if enriched.summary.trim() != task.title.trim() && !enriched.summary.trim().is_empty() {
        notes_parts.push(enriched.summary.trim().to_string());
    }
    if let Some(url) = task.url.as_deref().filter(|s| !s.trim().is_empty()) {
        notes_parts.push(url.trim().to_string());
    }
    let notes = if notes_parts.is_empty() {
        None
    } else {
        Some(notes_parts.join("\n"))
    };

    // Objective: the intent-framed goal from enrichment ("Review pull
    // request: …" / "Resolve issue: …" / bare title for generic tasks). The
    // card `content`/title is the `[provider] title` display form; the
    // objective is the clean goal the executing agent — and the triage LLM —
    // works toward, so it must state *what kind of job* this is.
    let objective = enriched.objective.clone();

    // Stamp the source identifiers the downstream dispatcher / write-back
    // needs (provider + repo + issue id + url) plus the enrichment urgency
    // used for prioritisation. This is the only writer of `source_metadata`.
    let source_metadata = build_source_metadata(source, enriched);

    // G7: pre-assign the card to the source's configured executor so the
    // dispatcher runs it deterministically (no LLM router). Unset → unassigned.
    let assigned_agent = source
        .assigned_executor
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let snapshot = todo_add(
        location,
        &content,
        CardPatch {
            notes,
            objective,
            assigned_agent,
            source_metadata: Some(source_metadata),
            ..Default::default()
        },
    )
    .map_err(|e| format!("[task_sources:route] failed to add todo card: {e}"))?;

    // The newly created card is always the last one in the snapshot (add
    // appends at the end). Return its id for the dedup ledger.
    let new_card_id = snapshot
        .cards
        .last()
        .map(|c| c.id.clone())
        .ok_or_else(|| "[task_sources:route] add returned empty card list".to_string())?;

    tracing::debug!(
        external_id = %task.external_id,
        card_id = %new_card_id,
        cards = snapshot.cards.len(),
        "[task_sources:route] card added to task-sources board"
    );
    Ok(new_card_id)
}

/// Build the card's `source_metadata` from the originating source + task:
/// the provider/repo/issue identifiers a later dispatcher or external
/// write-back needs to address the upstream item, plus the enrichment
/// urgency used to prioritise pickup. Repo is only present for GitHub
/// sources (the other providers don't carry a repo concept).
fn build_source_metadata(source: &TaskSource, enriched: &EnrichedTask) -> serde_json::Value {
    let task = &enriched.task;
    let mut meta = json!({
        "provider": task.provider,
        "source_id": source.id,
        "external_id": task.external_id,
        "urgency": enriched.urgency,
    });
    // Only stamp `kind` when the provider differentiated it (issue vs PR), so
    // the FE card and triage can tell "review this" from "solve this".
    if task.kind != TaskKind::Generic {
        meta["kind"] = json!(task.kind.as_str());
    }
    if let Some(url) = task.url.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        meta["url"] = json!(url);
    }
    if let FilterSpec::Github {
        repo: Some(repo), ..
    } = &source.filter
    {
        let repo = repo.trim();
        if !repo.is_empty() {
            meta["repo"] = json!(repo);
        }
    }
    meta
}

/// Dispatch a triage turn for a proactive task, gated by scheduler
/// capacity. Card creation already happened; a gated-off or deferred
/// turn is non-fatal — the task still sits on the board.
async fn dispatch_triage(
    config: &Config,
    source: &TaskSource,
    enriched: &EnrichedTask,
    card_id: &str,
) -> Result<(), String> {
    // Respect background-AI throttling. When the gate denies capacity
    // (Off / paused), we keep the card but skip the proactive turn.
    let Some(_permit) = scheduler_gate::wait_for_capacity().await else {
        tracing::info!(
            source_id = %source.id,
            "[task_sources:route] scheduler gate denied capacity; card added, agent turn skipped"
        );
        return Ok(());
    };

    let task = &enriched.task;
    let payload = json!({
        "task": task,
        "summary": enriched.summary,
        "agentPrompt": enriched.agent_prompt,
        "urgency": enriched.urgency,
        "url": task.url,
        "provider": task.provider,
        "sourceId": source.id,
    });

    // Link the envelope to the board card so triage's escalation arm routes
    // it through the deterministic dispatcher (claim → autonomous run →
    // write-back) instead of the one-shot triage sub-agent.
    let location = BoardLocation::Thread {
        workspace_dir: config.workspace_dir.clone(),
        thread_id: TASK_SOURCES_THREAD_ID.to_string(),
    };
    let envelope = TriggerEnvelope::from_external(
        &format!("task_sources:{}", source.id),
        "external task ingested",
        payload,
    )
    .with_task_card(card_id.to_string(), location);

    let outcome = run_triage(&envelope)
        .await
        .map_err(|e| format!("[task_sources:route] triage evaluation failed: {e}"))?;

    match outcome {
        TriageOutcome::Decision(run) => {
            apply_decision(run, &envelope)
                .await
                .map_err(|e| format!("[task_sources:route] apply_decision failed: {e}"))?;
            tracing::debug!(
                source_id = %source.id,
                external_id = %task.external_id,
                "[task_sources:route] triage decision applied"
            );
        }
        TriageOutcome::Deferred { reason, .. } => {
            tracing::debug!(
                source_id = %source.id,
                reason = %reason,
                "[task_sources:route] triage deferred (card remains on board)"
            );
        }
    }
    Ok(())
}

/// Title-case a provider slug for display on the card.
fn provider_label(provider: &str) -> String {
    match provider {
        "github" => "GitHub".to_string(),
        "notion" => "Notion".to_string(),
        "linear" => "Linear".to_string(),
        "clickup" => "ClickUp".to_string(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        }
    }
}

/// Read the current cards on the `task-sources` board. Used by tests and
/// callers that want to inspect routed work without an RPC round-trip.
pub fn board_cards(
    config: &Config,
) -> Result<Vec<crate::openhuman::agent::task_board::TaskBoardCard>, String> {
    let location = BoardLocation::Thread {
        workspace_dir: config.workspace_dir.clone(),
        thread_id: TASK_SOURCES_THREAD_ID.to_string(),
    };
    todos::ops::list(&location).map(|snap| snap.cards)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openhuman::agent::task_board::TaskCardStatus;
    use crate::openhuman::task_sources::types::ProviderSlug;
    use crate::openhuman::task_sources::NormalizedTask;
    use chrono::Utc;

    #[test]
    fn provider_label_titlecases_known_and_unknown() {
        assert_eq!(provider_label("github"), "GitHub");
        assert_eq!(provider_label("clickup"), "ClickUp");
        assert_eq!(provider_label("asana"), "Asana");
        assert_eq!(provider_label(""), "");
    }

    fn github_source(repo: Option<&str>) -> TaskSource {
        TaskSource {
            id: "ts-1".into(),
            provider: ProviderSlug::Github,
            connection_id: None,
            name: None,
            enabled: true,
            filter: FilterSpec::Github {
                repo: repo.map(str::to_string),
                labels: vec![],
                assignee_is_me: true,
                state: None,
                fetch_mode: Default::default(),
                extra: json!({}),
            },
            interval_secs: 1800,
            target: SourceTarget::AgentTodoProactive,
            max_tasks_per_fetch: 25,
            assigned_executor: None,
            created_at: Utc::now(),
            last_fetch_at: None,
            last_status: None,
        }
    }

    fn enriched(external_id: &str, url: Option<&str>, urgency: f32) -> EnrichedTask {
        let task = NormalizedTask {
            external_id: external_id.into(),
            provider: "github".into(),
            title: "Fix the bug".into(),
            url: url.map(str::to_string),
            ..Default::default()
        };
        // Objective is derived in enrichment — mirror that here so the helper
        // stays truthful (generic kind → bare title).
        let objective = crate::openhuman::task_sources::enrich::derive_objective(&task);
        EnrichedTask {
            task,
            summary: "Fix the bug".into(),
            urgency,
            linked_people: vec![],
            linked_memory_ids: vec![],
            agent_prompt: "do it".into(),
            objective,
            enriched_at: Utc::now(),
        }
    }

    #[test]
    fn source_metadata_carries_github_repo_and_identifiers() {
        let src = github_source(Some("octo/repo"));
        let e = enriched("123", Some("https://github.com/octo/repo/issues/123"), 0.7);
        let meta = build_source_metadata(&src, &e);
        assert_eq!(meta["provider"], json!("github"));
        assert_eq!(meta["source_id"], json!("ts-1"));
        assert_eq!(meta["external_id"], json!("123"));
        assert_eq!(meta["repo"], json!("octo/repo"));
        assert_eq!(
            meta["url"],
            json!("https://github.com/octo/repo/issues/123")
        );
        let urgency = meta["urgency"].as_f64().expect("urgency is a number");
        assert!((urgency - 0.7).abs() < 1e-6, "urgency was {urgency}");
    }

    #[test]
    fn source_metadata_omits_absent_repo_and_url() {
        let src = github_source(None);
        let e = enriched("9", None, 0.4);
        let meta = build_source_metadata(&src, &e);
        assert!(meta.get("repo").is_none());
        assert!(meta.get("url").is_none());
        assert_eq!(meta["external_id"], json!("9"));
        let urgency = meta["urgency"].as_f64().expect("urgency is a number");
        assert!((urgency - 0.4).abs() < 1e-6, "urgency was {urgency}");
    }

    fn temp_config() -> (tempfile::TempDir, Config) {
        let tmp = tempfile::tempdir().unwrap();
        let config = Config {
            workspace_dir: tmp.path().join("workspace"),
            action_dir: tmp.path().join("workspace"),
            config_path: tmp.path().join("config.toml"),
            ..Config::default()
        };
        std::fs::create_dir_all(&config.workspace_dir).unwrap();
        (tmp, config)
    }

    #[test]
    fn add_card_stamps_objective_assigned_agent_and_metadata() {
        let (_tmp, config) = temp_config();
        let mut src = github_source(Some("octo/repo"));
        // Whitespace around the executor must be trimmed into assigned_agent.
        src.assigned_executor = Some("  agent-x  ".into());
        let e = enriched("123", Some("https://github.com/octo/repo/issues/123"), 0.7);

        create_card(&config, &src, &e, &board_location(&config)).expect("create_card succeeds");

        let cards = board_cards(&config).expect("board_cards");
        assert_eq!(cards.len(), 1);
        let card = &cards[0];
        // Display title is the `[provider] title` form; objective is the bare title.
        assert_eq!(card.title, "[GitHub] Fix the bug");
        assert_eq!(card.objective.as_deref(), Some("Fix the bug"));
        assert_eq!(card.assigned_agent.as_deref(), Some("agent-x"));
        let meta = card
            .source_metadata
            .as_ref()
            .expect("source_metadata present");
        assert_eq!(meta["external_id"], json!("123"));
        assert_eq!(meta["repo"], json!("octo/repo"));
        // Generic kind is not stamped onto metadata.
        assert!(meta.get("kind").is_none());
    }

    #[test]
    fn pull_request_card_carries_review_objective_and_kind_metadata() {
        let (_tmp, config) = temp_config();
        let src = github_source(Some("octo/repo"));
        let mut task = NormalizedTask {
            external_id: "55".into(),
            provider: "github".into(),
            title: "Add retry".into(),
            ..Default::default()
        };
        task.kind = TaskKind::PullRequest;
        let objective = crate::openhuman::task_sources::enrich::derive_objective(&task);
        let e = EnrichedTask {
            task,
            summary: "Add retry".into(),
            urgency: 0.5,
            linked_people: vec![],
            linked_memory_ids: vec![],
            agent_prompt: "review it".into(),
            objective,
            enriched_at: Utc::now(),
        };

        create_card(&config, &src, &e, &board_location(&config)).expect("create_card succeeds");

        let cards = board_cards(&config).expect("board_cards");
        let card = &cards[0];
        // The objective tells the picking agent (and triage) the job is a review.
        assert_eq!(
            card.objective.as_deref(),
            Some("Review pull request: Add retry")
        );
        let meta = card
            .source_metadata
            .as_ref()
            .expect("source_metadata present");
        assert_eq!(meta["kind"], json!("pull_request"));
    }

    #[test]
    fn add_card_drops_whitespace_only_assigned_executor() {
        let (_tmp, config) = temp_config();
        let mut src = github_source(None);
        src.assigned_executor = Some("   ".into());
        let e = enriched("9", None, 0.4);

        create_card(&config, &src, &e, &board_location(&config)).expect("create_card succeeds");

        let cards = board_cards(&config).expect("board_cards");
        assert_eq!(cards.len(), 1);
        assert!(
            cards[0].assigned_agent.is_none(),
            "whitespace-only executor should not assign the card"
        );
    }

    #[test]
    fn source_metadata_has_no_repo_for_non_github_provider() {
        let mut src = github_source(Some("octo/repo"));
        // A non-GitHub filter carries no repo concept.
        src.provider = ProviderSlug::Linear;
        src.filter = FilterSpec::Linear {
            team_id: None,
            assignee_is_me: true,
            state: None,
            extra: json!({}),
        };
        let e = enriched("LIN-5", None, 0.5);
        let meta = build_source_metadata(&src, &e);
        assert!(meta.get("repo").is_none());
        assert_eq!(meta["source_id"], json!("ts-1"));
    }

    // ── Dedup behavior ──────────────────────────────────────────────────

    /// A `TodoOnly` source with the given id, so dedup tests never reach the
    /// triage dispatch path.
    fn source_with_id(id: &str) -> TaskSource {
        let mut src = github_source(Some("octo/repo"));
        src.id = id.into();
        src.target = SourceTarget::TodoOnly;
        src
    }

    fn set_status(config: &Config, card_id: &str, status: TaskCardStatus) {
        let location = board_location(config);
        let mut cards = todos::ops::list(&location).expect("list").cards;
        for c in &mut cards {
            if c.id == card_id {
                c.status = status.clone();
            }
        }
        todos::ops::replace(&location, cards).expect("replace");
    }

    #[test]
    fn same_source_unchanged_reingest_skips() {
        // The same source re-extracting the same item → no second card.
        let (_tmp, config) = temp_config();
        let src = source_with_id("source-a");
        let e = enriched("42", Some("https://github.com/octo/repo/issues/42"), 0.5);

        let card1 = match decide_and_add(&config, &src, &e).expect("first add") {
            Decision::Created(id) => id,
            Decision::Deduped(_) => panic!("first ingest should create"),
        };
        // Move it off Todo so the source is "already working" the item.
        set_status(&config, &card1, TaskCardStatus::AwaitingApproval);

        let second = decide_and_add(&config, &src, &e).expect("second add");
        match second {
            Decision::Deduped(existing) => assert_eq!(existing, card1),
            Decision::Created(_) => panic!("source must not re-extract an item it already has"),
        }
        assert_eq!(board_cards(&config).unwrap().len(), 1, "exactly one card");
    }

    #[test]
    fn different_sources_keep_independent_cards() {
        // Dedup is per-source: two sources both matching the same upstream item
        // each keep their own card.
        let (_tmp, config) = temp_config();
        let src_a = source_with_id("source-a");
        let src_b = source_with_id("source-b");
        let e = enriched("7", Some("https://github.com/octo/repo/issues/7"), 0.4);

        assert!(matches!(
            decide_and_add(&config, &src_a, &e).expect("a"),
            Decision::Created(_)
        ));
        assert!(matches!(
            decide_and_add(&config, &src_b, &e).expect("b"),
            Decision::Created(_),
        ));
        assert_eq!(
            board_cards(&config).unwrap().len(),
            2,
            "each source keeps its own card"
        );
    }

    #[test]
    fn same_source_replaces_untouched_todo_card() {
        let (_tmp, config) = temp_config();
        let src = source_with_id("source-a");
        let e = enriched("9", Some("https://github.com/octo/repo/issues/9"), 0.4);

        let card1 = match decide_and_add(&config, &src, &e).expect("add") {
            Decision::Created(id) => id,
            Decision::Deduped(_) => panic!("first add creates"),
        };

        // Re-ingest while the card is still an untouched Todo → replace it.
        let card2 = match decide_and_add(&config, &src, &e).expect("re-ingest") {
            Decision::Created(id) => id,
            Decision::Deduped(_) => panic!("untouched todo should be replaced"),
        };
        assert_ne!(card1, card2, "a fresh card replaced the old one");
        assert_eq!(board_cards(&config).unwrap().len(), 1, "no accumulation");
    }

    #[test]
    fn same_source_sweeps_leaked_todo_duplicates() {
        // Heals the observed bug: a source that leaked several Todo cards for
        // one item collapses back to a single card on the next ingest.
        let (_tmp, config) = temp_config();
        let src = source_with_id("source-a");
        let location = board_location(&config);
        let e = enriched("8", Some("https://github.com/octo/repo/issues/8"), 0.4);

        // Simulate three leaked Todo cards for the same item from this source.
        for _ in 0..3 {
            create_card(&config, &src, &e, &location).expect("seed");
        }
        assert_eq!(board_cards(&config).unwrap().len(), 3);

        let outcome = decide_and_add(&config, &src, &e).expect("re-ingest");
        assert!(matches!(outcome, Decision::Created(_)));
        assert_eq!(
            board_cards(&config).unwrap().len(),
            1,
            "leaked Todo duplicates collapse to one"
        );
    }

    #[test]
    fn inflight_card_is_not_replaced_on_reingest() {
        let (_tmp, config) = temp_config();
        let src = source_with_id("source-a");
        let e = enriched("11", Some("https://github.com/octo/repo/issues/11"), 0.4);

        let card1 = match decide_and_add(&config, &src, &e).expect("add") {
            Decision::Created(id) => id,
            Decision::Deduped(_) => panic!("first add creates"),
        };
        set_status(&config, &card1, TaskCardStatus::InProgress);

        // Re-ingest must NOT yank the in-flight card.
        let outcome = decide_and_add(&config, &src, &e).expect("re-ingest");
        match outcome {
            Decision::Deduped(existing) => assert_eq!(existing, card1),
            Decision::Created(_) => panic!("must not replace an in-flight card"),
        }
        let cards = board_cards(&config).unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(
            cards[0].status,
            TaskCardStatus::InProgress,
            "left untouched"
        );
    }

    #[test]
    fn rejected_card_does_not_resurface() {
        let (_tmp, config) = temp_config();
        let src = source_with_id("source-a");
        let e = enriched("12", Some("https://github.com/octo/repo/issues/12"), 0.4);

        let card1 = match decide_and_add(&config, &src, &e).expect("add") {
            Decision::Created(id) => id,
            Decision::Deduped(_) => unreachable!(),
        };
        set_status(&config, &card1, TaskCardStatus::Rejected);

        let outcome = decide_and_add(&config, &src, &e).expect("re-ingest");
        assert!(
            matches!(outcome, Decision::Deduped(_)),
            "rejected stays suppressed"
        );
        let cards = board_cards(&config).unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].status, TaskCardStatus::Rejected);
    }

    #[test]
    fn active_run_blocks_replacement() {
        let (_tmp, config) = temp_config();
        let src = source_with_id("source-a");
        let e = enriched("15", Some("https://github.com/octo/repo/issues/15"), 0.4);

        let card1 = match decide_and_add(&config, &src, &e).expect("add") {
            Decision::Created(id) => id,
            Decision::Deduped(_) => unreachable!(),
        };
        // Card is still Todo, but a run claims it → must not be removed.
        let location = board_location(&config);
        runs::create_run(&location, "run-1", &card1, "default").expect("create_run");

        let outcome = decide_and_add(&config, &src, &e).expect("re-ingest");
        match outcome {
            Decision::Deduped(existing) => assert_eq!(existing, card1),
            Decision::Created(_) => panic!("claimed card must not be replaced"),
        }
        assert_eq!(board_cards(&config).unwrap().len(), 1);
    }

    #[test]
    fn remove_source_cards_drops_todo_preserves_progressed() {
        let (_tmp, config) = temp_config();
        let src = source_with_id("source-a");
        let todo = enriched("20", Some("https://github.com/octo/repo/issues/20"), 0.4);
        let busy = enriched("21", Some("https://github.com/octo/repo/issues/21"), 0.4);

        decide_and_add(&config, &src, &todo).expect("add todo");
        let busy_id = match decide_and_add(&config, &src, &busy).expect("add busy") {
            Decision::Created(id) => id,
            Decision::Deduped(_) => unreachable!(),
        };
        set_status(&config, &busy_id, TaskCardStatus::InProgress);

        let (removed, preserved) = remove_source_cards(&config, "source-a").expect("cleanup");
        assert_eq!(removed, 1, "the untouched Todo card is removed");
        assert_eq!(preserved, 1, "the in-flight card is preserved");
        let cards = board_cards(&config).unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].id, busy_id);
    }

    #[test]
    fn remove_source_cards_ignores_other_sources() {
        let (_tmp, config) = temp_config();
        let a = source_with_id("source-a");
        let b = source_with_id("source-b");
        decide_and_add(&config, &a, &enriched("30", None, 0.4)).expect("a");
        decide_and_add(&config, &b, &enriched("31", None, 0.4)).expect("b");

        let (removed, _) = remove_source_cards(&config, "source-a").expect("cleanup");
        assert_eq!(removed, 1);
        let cards = board_cards(&config).unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(
            cards[0].source_metadata.as_ref().unwrap()["source_id"],
            json!("source-b")
        );
    }
}
