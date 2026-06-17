//! Delivery subsystem for finished detached background sub-agents.
//!
//! Surfaces results recorded in [`super::background_completions`] back into the
//! originating chat as a single **system-injected** turn:
//!   * **idle-gated** — never mid-turn; defers while a user turn is in flight,
//!   * **debounced** — a burst of completions batches into one turn,
//!   * **batched** — every result ready at delivery time goes in one turn,
//!     each tagged by its sub-agent process id.
//!
//! The turn is run via [`task_dispatcher::run_system_turn_on_thread`], which
//! streams it into the thread exactly like a chat turn (the same bridge cron /
//! welcome agents use), so it renders in the desktop UI.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;

use crate::core::event_bus::{subscribe_global, DomainEvent, EventHandler, SubscriptionHandle};

use super::background_completions;

/// Coalesce completions landing within this window into one delivery turn.
const DEBOUNCE: Duration = Duration::from_secs(3);

/// Sessions with a user turn currently in flight — delivery defers while busy.
fn busy() -> &'static Mutex<HashSet<String>> {
    static BUSY: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    BUSY.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Sessions whose delivery turn is in flight — prevents two concurrent turns.
fn delivering() -> &'static Mutex<HashSet<String>> {
    static D: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    D.get_or_init(|| Mutex::new(HashSet::new()))
}

fn is_busy(session: &str) -> bool {
    busy()
        .lock()
        .expect("background_delivery busy poisoned")
        .contains(session)
}

struct BackgroundDeliveryHandler;

#[async_trait]
impl EventHandler for BackgroundDeliveryHandler {
    fn name(&self) -> &str {
        "agent_orchestration::background_delivery"
    }

    async fn handle(&self, event: &DomainEvent) {
        match event {
            DomainEvent::AgentTurnStarted { session_id, .. } => {
                busy()
                    .lock()
                    .expect("busy poisoned")
                    .insert(session_id.clone());
            }
            DomainEvent::AgentTurnCompleted { session_id, .. } => {
                busy().lock().expect("busy poisoned").remove(session_id);
                // A user turn just ended — drain anything that finished while it ran.
                schedule_delivery(session_id.clone(), Duration::from_millis(300));
            }
            DomainEvent::SubagentCompleted { parent_session, .. } => {
                // Debounce so a burst of completions batches into a single turn.
                schedule_delivery(parent_session.clone(), DEBOUNCE);
            }
            _ => {}
        }
    }
}

/// Schedule a debounced delivery attempt for a session.
fn schedule_delivery(session: String, delay: Duration) {
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        try_deliver(session).await;
    });
}

/// Drain + deliver pending completions for a session — if idle and not already
/// delivering. Batches everything ready at this instant into one system turn.
async fn try_deliver(session: String) {
    if is_busy(&session) || !background_completions::has_pending(&session) {
        return;
    }
    // Claim the delivery slot (skip if a delivery is already in flight).
    {
        let mut d = delivering().lock().expect("delivering poisoned");
        if !d.insert(session.clone()) {
            return;
        }
    }

    // Re-check busy after claiming — a user turn may have started meanwhile.
    let job = if is_busy(&session) {
        None
    } else {
        let batch = background_completions::take_pending(&session);
        match (
            background_completions::batch_thread_id(&batch),
            background_completions::build_batched_notice(&batch),
        ) {
            (Some(thread_id), Some(notice)) => Some((batch.len(), thread_id, notice)),
            // No originating thread (headless) — nothing to stream into; drop.
            _ => None,
        }
    };

    delivering()
        .lock()
        .expect("delivering poisoned")
        .remove(&session);

    if let Some((count, thread_id, notice)) = job {
        log::info!(
            "[background_delivery] delivering {count} batched background result(s) \
             session={session} thread_id={thread_id}"
        );
        if let Err(e) =
            crate::openhuman::agent::task_dispatcher::run_system_turn_on_thread(thread_id, notice)
                .await
        {
            log::warn!("[background_delivery] delivery turn failed session={session} error={e}");
        }
    }
}

/// Register the delivery subscriber on the global event bus. Keeps the
/// subscription alive for the process lifetime. Idempotent.
pub fn register_background_delivery() {
    static HANDLE: OnceLock<Option<SubscriptionHandle>> = OnceLock::new();
    HANDLE.get_or_init(|| subscribe_global(Arc::new(BackgroundDeliveryHandler)));
}
