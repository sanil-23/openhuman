//! Archivist — background PostTurnHook that extracts lessons, indexes
//! episodic records, and manages conversation segments with event extraction.
//!
//! After each turn, the Archivist:
//! 1. Inserts the turn into the FTS5 episodic table.
//! 2. Manages conversation segments (boundary detection + lifecycle).
//! 3. On segment close: produces an LLM recap (soft-fallback to heuristic),
//!    embeds the recap, extracts events, and updates user profile.
//! 4. Extracts simple lessons from tool failures.
//! 5. (Phase 1 / #566) Pipes the turn into the memory tree as `conversations:agent`
//!    when `config.learning.chat_to_tree_enabled` is true.
//! 6. `flush_open_segment` force-closes the trailing open segment at session
//!    end so the last segment always gets a recap + embedding.

use crate::openhuman::agent::hooks::{PostTurnHook, TurnContext};
use crate::openhuman::config::Config;
use crate::openhuman::memory::store::events::{self, EventRecord, EventType};
use crate::openhuman::memory::store::fts5::{self, EpisodicEntry};
use crate::openhuman::memory::store::profile::{self, FacetType};
use crate::openhuman::memory::store::segments::{
    self, BoundaryConfig, BoundaryDecision, ConversationSegment,
};
use crate::openhuman::memory::tree::canonicalize::chat::{ChatBatch, ChatMessage};
use crate::openhuman::memory::tree::chat::{ChatConsumer, ChatProvider};
use crate::openhuman::memory::tree::ingest;
use crate::openhuman::memory::tree::score::embed::{build_embedder_from_config, Embedder};
use crate::openhuman::memory::tree::tree_source::summariser::llm::{
    LlmSummariser, LlmSummariserConfig,
};
use crate::openhuman::memory::tree::tree_source::summariser::{
    Summariser, SummaryContext, SummaryInput,
};
use crate::openhuman::memory::tree::tree_source::types::TreeKind;
use async_trait::async_trait;
use parking_lot::Mutex;
use rusqlite::Connection;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Background Archivist that indexes turns into FTS5 episodic memory
/// and manages conversation segmentation.
///
/// Produces an LLM recap + embedding for each closed segment and flushes
/// the trailing open segment at session end.
pub struct ArchivistHook {
    /// SQLite connection shared with UnifiedMemory.
    conn: Option<Arc<Mutex<Connection>>>,
    /// Whether the archivist is enabled.
    enabled: bool,
    /// Boundary detection configuration.
    boundary_config: BoundaryConfig,
    /// Optional runtime config — used to gate the tree-ingest path and to
    /// build the LLM chat provider + embedder.
    ///
    /// When `None`, the tree-ingest path is skipped. Set via
    /// [`ArchivistHook::with_config`] on the production path.
    config: Option<Config>,
    /// Optional LLM provider for segment recap. When `None`, the
    /// fallback heuristic summary is used instead.
    chat_provider: Option<Arc<dyn ChatProvider>>,
    /// Optional embedder for segment recap vectors. When `None`, embedding
    /// is skipped (segment is still summarised).
    embedder: Option<Arc<dyn Embedder>>,
}

impl ArchivistHook {
    /// Create an Archivist hook with a shared SQLite connection.
    ///
    /// LLM recap and embedding are disabled by default; call
    /// [`Self::with_config`] on the production path to wire them in.
    pub fn new(conn: Arc<Mutex<Connection>>, enabled: bool) -> Self {
        Self {
            conn: Some(conn),
            enabled,
            boundary_config: BoundaryConfig::default(),
            config: None,
            chat_provider: None,
            embedder: None,
        }
    }

    /// Attach runtime config so the archivist can gate the tree-ingest path
    /// and build its LLM chat provider + embedder from config.
    ///
    /// When `config.learning.chat_to_tree_enabled` is `true`, each completed
    /// turn is also piped into the memory tree as `source="conversations:agent"`.
    /// The chat provider is built via `build_chat_provider(config, Summarise)`;
    /// the embedder via `build_embedder_from_config(config)`. Both are
    /// soft-fallback: if construction fails, the fields stay `None` and the
    /// archivist falls back to heuristic summary / no embedding.
    pub fn with_config(mut self, config: Config) -> Self {
        // Build the LLM chat provider for segment recap.
        let chat_provider: Option<Arc<dyn ChatProvider>> =
            match crate::openhuman::memory::tree::chat::build_chat_provider(
                &config,
                ChatConsumer::Summarise,
            ) {
                Ok(p) => {
                    tracing::debug!("[archivist] segment recap provider={} registered", p.name());
                    Some(p)
                }
                Err(e) => {
                    tracing::warn!(
                        "[archivist] failed to build chat provider for recap (will use fallback): {e}"
                    );
                    None
                }
            };

        // Build the embedder for segment recap vectors.
        let embedder: Option<Arc<dyn Embedder>> = match build_embedder_from_config(&config) {
            Ok(e) => {
                tracing::debug!("[archivist] segment embed provider={} registered", e.name());
                Some(Arc::from(e))
            }
            Err(e) => {
                tracing::warn!(
                        "[archivist] failed to build embedder for segment recap (embedding skipped): {e}"
                    );
                None
            }
        };

        self.chat_provider = chat_provider;
        self.embedder = embedder;
        self.config = Some(config);
        self
    }

    /// Create a disabled/no-op Archivist (when FTS5 is not available).
    pub fn disabled() -> Self {
        Self {
            conn: None,
            enabled: false,
            boundary_config: BoundaryConfig::default(),
            config: None,
            chat_provider: None,
            embedder: None,
        }
    }

    /// Flush the currently-open segment for `session_id`, if any, by
    /// force-closing it and running the same close path (recap + embed +
    /// event extraction). This guarantees the trailing segment of a session
    /// is always finalized even when no boundary-triggering turn arrives.
    ///
    /// Called at session end (see `Agent::spawn_session_memory_extraction`
    /// in `session/turn.rs`). Safe to call multiple times — segment_close
    /// is idempotent (only transitions `open → closed`).
    pub async fn flush_open_segment(&self, session_id: &str) {
        if !self.enabled {
            return;
        }
        let Some(conn) = &self.conn else {
            return;
        };
        let now = Self::now_timestamp();
        tracing::debug!("[archivist] flush_open_segment: checking session={session_id}");
        let open_segment = match segments::open_segment_for_session(conn, session_id) {
            Ok(seg) => seg,
            Err(e) => {
                tracing::warn!("[archivist] flush: failed to query open segment: {e}");
                return;
            }
        };
        let Some(segment) = open_segment else {
            tracing::debug!("[archivist] flush: no open segment for session={session_id}");
            return;
        };
        tracing::debug!(
            "[archivist] flush: force-closing segment={} turn_count={}",
            segment.segment_id,
            segment.turn_count
        );
        if let Err(e) = segments::segment_close(conn, &segment.segment_id, now) {
            tracing::warn!("[archivist] flush: failed to close segment: {e}");
            return;
        }
        self.on_segment_closed(conn, &segment, session_id, now)
            .await;
    }

    fn now_timestamp() -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64()
    }

    /// Handle segment lifecycle for a new turn.
    ///
    /// Returns the closed segment (if any) so the caller can run
    /// `on_segment_closed` asynchronously after this function returns.
    /// Event extraction and recap run outside this function because they
    /// are async and may re-acquire the connection lock.
    fn manage_segment_sync(
        &self,
        conn: &Arc<Mutex<Connection>>,
        session_id: &str,
        timestamp: f64,
        user_message: &str,
        current_episodic_id: i64,
    ) -> Option<ConversationSegment> {
        let now = Self::now_timestamp();

        // Check for an open segment for this session.
        let open_segment = match segments::open_segment_for_session(conn, session_id) {
            Ok(seg) => seg,
            Err(e) => {
                tracing::warn!("[archivist] failed to query open segment: {e}");
                return None;
            }
        };

        match open_segment {
            Some(segment) => {
                // Run boundary detection.
                let decision = segments::detect_boundary(
                    &self.boundary_config,
                    &segment,
                    timestamp,
                    user_message,
                    None, // No embedding for now — cosine drift skipped without embedder access.
                );

                match decision {
                    BoundaryDecision::Continue => {
                        tracing::debug!(
                            "[archivist] segment={} continues (turn_count={})",
                            segment.segment_id,
                            segment.turn_count
                        );
                        if let Err(e) = segments::segment_append_turn(
                            conn,
                            &segment.segment_id,
                            current_episodic_id,
                            timestamp,
                            now,
                        ) {
                            tracing::warn!("[archivist] failed to append turn to segment: {e}");
                        }
                        None
                    }
                    BoundaryDecision::Boundary(reason) => {
                        tracing::debug!(
                            "[archivist] segment boundary detected: {reason} — closing {}",
                            segment.segment_id
                        );

                        // Close the current segment.
                        if let Err(e) = segments::segment_close(conn, &segment.segment_id, now) {
                            tracing::warn!("[archivist] failed to close segment: {e}");
                            return None;
                        }

                        // Create a new segment for the new topic.
                        // The new segment starts at the current turn's episodic ID.
                        let new_id = format!("seg-{}", uuid_v4());
                        if let Err(e) = segments::segment_create(
                            conn,
                            &new_id,
                            session_id,
                            "global",
                            current_episodic_id,
                            timestamp,
                            now,
                        ) {
                            tracing::warn!("[archivist] failed to create new segment: {e}");
                        }

                        // Return the closed segment so the caller can run
                        // on_segment_closed asynchronously.
                        Some(segment)
                    }
                }
            }
            None => {
                // No open segment — create the first one using the current episodic ID.
                let segment_id = format!("seg-{}", uuid_v4());
                tracing::debug!(
                    "[archivist] creating first segment={segment_id} for session={session_id}"
                );
                if let Err(e) = segments::segment_create(
                    conn,
                    &segment_id,
                    session_id,
                    "global",
                    current_episodic_id,
                    timestamp,
                    now,
                ) {
                    tracing::warn!("[archivist] failed to create initial segment: {e}");
                }
                None
            }
        }
    }

    /// Called when a segment is closed.
    ///
    /// Produces a segment recap (LLM if a chat provider is configured,
    /// otherwise the heuristic fallback), embeds the recap, extracts
    /// heuristic events, and updates the user profile.
    ///
    /// Soft-fallback contract (mirrors `LlmSummariser`): this function
    /// never returns `Err`; all failures are logged and ignored.
    async fn on_segment_closed(
        &self,
        conn: &Arc<Mutex<Connection>>,
        segment: &ConversationSegment,
        session_id: &str,
        now: f64,
    ) {
        // Gather the conversation text for this segment from episodic entries.
        let entries = fts5::episodic_session_entries(conn, session_id).unwrap_or_default();

        // Filter entries that fall within the segment's time window.
        // Use <= for end_timestamp (entries at the boundary are part of this
        // segment). The boundary-triggering turn has a timestamp AFTER
        // end_timestamp, so it won't be included.
        let segment_entries: Vec<&EpisodicEntry> = entries
            .iter()
            .filter(|e| {
                e.timestamp >= segment.start_timestamp
                    && segment
                        .end_timestamp
                        .map(|end| e.timestamp <= end)
                        .unwrap_or(true)
            })
            .collect();

        if segment_entries.is_empty() {
            tracing::debug!(
                "[archivist] segment={} has no entries — skipping recap",
                segment.segment_id
            );
            return;
        }

        // Build segment text from user messages (for event extraction).
        let segment_text: String = segment_entries
            .iter()
            .filter(|e| e.role == "user")
            .map(|e| e.content.as_str())
            .collect::<Vec<_>>()
            .join(". ");

        // ── Segment recap (LLM or heuristic fallback) ────────────────────
        // Build a full prose corpus from ALL entries (user + assistant
        // prose; tool-call JSON is already excluded because the archivist
        // stores stripped prose in the `content` column).
        let corpus_inputs: Vec<SummaryInput> = segment_entries
            .iter()
            .filter(|e| !e.content.trim().is_empty())
            .map(|e| {
                use crate::openhuman::memory::tree::types::approx_token_count;
                let content = e.content.clone();
                let token_count = approx_token_count(&content);
                let ts = chrono::DateTime::from_timestamp(e.timestamp as i64, 0)
                    .unwrap_or_else(chrono::Utc::now);
                SummaryInput {
                    id: format!("{}-{}", e.role, e.timestamp as u64),
                    content,
                    token_count,
                    entities: Vec::new(),
                    topics: Vec::new(),
                    time_range_start: ts,
                    time_range_end: ts,
                    score: 0.5,
                }
            })
            .collect();

        let summary_ctx = SummaryContext {
            tree_id: &segment.segment_id,
            tree_kind: TreeKind::Source,
            target_level: 0,
            token_budget: 2_000,
        };

        let summary = if let Some(ref provider) = self.chat_provider {
            let cfg = LlmSummariserConfig {
                model: provider.name().to_string(),
                structured_facet_extraction: false,
            };
            let summariser = LlmSummariser::new(cfg, Arc::clone(provider));
            tracing::debug!(
                "[archivist] generating LLM recap for segment={} provider={}",
                segment.segment_id,
                provider.name()
            );
            // `Summariser::summarise` never returns Err per its contract.
            match summariser.summarise(&corpus_inputs, &summary_ctx).await {
                Ok(output) if !output.content.is_empty() => {
                    tracing::debug!(
                        "[archivist] LLM recap ok segment={} chars={}",
                        segment.segment_id,
                        output.content.len()
                    );
                    output.content
                }
                Ok(_) => {
                    tracing::debug!(
                        "[archivist] LLM recap returned empty — using fallback segment={}",
                        segment.segment_id
                    );
                    let first = segment_entries
                        .first()
                        .map(|e| e.content.as_str())
                        .unwrap_or("");
                    let last = segment_entries
                        .last()
                        .map(|e| e.content.as_str())
                        .unwrap_or(first);
                    segments::fallback_summary(first, last, segment.turn_count)
                }
                Err(e) => {
                    tracing::warn!(
                        "[archivist] LLM recap failed (non-fatal) segment={}: {e} — using fallback",
                        segment.segment_id
                    );
                    let first = segment_entries
                        .first()
                        .map(|e| e.content.as_str())
                        .unwrap_or("");
                    let last = segment_entries
                        .last()
                        .map(|e| e.content.as_str())
                        .unwrap_or(first);
                    segments::fallback_summary(first, last, segment.turn_count)
                }
            }
        } else {
            // No chat provider — use heuristic fallback.
            let first = segment_entries
                .first()
                .map(|e| e.content.as_str())
                .unwrap_or("");
            let last = segment_entries
                .last()
                .map(|e| e.content.as_str())
                .unwrap_or(first);
            tracing::debug!(
                "[archivist] no chat provider — using heuristic fallback segment={}",
                segment.segment_id
            );
            segments::fallback_summary(first, last, segment.turn_count)
        };

        // Persist the recap.
        if let Err(e) = segments::segment_set_summary(conn, &segment.segment_id, &summary, now) {
            tracing::warn!("[archivist] failed to set segment summary: {e}");
        } else {
            tracing::debug!(
                "[archivist] recap persisted segment={} summary_chars={}",
                segment.segment_id,
                summary.len()
            );
        }

        // ── Finalize-time embedding ───────────────────────────────────────
        // Embed the recap only when the segment is being finalized (closed).
        // Never embed per-turn or on an open segment — this is the single
        // write point for segment_embeddings rows.
        if let Some(ref embedder) = self.embedder {
            let model_signature = embedder.name().to_string();
            tracing::debug!(
                "[archivist] embedding recap segment={} model={}",
                segment.segment_id,
                model_signature
            );
            match embedder.embed(&summary).await {
                Ok(vec) => {
                    match segments::segment_embedding_upsert(
                        conn,
                        &segment.segment_id,
                        &model_signature,
                        &vec,
                        now,
                    ) {
                        Ok(()) => {
                            tracing::debug!(
                                "[archivist] embedding stored segment={} model={} dim={}",
                                segment.segment_id,
                                model_signature,
                                vec.len()
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                "[archivist] failed to persist segment embedding (non-fatal) segment={}: {e}",
                                segment.segment_id
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "[archivist] embed call failed (non-fatal) segment={} model={}: {e}",
                        segment.segment_id,
                        model_signature
                    );
                }
            }
        } else {
            tracing::debug!(
                "[archivist] no embedder — skipping segment embedding segment={}",
                segment.segment_id
            );
        }

        // ── Heuristic event extraction ────────────────────────────────────
        if segment_text.is_empty() {
            return;
        }

        let extracted = events::extract_events_heuristic(&segment_text);
        tracing::debug!(
            "[archivist] extracted {} events from segment {}",
            extracted.len(),
            segment.segment_id
        );

        for (event_type, content) in &extracted {
            let event_id = format!("evt-{}", uuid_v4());
            let event = EventRecord {
                event_id,
                segment_id: segment.segment_id.clone(),
                session_id: session_id.to_string(),
                namespace: segment.namespace.clone(),
                event_type: event_type.clone(),
                content: content.clone(),
                subject: None,
                timestamp_ref: None,
                confidence: 0.6,
                embedding: None,
                source_turn_ids: None,
                created_at: now,
            };
            if let Err(e) = events::event_insert(conn, &event) {
                tracing::warn!("[archivist] failed to insert event: {e}");
            }

            // Update user profile from preference and fact events.
            match event_type {
                EventType::Preference => {
                    let key = extract_profile_key(content, "preference");
                    let facet_id = format!("prf-{}", uuid_v4());
                    if let Err(e) = profile::profile_upsert(
                        conn,
                        &facet_id,
                        &FacetType::Preference,
                        &key,
                        content,
                        0.6,
                        Some(&segment.segment_id),
                        now,
                    ) {
                        tracing::warn!("[archivist] failed to upsert profile facet: {e}");
                    }
                }
                EventType::Fact => {
                    let key = extract_profile_key(content, "fact");
                    let facet_id = format!("prf-{}", uuid_v4());
                    if let Err(e) = profile::profile_upsert(
                        conn,
                        &facet_id,
                        &FacetType::Context,
                        &key,
                        content,
                        0.6,
                        Some(&segment.segment_id),
                        now,
                    ) {
                        tracing::warn!("[archivist] failed to upsert profile fact: {e}");
                    }
                }
                _ => {}
            }
        }
    }
}

#[async_trait]
impl PostTurnHook for ArchivistHook {
    fn name(&self) -> &str {
        "archivist"
    }

    async fn on_turn_complete(&self, ctx: &TurnContext) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }

        let Some(conn) = &self.conn else {
            return Ok(());
        };

        let session_id = ctx.session_id.as_deref().unwrap_or("unknown");
        let timestamp = Self::now_timestamp();

        tracing::debug!(
            "[archivist] indexing turn: session={session_id}, tools={}, duration={}ms",
            ctx.tool_calls.len(),
            ctx.turn_duration_ms
        );

        // Index user message.
        fts5::episodic_insert(
            conn,
            &EpisodicEntry {
                id: None,
                session_id: session_id.to_string(),
                timestamp,
                role: "user".to_string(),
                content: ctx.user_message.clone(),
                lesson: None,
                tool_calls_json: None,
                cost_microdollars: 0,
            },
        )?;

        // Retrieve the inserted episodic ID for segment tracking.
        let current_episodic_id = {
            let db = conn.lock();
            db.query_row("SELECT last_insert_rowid()", [], |row| row.get::<_, i64>(0))
                .unwrap_or(1)
        };

        // Index assistant response with tool call summary.
        let tool_calls_json = if ctx.tool_calls.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&ctx.tool_calls).unwrap_or_default())
        };

        // Extract a simple lesson from tool failures (lightweight, no LLM needed).
        let lesson = extract_lesson_from_tools(&ctx.tool_calls);

        fts5::episodic_insert(
            conn,
            &EpisodicEntry {
                id: None,
                session_id: session_id.to_string(),
                // Offset by 1ms so assistant entries sort after user entries within
                // the same turn. Relies on turn timestamps having >=1ms resolution.
                timestamp: timestamp + 0.001,
                role: "assistant".to_string(),
                content: ctx.assistant_response.clone(),
                lesson,
                tool_calls_json,
                cost_microdollars: 0,
            },
        )?;

        tracing::debug!("[archivist] episodic rows written: session={session_id}");

        // Manage conversation segmentation (sync boundary detection + SQLite
        // operations). Returns the just-closed segment when a boundary fired.
        let closed_segment = self.manage_segment_sync(
            conn,
            session_id,
            timestamp,
            &ctx.user_message,
            current_episodic_id,
        );

        // Run async recap + embed on the closed segment (if any).
        if let Some(ref segment) = closed_segment {
            let now = Self::now_timestamp();
            self.on_segment_closed(conn, segment, session_id, now).await;
        }

        // ── Phase 1 / #566: pipe turn into the memory tree ───────────────────
        // Gate: only when config is attached and chat_to_tree_enabled is true.
        // Non-fatal: if tree-ingest fails, the episodic write already succeeded
        // and the turn result is not affected.
        if let Some(ref cfg) = self.config {
            if cfg.learning.chat_to_tree_enabled {
                tracing::debug!(
                    "[archivist] piping turn into tree as conversations:agent session={}",
                    session_id
                );
                self.pipe_turn_to_tree(cfg, ctx, session_id, timestamp)
                    .await;
            }
        }

        tracing::debug!("[archivist] turn indexed successfully: session={session_id}");
        Ok(())
    }
}

impl ArchivistHook {
    /// Pipe the completed turn into the memory tree as `source="conversations:agent"`.
    ///
    /// Tool-call JSON is stripped from the assistant text before ingest — only
    /// the assistant's prose response flows into the tree (memory ingestion
    /// policy: tool outputs must not reach memory).
    ///
    /// Failures are logged and swallowed; the episodic write is the source of
    /// truth.
    async fn pipe_turn_to_tree(
        &self,
        config: &Config,
        ctx: &TurnContext,
        session_id: &str,
        timestamp: f64,
    ) {
        use chrono::{TimeZone, Utc};

        // Build turn timestamps. The assistant message is offset by 1ms as in
        // the episodic write so ordering is stable.
        let user_ts = Utc
            .timestamp_opt(
                timestamp as i64,
                ((timestamp.fract() * 1e9) as u32).min(999_999_999),
            )
            .single()
            .unwrap_or_else(Utc::now);
        let asst_ts = Utc
            .timestamp_opt(
                (timestamp + 0.001) as i64,
                (((timestamp + 0.001).fract() * 1e9) as u32).min(999_999_999),
            )
            .single()
            .unwrap_or(user_ts);

        // Strip tool-call JSON from the assistant response.
        // Per memory ingestion policy, structured tool-call payloads must not
        // flow into the tree — only the prose response is ingested.
        let assistant_prose = strip_tool_calls_from_response(&ctx.assistant_response);

        let batch = ChatBatch {
            platform: "agent".into(),
            channel_label: session_id.to_string(),
            messages: vec![
                ChatMessage {
                    author: "user".into(),
                    timestamp: user_ts,
                    text: ctx.user_message.clone(),
                    source_ref: Some(format!("agent://session/{session_id}")),
                },
                ChatMessage {
                    author: "assistant".into(),
                    timestamp: asst_ts,
                    text: assistant_prose,
                    source_ref: Some(format!("agent://session/{session_id}")),
                },
            ],
        };

        // Use the session_id as the owner / identity tag.
        let source_id = "conversations:agent";
        let owner = session_id;
        let tags = vec!["agent_chat".to_string()];

        match ingest::ingest_chat(config, source_id, owner, tags, batch).await {
            Ok(result) => {
                tracing::debug!(
                    "[archivist] tree ingest ok: source_id={} chunks_written={} session={}",
                    source_id,
                    result.chunks_written,
                    session_id
                );
            }
            Err(e) => {
                tracing::warn!(
                    "[archivist] tree ingest failed (non-fatal): source_id={} session={} error={e}",
                    source_id,
                    session_id
                );
            }
        }
    }
}

/// Strip tool-call JSON blocks from an assistant response, leaving only the
/// prose text.
///
/// The archivist stores the full response (including `tool_calls_json`) in
/// the episodic log for diagnostic purposes. However, per the memory
/// ingestion policy, structured tool-call payloads must not reach the memory
/// tree — only the assistant's natural-language prose is ingested.
///
/// This function applies a lightweight heuristic: it removes any contiguous
/// spans of text that look like `<tool_call>…</tool_call>` XML/JSON blocks or
/// raw JSON objects that begin with `{"tool_calls":`. The output may be empty
/// if the entire response was tool-call markup — callers should handle that
/// case (empty text → no-op ingest).
fn strip_tool_calls_from_response(response: &str) -> String {
    // Fast path: if the response contains no obvious tool-call markers, return
    // it unchanged to avoid unnecessary allocation.
    if !response.contains("<tool_call>")
        && !response.contains("{\"tool_calls\"")
        && !response.contains("\"tool_use\"")
    {
        return response.to_string();
    }

    // Remove XML-style tool-call blocks.
    let mut cleaned = response.to_string();

    // Strip <tool_call>…</tool_call> spans (may span multiple lines).
    while let Some(start) = cleaned.find("<tool_call>") {
        if let Some(end) = cleaned[start..].find("</tool_call>") {
            cleaned.drain(start..start + end + "</tool_call>".len());
        } else {
            // Unclosed tag — remove from the tag to end of string.
            cleaned.truncate(start);
            break;
        }
    }

    // Trim and collapse runs of blank lines left by block removal.
    let trimmed = cleaned
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");

    // Collapse more than two consecutive newlines to two.
    let mut result = String::with_capacity(trimmed.len());
    let mut blank_run = 0usize;
    for line in trimmed.lines() {
        if line.is_empty() {
            blank_run += 1;
            if blank_run <= 2 {
                result.push('\n');
            }
        } else {
            blank_run = 0;
            result.push_str(line);
            result.push('\n');
        }
    }

    result.trim().to_string()
}

/// Extract simple lessons from tool call outcomes (no LLM needed).
fn extract_lesson_from_tools(
    tool_calls: &[crate::openhuman::agent::hooks::ToolCallRecord],
) -> Option<String> {
    let failures: Vec<&str> = tool_calls
        .iter()
        .filter(|tc| !tc.success)
        .map(|tc| tc.name.as_str())
        .collect();

    if failures.is_empty() {
        return None;
    }

    Some(format!(
        "Tools that failed in this turn: {}",
        failures.join(", ")
    ))
}

/// Extract a short profile key from event content (first few meaningful words).
fn extract_profile_key(content: &str, prefix: &str) -> String {
    let words: Vec<&str> = content
        .split_whitespace()
        .filter(|w| w.len() > 2)
        .take(4)
        .collect();
    let key = words.join("_").to_lowercase();
    let key = key
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect::<String>();
    if key.is_empty() {
        format!("{prefix}_unknown")
    } else {
        format!("{prefix}_{key}")
    }
}

/// Generate a simple UUID v4 (random).
fn uuid_v4() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{:x}{:08x}", nanos, rand_u32())
}

/// Simple random u32 from system entropy.
fn rand_u32() -> u32 {
    let state = RandomState::new();
    let mut hasher = state.build_hasher();
    hasher.write_u64(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64,
    );
    hasher.finish() as u32
}

#[cfg(test)]
impl ArchivistHook {
    /// Test-only constructor that injects a stub `ChatProvider` and `Embedder`
    /// directly, bypassing `with_config`'s provider-build logic. Used by
    /// Phase 1 tests to verify LLM recap and embedding paths without hitting
    /// a real LLM or Ollama daemon. Exposed as `pub(crate)` so Phase 3
    /// STM recall integration tests can drive the full archivist path.
    pub(crate) fn new_with_stubs(
        conn: Arc<Mutex<Connection>>,
        chat_provider: Arc<dyn ChatProvider>,
        embedder: Arc<dyn Embedder>,
    ) -> Self {
        Self {
            conn: Some(conn),
            enabled: true,
            boundary_config: BoundaryConfig::default(),
            config: None,
            chat_provider: Some(chat_provider),
            embedder: Some(embedder),
        }
    }
}

#[cfg(test)]
#[path = "archivist_tests.rs"]
mod tests;
