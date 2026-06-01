//! Mocked-LLM e2e tests for the workflow RUN plumbing.
//!
//! These exercise the genuinely-new execution path with a scripted LLM and no
//! network: a workflow is RUN (`spawn_skill_run_background` builds an
//! autonomous agent and `run_single`s it), reaches a terminal `DONE` footer,
//! and `await_run_outcome` returns it; and the orchestrator composes a
//! workflow via the `run_workflow` tool (spawn → inner run → await → result).
//!
//! ## Why `#[ignore]` + serial
//!
//! The inner workflow run is a detached `tokio::spawn` that rebuilds its LLM
//! provider from config and resolves the workspace from the **process-global**
//! `OPENHUMAN_WORKSPACE` env. So these tests install a process-global mock
//! provider (`factory::test_provider_override`) and set `OPENHUMAN_WORKSPACE`
//! — global state that would race other tests under the default parallel
//! runner. They are therefore `#[ignore]`d (kept out of the parallel default
//! run) and meant to be run serially:
//!
//! ```text
//! cargo test --lib workflows::e2e_run_tests -- --ignored --test-threads=1
//! ```
//!
//! A module-level async mutex also serializes them against each other if run
//! with `--ignored` but without `--test-threads=1`.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::openhuman::agent::harness::run_tool_call_loop;
use crate::openhuman::agent::tools::RunWorkflowTool;
use crate::openhuman::config::MultimodalConfig;
use crate::openhuman::inference::provider::factory::test_provider_override;
use crate::openhuman::inference::provider::traits::{
    ChatMessage, ChatRequest, ChatResponse, ProviderCapabilities,
};
use crate::openhuman::inference::provider::{Provider, ToolCall};
use crate::openhuman::tools::policy::DefaultToolPolicy;
use crate::openhuman::tools::traits::Tool;
use crate::openhuman::workflows::schemas::{await_run_outcome, spawn_skill_run_background};

/// Serialize this module's tests (each touches process-global state).
fn serial() -> &'static tokio::sync::Mutex<()> {
    static L: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    L.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// RAII override of the global `OPENHUMAN_WORKSPACE` env (restored on drop).
struct WorkspaceEnv {
    prev: Option<String>,
}
impl WorkspaceEnv {
    fn set(path: &std::path::Path) -> Self {
        let prev = std::env::var("OPENHUMAN_WORKSPACE").ok();
        std::env::set_var("OPENHUMAN_WORKSPACE", path);
        Self { prev }
    }
}
impl Drop for WorkspaceEnv {
    fn drop(&mut self) {
        match &self.prev {
            Some(v) => std::env::set_var("OPENHUMAN_WORKSPACE", v),
            None => std::env::remove_var("OPENHUMAN_WORKSPACE"),
        }
    }
}

/// One scripted LLM that serves BOTH the orchestrator and the inner workflow
/// run, routing by what's in the conversation:
///   - inner run prompt ("running a single workflow") → finish → DONE footer;
///   - orchestrator after run_workflow returned → final wrap-up;
///   - orchestrator first turn → call `run_workflow`.
struct MockLlm {
    workflow_id: String,
}

fn final_text(t: &str) -> ChatResponse {
    ChatResponse {
        text: Some(t.into()),
        tool_calls: vec![],
        usage: None,
        reasoning_content: None,
    }
}
fn tool_call_resp(id: &str, name: &str, args: serde_json::Value) -> ChatResponse {
    ChatResponse {
        text: Some(String::new()),
        tool_calls: vec![ToolCall {
            id: id.into(),
            name: name.into(),
            arguments: args.to_string(),
        }],
        usage: None,
        reasoning_content: None,
    }
}

#[async_trait]
impl Provider for MockLlm {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            native_tool_calling: true,
            ..ProviderCapabilities::default()
        }
    }
    async fn chat_with_system(
        &self,
        _system_prompt: Option<&str>,
        _message: &str,
        _model: &str,
        _temperature: f64,
    ) -> anyhow::Result<String> {
        Ok("ok".into())
    }
    async fn chat(
        &self,
        request: ChatRequest<'_>,
        _model: &str,
        _temperature: f64,
    ) -> anyhow::Result<ChatResponse> {
        let convo: String = request
            .messages
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        // Inner workflow run: finish immediately. The returned text becomes the
        // run's DONE footer body.
        if convo.contains("running a single workflow") || convo.contains("Workflow guidelines") {
            return Ok(final_text("WORKFLOW_DONE: inbox triaged"));
        }
        // Orchestrator, AFTER run_workflow's result came back: wrap up.
        if convo.contains("WORKFLOW_DONE") || convo.contains("\"status\"") {
            return Ok(final_text("ORCHESTRATOR_DONE"));
        }
        // Orchestrator, first turn: run the workflow.
        Ok(tool_call_resp(
            "c1",
            "run_workflow",
            serde_json::json!({ "workflow_id": self.workflow_id, "wait_seconds": 20 }),
        ))
    }
}

/// Seed a RUNNABLE workflow where the run path (`load_skills`/`get_skill`)
/// looks: `<ws>/skills/<id>/{skill.toml, SKILL.md}`. No required inputs, so
/// `run_workflow` with an empty input map spawns cleanly.
fn seed_runnable_workflow(ws: &std::path::Path, id: &str) {
    let dir = ws.join("skills").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("skill.toml"),
        format!("id = \"{id}\"\nwhen_to_use = \"triage email\"\n"),
    )
    .unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {id}\ndescription: Triage the inbox.\n---\n\nSummarise and label the inbox.\n"),
    )
    .unwrap();
}

// ── Test 1: a workflow RUN executes via the mock LLM and reaches DONE ─────

#[ignore = "process-global provider override + OPENHUMAN_WORKSPACE; run: \
            cargo test --lib workflows::e2e_run_tests -- --ignored --test-threads=1"]
#[tokio::test]
async fn inner_workflow_run_executes_via_mock_llm_and_reaches_done() {
    let _serial = serial().lock().await;
    let ws_root = tempfile::tempdir().unwrap();
    let _env = WorkspaceEnv::set(ws_root.path());
    // Seed exactly where the run path resolves the workspace to (the env maps
    // OPENHUMAN_WORKSPACE → <root>/workspace), so get_skill/load_skills finds it.
    let workspace = crate::openhuman::workflows::schemas::resolve_workspace_dir().await;
    seed_runnable_workflow(&workspace, "triage-inbox");
    let _guard = test_provider_override::install(Arc::new(MockLlm {
        workflow_id: "triage-inbox".into(),
    }));

    let started = spawn_skill_run_background("triage-inbox".to_string(), None)
        .await
        .expect("spawn should succeed — the workflow is runnable");
    let outcome = await_run_outcome(&started.log_path, Duration::from_secs(20))
        .await
        .unwrap_or_else(|| {
            panic!(
                "inner run never reached a terminal footer; log:\n{}",
                std::fs::read_to_string(&started.log_path).unwrap_or_default()
            )
        });
    assert_eq!(
        outcome.status,
        "DONE",
        "log:\n{}",
        std::fs::read_to_string(&started.log_path).unwrap_or_default()
    );
    assert!(
        outcome.output.contains("WORKFLOW_DONE"),
        "the inner agent's final text must land in the DONE footer; got: {}",
        outcome.output
    );
}

// ── Test 2: orchestrator composes a workflow via the run_workflow tool ────

#[ignore = "process-global provider override + OPENHUMAN_WORKSPACE; run: \
            cargo test --lib workflows::e2e_run_tests -- --ignored --test-threads=1"]
#[tokio::test]
async fn orchestrator_runs_workflow_tool_and_gets_inner_result() {
    let _serial = serial().lock().await;
    let ws_root = tempfile::tempdir().unwrap();
    let _env = WorkspaceEnv::set(ws_root.path());
    let workspace = crate::openhuman::workflows::schemas::resolve_workspace_dir().await;
    seed_runnable_workflow(&workspace, "triage-inbox");
    // The inner run (spawned by the run_workflow tool) builds its provider from
    // config → needs the global override. The outer loop gets the mock directly.
    let _guard = test_provider_override::install(Arc::new(MockLlm {
        workflow_id: "triage-inbox".into(),
    }));

    let provider = MockLlm {
        workflow_id: "triage-inbox".into(),
    };
    let tools: Vec<Box<dyn Tool>> = vec![Box::new(RunWorkflowTool::new())];
    let mut history = vec![ChatMessage::user("Triage my inbox.")];

    let result = run_tool_call_loop(
        &provider,
        &mut history,
        &tools,
        "test-provider",
        "model",
        0.0,
        true,
        "channel",
        &MultimodalConfig::default(),
        5,
        None,
        None,
        &[],
        None,
        None,
        &DefaultToolPolicy,
    )
    .await
    .expect("orchestrator loop should complete");

    assert_eq!(result, "ORCHESTRATOR_DONE");
    // The run_workflow tool result (carrying the inner run's DONE outcome) must
    // have flowed back into the conversation.
    let tool_msgs: String = history
        .iter()
        .filter(|m| m.role == "tool")
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        tool_msgs.contains("DONE") && tool_msgs.contains("WORKFLOW_DONE"),
        "run_workflow must return the inner run's terminal outcome; got:\n{tool_msgs}"
    );
}
