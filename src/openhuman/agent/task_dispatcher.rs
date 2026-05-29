//! Deterministic task-card dispatcher.
//!
//! Turns a [`TaskBoardCard`] into work: it **claims** the card (flips it to
//! `in_progress`, which `todos::ops::enforce_single_in_progress` makes a
//! per-board mutual-exclusion lock), runs a single **autonomous agent turn**
//! toward the card's objective, and **writes the outcome back** to the board
//! (`done` + evidence on success, `blocked` + reason on failure).
//!
//! This is the one executor both dispatch paths converge on:
//! - the **board poller** (cards that arrived without a proactive trigger), and
//! - the **proactive triage** arm (`agent::triage::apply_decision`), once it has
//!   decided to act on a task-board card.
//!
//! The runner mirrors `skills::spawn_skill_run_background`: build the
//! `orchestrator` agent fresh inside a detached task, cap tool iterations, and
//! run `agent.run_single` under `with_autonomous_iter_cap`. PR-4 generalises the
//! executor from the default agent to a resolved personality/skill; this module
//! keeps the default-agent path so the pipeline runs end-to-end first.

use std::sync::OnceLock;
use std::time::Duration;

use crate::openhuman::agent::harness::session::Agent;
use crate::openhuman::agent::harness::subagent_runner::with_autonomous_iter_cap;
use crate::openhuman::agent::task_board::{TaskBoardCard, TaskCardStatus};
use crate::openhuman::config::Config;
use crate::openhuman::todos::ops::{self, BoardLocation, CardPatch};

/// Tool-iteration ceiling for an autonomous task run. Matches the skill-run
/// cap — a task brief is the same shape of bounded autonomous work.
const TASK_RUN_MAX_ITERATIONS: usize = 200;

/// Max chars of the agent's final output retained as board `evidence`.
const EVIDENCE_MAX_CHARS: usize = 2_000;

/// Render a card into the goal prompt handed to the autonomous run.
///
/// The card's `content`/title is the display form; the prompt leads with the
/// clean `objective`, then any `plan` steps and `acceptance_criteria`, and a
/// pointer to the originating source so the agent can pull related context from
/// memory via its `memory_recall` tool (the GitHub/Notion/… activity for this
/// item is ingested into the summary tree by the memory-sources domain).
pub fn build_task_prompt(card: &TaskBoardCard) -> String {
    let mut lines: Vec<String> = Vec::new();

    let objective = card
        .objective
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| card.title.trim());
    lines.push(format!(
        "You are autonomously executing one task to completion. Objective:\n{objective}"
    ));

    if !card.plan.is_empty() {
        lines.push("\nPlan:".to_string());
        for (i, step) in card.plan.iter().enumerate() {
            lines.push(format!("{}. {}", i + 1, step.trim()));
        }
    }

    if !card.acceptance_criteria.is_empty() {
        lines.push("\nAcceptance criteria (the task is done only when all hold):".to_string());
        for c in &card.acceptance_criteria {
            lines.push(format!("- {}", c.trim()));
        }
    }

    if let Some(meta) = &card.source_metadata {
        let provider = meta.get("provider").and_then(|v| v.as_str());
        let repo = meta.get("repo").and_then(|v| v.as_str());
        let external_id = meta.get("external_id").and_then(|v| v.as_str());
        let url = meta.get("url").and_then(|v| v.as_str());
        let mut origin = String::new();
        if let Some(p) = provider {
            origin.push_str(p);
        }
        if let Some(r) = repo {
            origin.push_str(&format!(" {r}"));
        }
        if let Some(id) = external_id {
            origin.push_str(&format!("#{id}"));
        }
        if !origin.trim().is_empty() {
            lines.push(format!(
                "\nThis task originates from {}. Its activity has been ingested into memory — use \
                 your memory_recall tool to pull related context (prior discussion, linked items) \
                 before and while you work.",
                origin.trim()
            ));
        }
        if let Some(u) = url {
            lines.push(format!("Source link: {u}"));
        }
    }

    lines.push(
        "\nWork the task to completion. Do not pick up unrelated work. When finished, your final \
         message should summarise what you did and the evidence (commits, PRs, results)."
            .to_string(),
    );

    lines.join("\n")
}

/// Dispatch one card: claim it, run an autonomous turn, write the result back.
///
/// Returns the dispatch `run_id` once the card is claimed and the detached run
/// has been spawned. Returns `Err` *without* spawning if the claim fails — most
/// commonly because another card on the board is already `in_progress`
/// (`enforce_single_in_progress`), which is the intended per-board throttle; the
/// caller (poller) simply tries again on its next tick.
pub async fn dispatch_card(location: BoardLocation, card: TaskBoardCard) -> Result<String, String> {
    let card_id = card.id.clone();
    let prompt = build_task_prompt(&card);

    // Claim the card. The Todo→InProgress transition doubles as the lock:
    // `enforce_single_in_progress` rejects a second concurrent in-progress
    // card, so a failed claim means "something else is running" → skip.
    ops::update_status(&location, &card_id, TaskCardStatus::InProgress)
        .map_err(|e| format!("[task_dispatcher] claim failed for card {card_id}: {e}"))?;

    let run_id = uuid::Uuid::new_v4().to_string();
    tracing::info!(
        card_id = %card_id,
        run_id = %run_id,
        prompt_chars = prompt.chars().count(),
        "[task_dispatcher] card claimed (todo→in_progress), spawning autonomous run"
    );

    let run_id_for_return = run_id.clone();
    let location_for_run = location.clone();
    tokio::spawn(async move {
        let outcome = run_autonomous(&prompt, &run_id).await;
        write_back(&location_for_run, &card_id, &run_id, outcome);
    });

    Ok(run_id_for_return)
}

/// Build the orchestrator agent fresh and run a single autonomous turn.
async fn run_autonomous(prompt: &str, run_id: &str) -> Result<String, String> {
    let mut config = Config::load_or_init()
        .await
        .map_err(|e| format!("load config: {e:#}"))?;
    config.agent.max_tool_iterations = TASK_RUN_MAX_ITERATIONS;
    // Match skill-run egress handling: only widen to the permissive default
    // when the operator hasn't configured an explicit allow-list.
    if config.http_request.allowed_domains.is_empty() {
        config.http_request.allowed_domains = vec!["*".to_string()];
    }

    let mut agent = Agent::from_config_for_agent(&config, "orchestrator")
        .map_err(|e| format!("build agent: {e:#}"))?;
    agent.set_event_context(run_id.to_string(), "task");
    agent.set_agent_definition_name(format!(
        "orchestrator-task-{}",
        run_id.get(..8).unwrap_or(run_id)
    ));

    with_autonomous_iter_cap(TASK_RUN_MAX_ITERATIONS, agent.run_single(prompt))
        .await
        .map_err(|e| format!("{e:#}"))
}

/// Deterministic board write-back: the dispatcher owns the card lifecycle.
/// Success → `done` + evidence; failure → `blocked` + blocker reason. An
/// external write failure here is logged, never propagated — the run already
/// happened.
fn write_back(
    location: &BoardLocation,
    card_id: &str,
    run_id: &str,
    outcome: Result<String, String>,
) {
    let patch = match &outcome {
        Ok(output) => {
            tracing::info!(
                card_id = %card_id,
                run_id = %run_id,
                output_chars = output.chars().count(),
                "[task_dispatcher] run complete → done"
            );
            CardPatch {
                status: Some(TaskCardStatus::Done),
                evidence: Some(vec![truncate_chars(output.trim(), EVIDENCE_MAX_CHARS)]),
                ..Default::default()
            }
        }
        Err(err) => {
            tracing::warn!(
                card_id = %card_id,
                run_id = %run_id,
                error = %err,
                "[task_dispatcher] run failed → blocked"
            );
            CardPatch {
                status: Some(TaskCardStatus::Blocked),
                blocker: Some(truncate_chars(err, EVIDENCE_MAX_CHARS)),
                ..Default::default()
            }
        }
    };

    if let Err(e) = ops::edit(location, card_id, patch) {
        tracing::error!(
            card_id = %card_id,
            run_id = %run_id,
            error = %e,
            "[task_dispatcher] board write-back failed (run outcome lost from board)"
        );
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

// ── Board poller ──────────────────────────────────────────────────────────

/// How often the poller wakes to look for a dispatchable card.
const POLLER_TICK_SECONDS: u64 = 60;

static POLLER_STARTED: OnceLock<()> = OnceLock::new();

/// Spawn the board poller. Idempotent — only the first call installs the loop.
///
/// Each tick it scans the `task-sources` board and dispatches the
/// highest-urgency `todo` card via [`dispatch_card`], gated by background-AI
/// capacity (`scheduler_gate`). This is the catch-all for cards that arrive
/// without a proactive trigger (`TodoOnly` sources, manual cards, or proactive
/// turns the gate skipped). Cards that *did* get a proactive trigger are
/// dispatched by the triage arm; the claim-based lock makes firing both safe.
pub fn start_board_poller() {
    if POLLER_STARTED.set(()).is_err() {
        tracing::debug!("[task_dispatcher:poller] already running, skipping start");
        return;
    }
    tokio::spawn(async move {
        tracing::info!(
            tick_seconds = POLLER_TICK_SECONDS,
            "[task_dispatcher:poller] starting"
        );
        let mut ticker = tokio::time::interval(Duration::from_secs(POLLER_TICK_SECONDS));
        ticker.tick().await; // skip the immediate fire so startup isn't slammed
        loop {
            ticker.tick().await;
            if let Err(e) = poll_once().await {
                tracing::warn!(error = %e, "[task_dispatcher:poller] tick failed (continuing)");
            }
        }
    });
}

/// One poller tick: dispatch the highest-urgency `todo` card on the
/// task-sources board, if any and if capacity allows. `pub(crate)` so tests can
/// drive a tick without the real interval.
pub(crate) async fn poll_once() -> Result<(), String> {
    // Gate on background-AI capacity (autonomy / power / pause). Dropping the
    // permit immediately is fine: this is a "may background work start now"
    // check; the run itself is detached.
    let Some(_permit) = crate::openhuman::scheduler_gate::wait_for_capacity().await else {
        tracing::debug!("[task_dispatcher:poller] scheduler gate denied capacity; idle tick");
        return Ok(());
    };

    let config = Config::load_or_init()
        .await
        .map_err(|e| format!("load config: {e:#}"))?;
    if !config.task_sources.enabled {
        return Ok(());
    }

    let location = BoardLocation::Thread {
        workspace_dir: config.workspace_dir.clone(),
        thread_id: crate::openhuman::task_sources::TASK_SOURCES_THREAD_ID.to_string(),
    };
    let snapshot = ops::list(&location)?;

    // `enforce_single_in_progress` caps the board at one running card, so if
    // one is already in progress there's nothing for this tick to claim.
    if snapshot
        .cards
        .iter()
        .any(|c| c.status == TaskCardStatus::InProgress)
    {
        return Ok(());
    }

    let Some(card) = pick_next_todo(&snapshot.cards) else {
        return Ok(());
    };

    tracing::info!(
        card_id = %card.id,
        urgency = card_urgency(&card),
        "[task_dispatcher:poller] dispatching highest-urgency todo card"
    );
    dispatch_card(location, card).await.map(|_| ())
}

/// Highest-urgency `todo` card (urgency from `source_metadata.urgency`,
/// default 0.0; ties broken toward the lower board `order`). Returns a clone.
fn pick_next_todo(cards: &[TaskBoardCard]) -> Option<TaskBoardCard> {
    cards
        .iter()
        .filter(|c| c.status == TaskCardStatus::Todo)
        .max_by(|a, b| {
            card_urgency(a)
                .partial_cmp(&card_urgency(b))
                .unwrap_or(std::cmp::Ordering::Equal)
                // On equal urgency, prefer the lower `order` (earlier card):
                // reversing the order comparison makes it the "greater" pick.
                .then(b.order.cmp(&a.order))
        })
        .cloned()
}

fn card_urgency(card: &TaskBoardCard) -> f64 {
    card.source_metadata
        .as_ref()
        .and_then(|m| m.get("urgency"))
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn card(objective: Option<&str>) -> TaskBoardCard {
        TaskBoardCard {
            id: "task-1".into(),
            title: "[GitHub] Fix login bug".into(),
            status: TaskCardStatus::Todo,
            objective: objective.map(str::to_string),
            plan: vec![],
            assigned_agent: None,
            allowed_tools: vec![],
            approval_mode: None,
            acceptance_criteria: vec![],
            evidence: vec![],
            notes: None,
            blocker: None,
            source_metadata: None,
            order: 0,
            updated_at: String::new(),
        }
    }

    #[test]
    fn prompt_uses_objective_then_falls_back_to_title() {
        let p = build_task_prompt(&card(Some("Fix the login bug")));
        assert!(p.contains("Fix the login bug"));
        assert!(!p.contains("[GitHub]"));

        let p2 = build_task_prompt(&card(None));
        assert!(p2.contains("[GitHub] Fix login bug"));
    }

    #[test]
    fn prompt_includes_plan_and_acceptance_criteria() {
        let mut c = card(Some("Do it"));
        c.plan = vec!["step one".into(), "step two".into()];
        c.acceptance_criteria = vec!["tests pass".into()];
        let p = build_task_prompt(&c);
        assert!(p.contains("Plan:"));
        assert!(p.contains("1. step one"));
        assert!(p.contains("2. step two"));
        assert!(p.contains("Acceptance criteria"));
        assert!(p.contains("- tests pass"));
    }

    #[test]
    fn prompt_points_at_source_and_memory_when_metadata_present() {
        let mut c = card(Some("Resolve issue"));
        c.source_metadata = Some(json!({
            "provider": "github",
            "repo": "octo/repo",
            "external_id": "123",
            "url": "https://github.com/octo/repo/issues/123",
        }));
        let p = build_task_prompt(&c);
        assert!(p.contains("github octo/repo#123"));
        assert!(p.contains("memory_recall"));
        assert!(p.contains("https://github.com/octo/repo/issues/123"));
    }

    #[test]
    fn prompt_omits_source_block_without_metadata() {
        let p = build_task_prompt(&card(Some("Do it")));
        assert!(!p.contains("memory_recall"));
    }

    #[test]
    fn truncate_caps_long_strings() {
        let s = "x".repeat(5_000);
        let out = truncate_chars(&s, EVIDENCE_MAX_CHARS);
        assert!(out.chars().count() <= EVIDENCE_MAX_CHARS);
        assert!(out.ends_with('…'));
    }

    fn card_with(
        id: &str,
        status: TaskCardStatus,
        urgency: Option<f64>,
        order: u32,
    ) -> TaskBoardCard {
        let mut c = card(Some("obj"));
        c.id = id.into();
        c.status = status;
        c.order = order;
        c.source_metadata = urgency.map(|u| json!({ "urgency": u }));
        c
    }

    #[test]
    fn poller_picks_highest_urgency_todo_skipping_other_statuses() {
        let cards = vec![
            card_with("a", TaskCardStatus::Todo, Some(0.3), 0),
            card_with("b", TaskCardStatus::Done, Some(0.99), 1),
            card_with("c", TaskCardStatus::Todo, Some(0.8), 2),
            card_with("d", TaskCardStatus::Todo, None, 3),
        ];
        let picked = pick_next_todo(&cards).expect("a todo card is available");
        assert_eq!(
            picked.id, "c",
            "highest-urgency todo wins, done card ignored"
        );
    }

    #[test]
    fn poller_breaks_urgency_ties_toward_lower_order() {
        let cards = vec![
            card_with("late", TaskCardStatus::Todo, Some(0.5), 5),
            card_with("early", TaskCardStatus::Todo, Some(0.5), 2),
        ];
        assert_eq!(pick_next_todo(&cards).unwrap().id, "early");
    }

    #[test]
    fn poller_returns_none_when_no_todo_cards() {
        let cards = vec![card_with("a", TaskCardStatus::Done, Some(0.9), 0)];
        assert!(pick_next_todo(&cards).is_none());
    }
}
