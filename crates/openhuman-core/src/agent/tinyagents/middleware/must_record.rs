//! [`MustRecordMiddleware`]: do not let a turn end until the agent has called
//! one of the verbs its host records.

use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;

use tinyagents_harness::context::RunContext;
use tinyagents_harness::error::Result as TaResult;
use tinyagents_harness::middleware::{Middleware, ToolInvocationIdentity};
use tinyinference_llm::model::{ModelRequest, ModelResponse, ToolChoice};
use tinytools::ToolResult as TaToolResult;

/// Holds the floor open until the agent calls a **recording** verb.
///
/// # The gap this closes
///
/// Some hosts record a turn only when it calls a particular tool, and nothing
/// else -- for such a host "a reply that calls no tool records nothing" is the
/// literal contract. The tools are the host's to name; this middleware is
/// constructed with them and knows nothing else about them. A turn that ends in prose is therefore silent: the work
/// ran, the model answered, and the desk heard none of it. The host's only
/// remedy is to run the seat again with a note, which costs a full round and
/// teaches nothing durable -- observed live as a seat that was told, corrected
/// itself, and made the same mistake nine rounds later.
///
/// Prose is not the only way in. A model that emits its tool call as *text*
/// (a provider whose native markup was not parsed) lands here too, having
/// genuinely tried to call a tool, and so does one whose call was refused.
/// What they share is the outcome, which is why this guards the outcome.
///
/// # How
///
/// Two hooks, and the loop's own machinery underneath:
///
/// - [`Middleware::after_model`] sees a response that requests no tool while
///   nothing has been recorded, and sets
///   [`ModelResponse::continue_turn`] rather than letting it stand as the
///   answer. The run loop takes that as "the model is not finished", pushes the
///   note as a user message, and iterates -- the same path a model asking for
///   another round already uses, bounded by the same `max_model_calls`.
/// - [`Middleware::before_model`] then narrows that next call to the recording
///   verbs alone and sets [`ToolChoice::Required`], so the model cannot answer
///   in prose again. It is a constrained choice among the verbs, not a forced
///   single one: which verb ends a turn is the agent's to decide.
///
/// # Once
///
/// The floor is held exactly once per turn. If the forced call still records
/// nothing -- the provider fails it, the markup breaks again -- the turn ends
/// silent and the host falls back to whatever it does for a turn that recorded
/// nothing. Holding it twice would spend a second round on a model that has
/// already shown it cannot comply, and the caller's cap is not a budget to
/// burn on retries.
///
/// Inert when `recording` is empty, which is every host that records a turn by
/// its text.
pub(crate) struct MustRecordMiddleware {
    /// Verb names that record. Empty leaves this middleware inert.
    recording: Vec<String>,
    /// A recording verb has been called in this turn.
    recorded: AtomicBool,
    /// The floor has been held once already.
    held: AtomicBool,
}

impl MustRecordMiddleware {
    /// Over the verbs this host records a turn by.
    pub(crate) fn new(recording: Vec<String>) -> Self {
        Self {
            recording,
            recorded: AtomicBool::new(false),
            held: AtomicBool::new(false),
        }
    }

    /// Whether this turn still owes the host a recorded verb.
    fn owes(&self) -> bool {
        !self.recording.is_empty() && !self.recorded.load(Ordering::SeqCst)
    }

    /// The note handed back with the floor, naming the verbs it may use.
    fn note(&self) -> String {
        format!(
            "Your reply recorded nothing: prose ends your turn but is not \
             speech, and nobody received it. Say it now in a call to one of \
             {}. Report what you established -- do not reach further first.",
            self.recording
                .iter()
                .map(|verb| format!("`{verb}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

#[async_trait]
impl Middleware<(), crate::agent::tinyagents::host::OpenHumanRunContext>
    for MustRecordMiddleware
{
    fn name(&self) -> &str {
        "must_record"
    }

    async fn before_model(
        &self,
        _ctx: &mut RunContext<crate::agent::tinyagents::host::OpenHumanRunContext>,
        _state: &(),
        request: &mut ModelRequest,
    ) -> TaResult<()> {
        // Only the call this middleware asked for, and only while the debt
        // stands: a recording verb landing in between clears it.
        if !self.held.load(Ordering::SeqCst) || !self.owes() {
            return Ok(());
        }
        request
            .tools
            .retain(|tool| self.recording.iter().any(|verb| verb == &tool.name));
        if request.tools.is_empty() {
            // The verbs are not on this turn's belt after all. Forcing a choice
            // among none would refuse the call outright, so leave the request
            // as it was and let the turn end.
            return Ok(());
        }
        request.tool_choice = ToolChoice::Required;
        Ok(())
    }

    async fn after_model(
        &self,
        _ctx: &mut RunContext<crate::agent::tinyagents::host::OpenHumanRunContext>,
        _state: &(),
        response: &mut ModelResponse,
    ) -> TaResult<()> {
        if !response.message.tool_calls.is_empty() {
            // It asked for a tool; whether that tool records is settled in
            // `after_tool`, once it has actually run.
            return Ok(());
        }
        if !self.owes() || self.held.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        tracing::info!(
            verbs = ?self.recording,
            "[tinyagents::mw] turn would end without recording; holding the floor for one call"
        );
        response.continue_turn = Some(self.note());
        Ok(())
    }

    async fn after_tool(
        &self,
        _ctx: &mut RunContext<crate::agent::tinyagents::host::OpenHumanRunContext>,
        _state: &(),
        invocation: &ToolInvocationIdentity,
        result: &mut TaToolResult,
    ) -> TaResult<()> {
        // A refused call records nothing, so it does not clear the debt --
        // which is the case this exists for.
        if result.is_error {
            return Ok(());
        }
        if self
            .recording
            .iter()
            .any(|verb| verb == invocation.tool_name())
        {
            self.recorded.store(true, Ordering::SeqCst);
        }
        Ok(())
    }
}
