//! Orchestration front-end tools (stage 4).
//!
//! The two-pass front-end agent expresses its routing decision through two
//! early-exit tools (domain-owned per the repo tool-ownership rule):
//!
//! - [`ReplyToChannelTool`] (`reply_to_channel`) — pass 2: emit the finished
//!   `channel_response` that goes back over the tiny.place DM.
//! - [`DeferToOrchestratorTool`] (`defer_to_orchestrator`) — pass 1: hand
//!   macro-instructions down to the reasoning core.
//!
//! Both are pure "record the decision" tools: they echo their payload back as a
//! `ToolResult` and the harness [`EarlyExit`](crate::openhuman::tinyagents::EarlyExit)
//! hook captures the tool name + argument. They carry no external effect — the
//! actual DM send is the graph's `send_dm` node — so they stay `ReadOnly`.
//!
//! ## Reasoning-core session-history tools (Master chat)
//!
//! The reasoning core also gets read-only tools to browse the orchestration
//! store — the persisted OpenHuman↔agent session transcripts — so it can answer a
//! Master-chat question from its own history of chats with other agents instead of
//! only the single window it was woken for:
//!
//! - [`ListSessionsTool`] (`orchestration_list_sessions`) — enumerate the session
//!   windows (which peers/threads exist, with a one-line preview).
//! - [`ReadSessionTool`] (`orchestration_read_session`) — read one session's
//!   transcript by id.
//!
//! Both are `ReadOnly` and touch only the workspace-internal orchestration DB via
//! [`super::store`]; they carry no external effect (see [`super::ops`] for the
//! gated *send-on-behalf* path).

use std::future::Future;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::openhuman::config::Config;
use crate::openhuman::tools::{Tool, ToolResult};

use super::store;
use super::types::OrchestrationSession;

tokio::task_local! {
    /// Task-local capture of the front end's decision payload. The decision
    /// tools echo their argument as a `ToolResult`, but the split-brain graph
    /// needs the exact `text` / `instructions` the model passed — NOT the
    /// trailing narration the agent loop returns after the tool call (which is
    /// what `run_single` yields). Each decision tool records its payload here.
    static DECISION_CAPTURE: Arc<Mutex<Option<String>>>;
}

/// Scope a front-end decision capture around one front-end agent turn `fut`,
/// returning `(turn_output, captured_payload)`. `captured_payload` is the
/// argument the model passed to `reply_to_channel` / `defer_to_orchestrator`
/// (the authoritative channel response / macro-instructions), or `None` when the
/// turn ended without calling a decision tool (caller falls back to the raw text).
pub async fn with_decision_capture<F: Future>(fut: F) -> (F::Output, Option<String>) {
    let cell = Arc::new(Mutex::new(None));
    let out = DECISION_CAPTURE.scope(cell.clone(), Box::pin(fut)).await;
    let captured = cell.lock().ok().and_then(|mut slot| slot.take());
    (out, captured)
}

/// Record a front-end decision payload from a decision tool. Last write wins
/// (the turn's terminal decision). No-op outside a [`with_decision_capture`] scope.
fn record_decision(payload: &str) {
    let _ = DECISION_CAPTURE.try_with(|cell| {
        if let Ok(mut slot) = cell.lock() {
            *slot = Some(payload.to_string());
        }
    });
}

/// `reply_to_channel` — the front end's pass-2 terminal decision.
pub struct ReplyToChannelTool;

/// `defer_to_orchestrator` — the front end's pass-1 hand-off decision.
pub struct DeferToOrchestratorTool;

/// Extract a required string field, returning an error `ToolResult` when absent.
fn required_str(args: &Value, field: &str) -> Result<String, ToolResult> {
    match args.get(field).and_then(Value::as_str) {
        Some(s) if !s.trim().is_empty() => Ok(s.to_string()),
        _ => Err(ToolResult::error(format!("`{field}` is required"))),
    }
}

#[async_trait]
impl Tool for ReplyToChannelTool {
    fn name(&self) -> &str {
        "reply_to_channel"
    }

    fn description(&self) -> &str {
        "Send the finished reply back to the session over its tiny.place DM channel. \
         Call this once you have a complete answer for the counterpart."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "The finished reply to send back to the session."
                }
            },
            "required": ["text"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        match required_str(&args, "text") {
            Ok(text) => {
                record_decision(&text);
                Ok(ToolResult::success(text))
            }
            Err(e) => Ok(e),
        }
    }
}

#[async_trait]
impl Tool for DeferToOrchestratorTool {
    fn name(&self) -> &str {
        "defer_to_orchestrator"
    }

    fn description(&self) -> &str {
        "Hand this turn down to the reasoning core with macro-instructions. Call this \
         when the request needs real work (tools, sub-agents, multi-step reasoning) \
         rather than an immediate reply."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "instructions": {
                    "type": "string",
                    "description": "Concise macro-instructions describing what the reasoning core should do."
                }
            },
            "required": ["instructions"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        match required_str(&args, "instructions") {
            Ok(instructions) => {
                record_decision(&instructions);
                Ok(ToolResult::success(instructions))
            }
            Err(e) => Ok(e),
        }
    }
}

// ── Reasoning-core session-history read tools (Master chat) ──────────────────

/// Default / cap on how many messages a `orchestration_read_session` call returns.
const READ_SESSION_DEFAULT_LIMIT: u32 = 50;
const READ_SESSION_MAX_LIMIT: u32 = 200;
/// Cap on how many session rows `orchestration_list_sessions` returns.
const LIST_SESSIONS_MAX: usize = 100;
/// One-line preview length for the session list (char-safe, matches the roster).
const PREVIEW_MAX_CHARS: usize = 120;

/// The pinned sentinel windows — not agent↔agent transcripts, so they are hidden
/// from the history-browsing tools (the agent reads those via its normal channel).
fn is_pinned_window(session_id: &str) -> bool {
    matches!(session_id, "master" | "subconscious")
}

/// UTF-8-safe one-line preview (mirrors the roster `task_preview` in `schemas.rs`).
fn preview_line(body: &str) -> String {
    let trimmed = body.trim().replace('\n', " ");
    if trimmed.chars().count() <= PREVIEW_MAX_CHARS {
        return trimmed;
    }
    let mut out: String = trimmed.chars().take(PREVIEW_MAX_CHARS - 1).collect();
    out.push('…');
    out
}

/// `orchestration_list_sessions` — enumerate the persisted OpenHuman↔agent session
/// windows so the reasoning core can decide which history to read.
pub struct ListSessionsTool {
    config: Arc<Config>,
}

impl ListSessionsTool {
    pub fn new(config: Arc<Config>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for ListSessionsTool {
    fn name(&self) -> &str {
        "orchestration_list_sessions"
    }

    fn description(&self) -> &str {
        "List your saved chat sessions with other agents (the persisted OpenHuman↔agent \
         transcripts), newest activity first. Use this to find which past conversation to \
         read before answering a question. Returns each session's id, the peer agent, the \
         source harness, an optional label, the last activity time, the message count, and a \
         one-line preview. Read a session's full transcript with `orchestration_read_session`."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "limit": {
                    "type": "integer",
                    "description": "Max sessions to return (default all, capped at 100).",
                    "minimum": 1,
                    "maximum": LIST_SESSIONS_MAX,
                }
            },
            "additionalProperties": false
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .map(|n| (n as usize).min(LIST_SESSIONS_MAX))
            .unwrap_or(LIST_SESSIONS_MAX);

        let workspace = self.config.workspace_dir.clone();
        let result = store::with_connection(&workspace, |conn| {
            let sessions: Vec<OrchestrationSession> = store::list_sessions(conn)?;
            let mut out = Vec::with_capacity(sessions.len());
            for s in sessions {
                if is_pinned_window(&s.session_id) {
                    continue;
                }
                let count = store::count_messages(conn, &s.agent_id, &s.session_id)?;
                // Newest message body as a one-line preview. `list_recent_messages`
                // orders newest-first internally (DESC) then reverses, so with a
                // limit of 1 the single returned row is the newest message.
                let preview = store::list_recent_messages(conn, &s.agent_id, &s.session_id, 1)?
                    .last()
                    .map(|m| preview_line(&m.body));
                out.push(json!({
                    "sessionId": s.session_id,
                    "peerAgentId": s.agent_id,
                    "source": s.source,
                    "label": s.label,
                    "lastMessageAt": s.last_message_at,
                    "messageCount": count,
                    "preview": preview,
                }));
                if out.len() >= limit {
                    break;
                }
            }
            Ok(out)
        });

        match result {
            Ok(sessions) => {
                log::debug!(
                    target: "orchestration",
                    "[orchestration] tool.list_sessions returned={}",
                    sessions.len(),
                );
                let body = serde_json::to_string(&json!({ "sessions": sessions }))
                    .unwrap_or_else(|_| "{\"sessions\":[]}".to_string());
                Ok(ToolResult::success(body))
            }
            Err(e) => Ok(ToolResult::error(format!("list_sessions failed: {e}"))),
        }
    }

    fn is_concurrency_safe(&self, _args: &Value) -> bool {
        true
    }
}

/// `orchestration_read_session` — read one session's transcript by id.
pub struct ReadSessionTool {
    config: Arc<Config>,
}

impl ReadSessionTool {
    pub fn new(config: Arc<Config>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for ReadSessionTool {
    fn name(&self) -> &str {
        "orchestration_read_session"
    }

    fn description(&self) -> &str {
        "Read the transcript of one of your saved agent chat sessions (from \
         `orchestration_list_sessions`). Returns the messages in chronological order with role, \
         body, and timestamp. Use `before` to page backwards through a long history."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "sessionId": {
                    "type": "string",
                    "description": "The session id to read (from orchestration_list_sessions)."
                },
                "limit": {
                    "type": "integer",
                    "description": "Max messages to return (default 50, capped at 200).",
                    "minimum": 1,
                    "maximum": READ_SESSION_MAX_LIMIT,
                },
                "before": {
                    "type": "string",
                    "description": "Exclusive ISO-8601 timestamp to page backwards from."
                }
            },
            "required": ["sessionId"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let session_id = match required_str(&args, "sessionId") {
            Ok(s) => s,
            Err(e) => return Ok(e),
        };
        if is_pinned_window(&session_id) {
            return Ok(ToolResult::error(
                "`sessionId` must be an agent session, not a pinned window".to_string(),
            ));
        }
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .map(|n| (n as u32).min(READ_SESSION_MAX_LIMIT))
            .unwrap_or(READ_SESSION_DEFAULT_LIMIT);
        let before = args
            .get("before")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string);

        let workspace = self.config.workspace_dir.clone();
        let result = store::with_connection(&workspace, |conn| {
            store::list_messages_by_session(conn, &session_id, limit, before.as_deref())
        });

        match result {
            Ok(messages) => {
                log::debug!(
                    target: "orchestration",
                    "[orchestration] tool.read_session session={session_id} returned={}",
                    messages.len(),
                );
                let rendered: Vec<Value> = messages
                    .into_iter()
                    .map(|m| {
                        json!({
                            "role": m.role,
                            "body": m.body,
                            "timestamp": m.timestamp,
                        })
                    })
                    .collect();
                let body = serde_json::to_string(&json!({
                    "sessionId": session_id,
                    "messages": rendered,
                }))
                .unwrap_or_else(|_| "{\"messages\":[]}".to_string());
                Ok(ToolResult::success(body))
            }
            Err(e) => Ok(ToolResult::error(format!("read_session failed: {e}"))),
        }
    }

    fn is_concurrency_safe(&self, _args: &Value) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reply_tool_echoes_text_and_rejects_empty() {
        let t = ReplyToChannelTool;
        assert_eq!(t.name(), "reply_to_channel");
        let ok = t.execute(json!({"text": "all done"})).await.unwrap();
        assert!(ok.text().contains("all done"));
        let bad = t.execute(json!({"text": "  "})).await.unwrap();
        assert!(bad.is_error);
    }

    #[tokio::test]
    async fn defer_tool_echoes_instructions_and_rejects_missing() {
        let t = DeferToOrchestratorTool;
        assert_eq!(t.name(), "defer_to_orchestrator");
        let ok = t
            .execute(json!({"instructions": "research X then summarize"}))
            .await
            .unwrap();
        assert!(ok.text().contains("research X"));
        let bad = t.execute(json!({})).await.unwrap();
        assert!(bad.is_error);
    }

    #[tokio::test]
    async fn decision_capture_surfaces_tool_payload_not_turn_narration() {
        // The runtime must send the `reply_to_channel` argument (the real reply),
        // not the model's trailing "Done — sent to the session" narration that
        // `run_single` returns. Reproduces the reply-plumbing bug.
        let reply = ReplyToChannelTool;
        let (turn_text, captured) = with_decision_capture(async {
            let _ = reply
                .execute(json!({"text": "the actual email summary"}))
                .await
                .unwrap();
            "Done — the reply has been sent to the session".to_string()
        })
        .await;
        assert_eq!(turn_text, "Done — the reply has been sent to the session");
        assert_eq!(captured.as_deref(), Some("the actual email summary"));
    }

    #[tokio::test]
    async fn decision_capture_is_none_without_a_decision_tool() {
        let (turn_text, captured) =
            with_decision_capture(async { "just narration".to_string() }).await;
        assert_eq!(turn_text, "just narration");
        assert_eq!(captured, None);
    }

    // ── session-history read tools ──────────────────────────────────────────

    use super::super::types::{ChatKind, OrchestrationMessage};

    fn test_config(tmp: &tempfile::TempDir) -> Arc<Config> {
        Arc::new(Config {
            workspace_dir: tmp.path().to_path_buf(),
            ..Config::default()
        })
    }

    fn seed_msg(
        conn: &rusqlite::Connection,
        session: &str,
        seq: i64,
        role: &str,
        body: &str,
        ts: &str,
    ) {
        store::insert_message(
            conn,
            &OrchestrationMessage {
                id: format!("{session}-{seq}"),
                agent_id: "@peer".into(),
                session_id: session.into(),
                chat_kind: ChatKind::Session,
                role: role.into(),
                body: body.into(),
                timestamp: ts.into(),
                seq,
            },
        )
        .unwrap();
    }

    fn seed_session(conn: &rusqlite::Connection, session: &str, source: &str, last_at: &str) {
        store::upsert_session(
            conn,
            &OrchestrationSession {
                session_id: session.into(),
                agent_id: "@peer".into(),
                source: source.into(),
                label: None,
                workspace: None,
                last_seq: 0,
                created_at: last_at.into(),
                last_message_at: last_at.into(),
            },
        )
        .unwrap();
    }

    #[tokio::test]
    async fn list_sessions_tool_lists_agent_sessions_and_hides_pinned() {
        let tmp = tempfile::tempdir().unwrap();
        let config = test_config(&tmp);
        store::with_connection(&config.workspace_dir, |conn| {
            seed_session(conn, "s-1", "claude", "2026-07-02T00:01:00Z");
            seed_msg(conn, "s-1", 1, "user", "how do I ship it?", "2026-07-02T00:01:00Z");
            // A pinned window must be excluded from the history browser.
            seed_session(conn, "master", "master", "2026-07-02T00:02:00Z");
            seed_msg(conn, "master", 1, "user", "steer", "2026-07-02T00:02:00Z");
            Ok(())
        })
        .unwrap();

        let tool = ListSessionsTool::new(config);
        let out = tool.execute(json!({})).await.unwrap();
        assert!(!out.is_error);
        let v: Value = serde_json::from_str(&out.text()).unwrap();
        let sessions = v["sessions"].as_array().unwrap();
        assert_eq!(sessions.len(), 1, "only the agent session, not master");
        assert_eq!(sessions[0]["sessionId"], "s-1");
        assert_eq!(sessions[0]["peerAgentId"], "@peer");
        assert_eq!(sessions[0]["source"], "claude");
        assert_eq!(sessions[0]["messageCount"], 1);
        assert_eq!(sessions[0]["preview"], "how do I ship it?");
    }

    #[tokio::test]
    async fn read_session_tool_returns_transcript_chronologically() {
        let tmp = tempfile::tempdir().unwrap();
        let config = test_config(&tmp);
        store::with_connection(&config.workspace_dir, |conn| {
            seed_session(conn, "s-1", "codex", "2026-07-02T00:03:00Z");
            seed_msg(conn, "s-1", 1, "user", "first", "2026-07-02T00:01:00Z");
            seed_msg(conn, "s-1", 2, "agent", "second", "2026-07-02T00:02:00Z");
            Ok(())
        })
        .unwrap();

        let tool = ReadSessionTool::new(config);
        let out = tool.execute(json!({ "sessionId": "s-1" })).await.unwrap();
        assert!(!out.is_error);
        let v: Value = serde_json::from_str(&out.text()).unwrap();
        let msgs = v["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[0]["body"], "first");
        assert_eq!(msgs[1]["body"], "second"); // chronological order
    }

    #[tokio::test]
    async fn read_session_tool_rejects_missing_id_and_pinned_window() {
        let tmp = tempfile::tempdir().unwrap();
        let config = test_config(&tmp);
        let tool = ReadSessionTool::new(config);
        assert!(tool.execute(json!({})).await.unwrap().is_error);
        assert!(tool
            .execute(json!({ "sessionId": "master" }))
            .await
            .unwrap()
            .is_error);
    }
}
