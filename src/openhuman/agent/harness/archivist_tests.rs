use super::*;
use crate::openhuman::agent::hooks::{ToolCallRecord, TurnContext};
use crate::openhuman::memory::store::{events as ev, fts5, segments as seg};
use crate::openhuman::memory::tree::chat::ChatPrompt;

fn setup_conn() -> Arc<Mutex<Connection>> {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(fts5::EPISODIC_INIT_SQL).unwrap();
    conn.execute_batch(seg::SEGMENTS_INIT_SQL).unwrap();
    conn.execute_batch(ev::EVENTS_INIT_SQL).unwrap();
    conn.execute_batch(profile::PROFILE_INIT_SQL).unwrap();
    Arc::new(Mutex::new(conn))
}

#[tokio::test]
async fn archivist_indexes_turn() {
    let conn = setup_conn();
    let hook = ArchivistHook::new(conn.clone(), true);

    let ctx = TurnContext {
        user_message: "What is Rust?".into(),
        assistant_response: "Rust is a systems programming language.".into(),
        tool_calls: vec![],
        turn_duration_ms: 500,
        session_id: Some("test-session".into()),
        iteration_count: 1,
    };

    hook.on_turn_complete(&ctx).await.unwrap();

    let entries = fts5::episodic_session_entries(&conn, "test-session").unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].role, "user");
    assert_eq!(entries[1].role, "assistant");
}

#[tokio::test]
async fn archivist_creates_segment_on_first_turn() {
    let conn = setup_conn();
    let hook = ArchivistHook::new(conn.clone(), true);

    let ctx = TurnContext {
        user_message: "Hello world".into(),
        assistant_response: "Hi there!".into(),
        tool_calls: vec![],
        turn_duration_ms: 100,
        session_id: Some("seg-test".into()),
        iteration_count: 1,
    };

    hook.on_turn_complete(&ctx).await.unwrap();

    let open = seg::open_segment_for_session(&conn, "seg-test").unwrap();
    assert!(open.is_some());
    assert_eq!(open.unwrap().turn_count, 1);
}

#[tokio::test]
async fn archivist_detects_topic_change_boundary() {
    let conn = setup_conn();
    let hook = ArchivistHook::new(conn.clone(), true);

    hook.on_turn_complete(&TurnContext {
        user_message: "Tell me about Rust".into(),
        assistant_response: "Rust is great.".into(),
        tool_calls: vec![],
        turn_duration_ms: 100,
        session_id: Some("boundary-test".into()),
        iteration_count: 1,
    })
    .await
    .unwrap();

    hook.on_turn_complete(&TurnContext {
        user_message: "How about its memory safety?".into(),
        assistant_response: "It uses ownership.".into(),
        tool_calls: vec![],
        turn_duration_ms: 100,
        session_id: Some("boundary-test".into()),
        iteration_count: 2,
    })
    .await
    .unwrap();

    hook.on_turn_complete(&TurnContext {
        user_message: "Switching to a different topic now. I prefer dark mode.".into(),
        assistant_response: "Noted about dark mode.".into(),
        tool_calls: vec![],
        turn_duration_ms: 100,
        session_id: Some("boundary-test".into()),
        iteration_count: 3,
    })
    .await
    .unwrap();

    let segments = seg::segments_by_namespace(&conn, "global", 10).unwrap();
    assert!(
        segments.len() >= 2,
        "Expected at least 2 segments, got {}",
        segments.len()
    );
}

#[tokio::test]
async fn archivist_extracts_failure_lesson() {
    let conn = setup_conn();
    let hook = ArchivistHook::new(conn.clone(), true);

    let ctx = TurnContext {
        user_message: "Run tests".into(),
        assistant_response: "Tests failed.".into(),
        tool_calls: vec![ToolCallRecord {
            name: "shell".into(),
            arguments: serde_json::json!({"command": "cargo test"}),
            success: false,
            output_summary: "shell: failed (error)".into(),
            duration_ms: 3000,
        }],
        turn_duration_ms: 3500,
        session_id: Some("test-session-2".into()),
        iteration_count: 2,
    };

    hook.on_turn_complete(&ctx).await.unwrap();

    let entries = fts5::episodic_session_entries(&conn, "test-session-2").unwrap();
    let assistant_entry = entries.iter().find(|e| e.role == "assistant").unwrap();
    assert!(assistant_entry.lesson.as_ref().unwrap().contains("shell"));
}

#[tokio::test]
async fn disabled_archivist_is_noop() {
    let hook = ArchivistHook::disabled();
    let ctx = TurnContext {
        user_message: "test".into(),
        assistant_response: "test".into(),
        tool_calls: vec![],
        turn_duration_ms: 0,
        session_id: None,
        iteration_count: 0,
    };
    hook.on_turn_complete(&ctx).await.unwrap();
}

#[test]
fn extract_profile_key_works() {
    let key = extract_profile_key("I prefer dark mode for coding", "preference");
    assert!(key.starts_with("preference_"));
    assert!(key.contains("prefer"));
}

#[tokio::test]
async fn archivist_accumulates_turns_in_segment() {
    let conn = setup_conn();
    let hook = ArchivistHook::new(conn.clone(), true);

    let session = "accum-session";

    for i in 1..=3 {
        hook.on_turn_complete(&TurnContext {
            user_message: format!("Turn number {i}"),
            assistant_response: format!("Response {i}"),
            tool_calls: vec![],
            turn_duration_ms: 50,
            session_id: Some(session.into()),
            iteration_count: i,
        })
        .await
        .unwrap();
    }

    let open_seg = seg::open_segment_for_session(&conn, session)
        .unwrap()
        .expect("Expected an open segment after 3 turns");

    assert_eq!(
        open_seg.turn_count, 3,
        "Segment should have accumulated 3 turns, got {}",
        open_seg.turn_count
    );
}

#[tokio::test]
async fn archivist_extracts_preference_event_on_boundary() {
    let conn = setup_conn();
    let hook = ArchivistHook::new(conn.clone(), true);

    let session = "pref-boundary-session";

    hook.on_turn_complete(&TurnContext {
        user_message: "Tell me about Rust ownership".into(),
        assistant_response: "Ownership is a key concept in Rust.".into(),
        tool_calls: vec![],
        turn_duration_ms: 100,
        session_id: Some(session.into()),
        iteration_count: 1,
    })
    .await
    .unwrap();

    hook.on_turn_complete(&TurnContext {
        user_message: "I prefer dark mode for all my editors".into(),
        assistant_response: "Good to know! Dark mode is easier on the eyes.".into(),
        tool_calls: vec![],
        turn_duration_ms: 100,
        session_id: Some(session.into()),
        iteration_count: 2,
    })
    .await
    .unwrap();

    hook.on_turn_complete(&TurnContext {
        user_message: "Switching to a different topic — how does Tokio work?".into(),
        assistant_response: "Tokio is an async runtime.".into(),
        tool_calls: vec![],
        turn_duration_ms: 100,
        session_id: Some(session.into()),
        iteration_count: 3,
    })
    .await
    .unwrap();

    let events = ev::events_by_type(&conn, "global", "preference", 20).unwrap();
    assert!(
        !events.is_empty(),
        "Expected at least one preference event after segment close; got 0."
    );
    let has_dark_mode = events
        .iter()
        .any(|e| e.content.to_lowercase().contains("prefer"));
    assert!(
        has_dark_mode,
        "Expected a preference event mentioning 'prefer', found: {:?}",
        events.iter().map(|e| &e.content).collect::<Vec<_>>()
    );
}

// ── Phase 0: episodic_capture_enabled independent of learning.enabled ────────

/// When `learning.enabled = false` but `episodic_capture_enabled = true`,
/// the ArchivistHook (constructed directly, as builder.rs would produce)
/// must still write 2 episodic_log rows (user + assistant) and create/advance
/// a segment. This verifies the core contract: episodic capture runs
/// regardless of the learning inference stack toggle.
#[tokio::test]
async fn phase0_episodic_rows_and_segment_without_learning_enabled() {
    let conn = setup_conn();
    // Simulate what builder.rs does when learning.enabled=false but
    // episodic_capture_enabled=true: construct the hook directly with
    // the SQLite conn, enabled=true. No config attached (no LLM recap
    // or tree ingest — those are gated by learning.enabled / chat_to_tree_enabled).
    let hook = ArchivistHook::new(conn.clone(), true);

    let session = "phase0-test-session";

    hook.on_turn_complete(&TurnContext {
        user_message: "Hello, what is Rust?".into(),
        assistant_response: "Rust is a systems language.".into(),
        tool_calls: vec![],
        turn_duration_ms: 100,
        session_id: Some(session.into()),
        iteration_count: 1,
    })
    .await
    .unwrap();

    // Verify 2 episodic rows were written.
    let entries = fts5::episodic_session_entries(&conn, session).unwrap();
    assert_eq!(
        entries.len(),
        2,
        "Expected 2 episodic rows (user + assistant), got {}",
        entries.len()
    );
    assert_eq!(entries[0].role, "user");
    assert_eq!(entries[1].role, "assistant");

    // Verify a segment was created.
    let open_seg = seg::open_segment_for_session(&conn, session)
        .unwrap()
        .expect("Expected an open segment after first turn");
    assert_eq!(open_seg.turn_count, 1);

    // Add a second turn to verify segment advances.
    hook.on_turn_complete(&TurnContext {
        user_message: "Tell me more about ownership.".into(),
        assistant_response: "Ownership prevents data races.".into(),
        tool_calls: vec![],
        turn_duration_ms: 100,
        session_id: Some(session.into()),
        iteration_count: 2,
    })
    .await
    .unwrap();

    let entries2 = fts5::episodic_session_entries(&conn, session).unwrap();
    assert_eq!(
        entries2.len(),
        4,
        "Expected 4 episodic rows after 2 turns, got {}",
        entries2.len()
    );
    let open_seg2 = seg::open_segment_for_session(&conn, session)
        .unwrap()
        .expect("Expected an open segment after 2 turns");
    assert_eq!(
        open_seg2.turn_count, 2,
        "Segment should have 2 turns, got {}",
        open_seg2.turn_count
    );
}

// ── Phase 1: LLM recap + finalize-time embedding ─────────────────────────────

/// Stub ChatProvider that returns a fixed recap string without hitting
/// any real LLM, so the test is hermetic.
struct StubChatProvider;

#[async_trait::async_trait]
impl crate::openhuman::memory::tree::chat::ChatProvider for StubChatProvider {
    fn name(&self) -> &str {
        "stub:test"
    }

    async fn chat_for_json(&self, _prompt: &ChatPrompt) -> anyhow::Result<String> {
        Ok("stub recap: discussed Rust ownership model".to_string())
    }

    async fn chat_for_text(&self, _prompt: &ChatPrompt) -> anyhow::Result<String> {
        Ok("stub recap: discussed Rust ownership model".to_string())
    }
}

/// Stub Embedder that returns a fixed unit vector without hitting Ollama.
struct StubEmbedder;

#[async_trait::async_trait]
impl crate::openhuman::memory::tree::score::embed::Embedder for StubEmbedder {
    fn name(&self) -> &'static str {
        "stub-embedder-v1"
    }

    async fn embed(&self, _text: &str) -> anyhow::Result<Vec<f32>> {
        // Return a simple 4-dim unit vector.
        Ok(vec![0.5_f32, 0.5, 0.5, 0.5])
    }
}

/// Build an ArchivistHook with stub provider + embedder injected directly.
/// Uses the test-only `new_with_stubs` constructor to bypass `with_config`.
fn hook_with_stubs(conn: Arc<Mutex<Connection>>) -> ArchivistHook {
    ArchivistHook::new_with_stubs(conn, Arc::new(StubChatProvider), Arc::new(StubEmbedder))
}

/// When a segment closes, the LLM chat provider recap is used (verified by
/// a non-empty segment summary) and an embedding row is written to
/// `segment_embeddings`.
#[tokio::test]
async fn phase1_llm_recap_and_embedding_on_segment_close() {
    let conn = setup_conn();
    let hook = hook_with_stubs(conn.clone());

    let session = "phase1-recap-test";

    // Turn 1 — opens first segment.
    hook.on_turn_complete(&TurnContext {
        user_message: "Tell me about Rust ownership".into(),
        assistant_response: "Rust's ownership model prevents data races.".into(),
        tool_calls: vec![],
        turn_duration_ms: 100,
        session_id: Some(session.into()),
        iteration_count: 1,
    })
    .await
    .unwrap();

    // Turn 2 — continues same segment.
    hook.on_turn_complete(&TurnContext {
        user_message: "What about the borrow checker?".into(),
        assistant_response: "The borrow checker enforces ownership rules at compile time.".into(),
        tool_calls: vec![],
        turn_duration_ms: 100,
        session_id: Some(session.into()),
        iteration_count: 2,
    })
    .await
    .unwrap();

    // Turn 3 — topic change triggers a boundary → closes first segment → recap + embed fire.
    hook.on_turn_complete(&TurnContext {
        user_message: "Completely different topic: what is async/await in Python?".into(),
        assistant_response: "Python asyncio enables concurrent programming.".into(),
        tool_calls: vec![],
        turn_duration_ms: 100,
        session_id: Some(session.into()),
        iteration_count: 3,
    })
    .await
    .unwrap();

    // Verify segments exist.
    let segments = seg::segments_by_namespace(&conn, "global", 10).unwrap();
    assert!(
        segments.len() >= 2,
        "Expected at least 2 segments (closed + open), got {}",
        segments.len()
    );

    // Find the closed segment (has a summary).
    let closed = segments
        .iter()
        .find(|s| s.summary.as_ref().map(|s| !s.is_empty()).unwrap_or(false));
    assert!(
        closed.is_some(),
        "Expected at least one closed segment with a non-empty summary"
    );

    let closed_seg = closed.unwrap();
    let summary = closed_seg.summary.as_ref().unwrap();
    // The stub provider returns a fixed string — verify it was persisted.
    assert!(
        summary.contains("stub recap"),
        "Expected summary to contain 'stub recap', got: {:?}",
        summary
    );

    // Verify an embedding row was written for the closed segment.
    let embedding =
        seg::segment_embedding_get(&conn, &closed_seg.segment_id, "stub-embedder-v1").unwrap();
    assert!(
        embedding.is_some(),
        "Expected an embedding row for segment={} model=stub-embedder-v1",
        closed_seg.segment_id
    );
    let vec = embedding.unwrap();
    assert_eq!(vec.len(), 4, "Expected 4-dim vector from stub embedder");
    for v in &vec {
        assert!(
            (*v - 0.5_f32).abs() < 1e-4,
            "Expected vector components ≈ 0.5, got {v}"
        );
    }
}

/// `flush_open_segment` must force-close the trailing open segment and
/// trigger recap + embedding even without a boundary-triggering turn.
#[tokio::test]
async fn phase1_flush_open_segment_finalizes_trailing_segment() {
    let conn = setup_conn();
    let hook = hook_with_stubs(conn.clone());

    let session = "phase1-flush-test";

    // Write 2 turns — stays in one open segment (no topic boundary fires).
    for i in 1..=2 {
        hook.on_turn_complete(&TurnContext {
            user_message: format!("Question about Rust turn {i}"),
            assistant_response: format!("Answer about Rust turn {i}"),
            tool_calls: vec![],
            turn_duration_ms: 50,
            session_id: Some(session.into()),
            iteration_count: i,
        })
        .await
        .unwrap();
    }

    // Confirm the segment is still open (no boundary fired).
    let open_seg_before = seg::open_segment_for_session(&conn, session).unwrap();
    assert!(
        open_seg_before.is_some(),
        "Expected an open segment before flush"
    );

    // Flush — should force-close, recap, and embed.
    hook.flush_open_segment(session).await;

    // Segment should now be closed (no open segment for this session).
    let open_seg_after = seg::open_segment_for_session(&conn, session).unwrap();
    assert!(
        open_seg_after.is_none(),
        "Expected no open segment after flush_open_segment"
    );

    // The formerly-open segment should now have a summary.
    let segments = seg::segments_by_namespace(&conn, "global", 10).unwrap();
    let flushed = segments.iter().find(|s| {
        s.session_id == session && s.summary.as_ref().map(|s| !s.is_empty()).unwrap_or(false)
    });
    assert!(
        flushed.is_some(),
        "Expected flushed segment to have a non-empty summary"
    );

    let seg_id = &flushed.unwrap().segment_id;
    let embedding = seg::segment_embedding_get(&conn, seg_id, "stub-embedder-v1").unwrap();
    assert!(
        embedding.is_some(),
        "Expected embedding row for flushed segment={seg_id}"
    );
}
