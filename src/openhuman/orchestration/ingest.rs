//! DM ingest: decrypt-once → classify → persist → acknowledge.
//!
//! Driven by the existing `DomainEvent::TinyPlaceStreamMessage` (the tinyplace
//! websocket recv loop), filtered to conversation/DM streams. Never logs message
//! bodies or seeds.

use crate::core::event_bus::{publish_global, DomainEvent};
use crate::openhuman::config::Config;
use crate::openhuman::tinyplace::{acknowledge_message, decrypt_envelope};

use super::store;
use super::types::{ChatKind, OrchestrationMessage, OrchestrationSession, SessionEnvelopeV1};

const LOG: &str = "orchestration";

/// True for streams that carry ciphertext DM envelopes worth ingesting.
fn is_dm_stream(kind: &str, stream_id: &str) -> bool {
    kind.eq_ignore_ascii_case("conversation")
        || kind.eq_ignore_ascii_case("dm")
        || stream_id.starts_with("conversation:")
}

/// Entry point from the bus subscriber. Cheap no-op when orchestration is
/// disabled or the stream is not a DM stream.
pub async fn ingest_stream_message(
    config: &Config,
    kind: &str,
    stream_id: &str,
    raw: &serde_json::Value,
) {
    if !config.orchestration.enabled {
        return;
    }
    if !is_dm_stream(kind, stream_id) {
        return;
    }
    let envelope: tinyplace::types::MessageEnvelope = match serde_json::from_value(raw.clone()) {
        Ok(env) => env,
        Err(e) => {
            log::debug!(target: LOG, "[orchestration] ingest.skip stream={stream_id} not-an-envelope err={e}");
            return;
        }
    };
    if let Err(e) = ingest_one(config, envelope).await {
        log::warn!(target: LOG, "[orchestration] ingest.error stream={stream_id}: {e}");
    }
}

async fn ingest_one(
    config: &Config,
    envelope: tinyplace::types::MessageEnvelope,
) -> Result<(), String> {
    let msg_id = envelope.id.clone();
    let agent_id = envelope.from.clone();
    log::debug!(target: LOG, "[orchestration] ingest.entry id={msg_id} from={agent_id}");

    // 1. Dedupe BEFORE decrypt — protects the non-idempotent Signal ratchet.
    let workspace_dir = config.workspace_dir.clone();
    let already = store::with_connection(&workspace_dir, |c| store::message_exists(c, &msg_id))
        .map_err(|e| format!("store lookup: {e}"))?;
    if already {
        log::debug!(target: LOG, "[orchestration] ingest.dedupe id={msg_id}");
        return Ok(());
    }

    // 2. Decrypt exactly once.
    let plaintext = decrypt_envelope(&envelope).await?;

    // 3. Classify: harness envelope → Session, else the peer's Master window.
    let (chat_kind, session_id, role, source, label, workspace, seq, body, ts) =
        match SessionEnvelopeV1::parse(&plaintext) {
            Some(env) => {
                let label = (env.scope.scope_type == "folder").then(|| env.scope.key.clone());
                let workspace = (!env.scope.cwd.is_empty()).then(|| env.scope.cwd.clone());
                (
                    ChatKind::Session,
                    env.scope.harness_session_id,
                    env.message.role,
                    env.harness.provider,
                    label,
                    workspace,
                    env.message.line,
                    env.message.text,
                    if env.message.timestamp.is_empty() {
                        envelope.timestamp.clone()
                    } else {
                        env.message.timestamp
                    },
                )
            }
            None => (
                ChatKind::Master,
                "master".to_string(),
                "user".to_string(),
                String::new(),
                None,
                None,
                0,
                plaintext,
                envelope.timestamp.clone(),
            ),
        };

    // 4. Persist (idempotent).
    let now = chrono::Utc::now().to_rfc3339();
    let session_id_for_event = session_id.clone();
    let agent_id_for_event = agent_id.clone();
    let chat_kind_str = chat_kind.as_str().to_string();
    let landed = store::with_connection(&workspace_dir, |c| {
        store::upsert_session(
            c,
            &OrchestrationSession {
                session_id: session_id.clone(),
                agent_id: agent_id.clone(),
                source,
                label,
                workspace,
                last_seq: seq,
                created_at: now.clone(),
                last_message_at: ts.clone(),
            },
        )?;
        store::insert_message(
            c,
            &OrchestrationMessage {
                id: msg_id.clone(),
                agent_id: agent_id.clone(),
                session_id: session_id.clone(),
                chat_kind,
                role,
                body,
                timestamp: ts.clone(),
                seq,
            },
        )
    })
    .map_err(|e| format!("persist: {e}"))?;

    // 5. Acknowledge (consume once) + fan out for stages 4/7.
    if landed {
        if let Err(e) = acknowledge_message(&msg_id).await {
            log::warn!(target: LOG, "[orchestration] ingest.ack_failed id={msg_id}: {e}");
        }
        publish_global(DomainEvent::OrchestrationSessionMessage {
            agent_id: agent_id_for_event,
            session_id: session_id_for_event,
            chat_kind: chat_kind_str,
        });
    }
    log::debug!(target: LOG, "[orchestration] ingest.exit id={msg_id} landed={landed}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dm_stream_filter() {
        assert!(is_dm_stream("conversation", "conversation:abc"));
        assert!(is_dm_stream("DM", "x"));
        assert!(is_dm_stream("other", "conversation:abc"));
        assert!(!is_dm_stream("inbox", "inbox"));
    }
}
