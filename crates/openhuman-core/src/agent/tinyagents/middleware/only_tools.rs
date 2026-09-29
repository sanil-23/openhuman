//! A turn its caller narrowed to a named set of tools, one of which it must
//! call.
//!
//! # Why a caller would ask for this
//!
//! A host that records a turn only when it calls a particular tool has a
//! failure it cannot otherwise reach: a turn that answers in prose. The work
//! ran, the model replied, and by that host's own contract nothing happened.
//! Such a host can run the seat again and *insist* -- seeding what was said
//! and offering only the tools that would record it -- but it needs the turn
//! to be genuinely constrained, or the second attempt ends in prose exactly
//! as the first did.
//!
//! This middleware is the constraint and nothing more. It does not know what
//! recording means, does not detect silence and does not decide when to apply
//! itself: the caller names the tools and this retains them. Deciding *that*
//! a turn should be constrained is a policy question, and policy belongs to
//! whoever holds the contract.
//!
//! A name that is not on the turn's belt is ignored rather than an error: a
//! belt is assembled per turn and a caller cannot know every tool a given
//! turn was served. If none of the names are on it, the constraint is dropped
//! whole -- requiring a choice among no tools refuses the call outright, which
//! would turn a caller's mistake into a failed turn.

use std::sync::Arc;

use async_trait::async_trait;
use tinyagents_harness::context::RunContext;
use tinyagents_harness::error::Result as TaResult;
use tinyagents_harness::middleware::Middleware;
use tinyinference_llm::model::{ModelRequest, ToolChoice};

use crate::agent::tinyagents::host::OpenHumanRunContext;

/// Retains only `tools` on every model call of this turn, and requires one of
/// them to be called.
#[derive(Debug)]
pub(crate) struct OnlyToolsMiddleware {
    tools: Arc<[String]>,
}

impl OnlyToolsMiddleware {
    pub(crate) fn new(tools: Vec<String>) -> Self {
        Self {
            tools: tools.into(),
        }
    }
}

#[async_trait]
impl Middleware<(), OpenHumanRunContext> for OnlyToolsMiddleware {
    fn name(&self) -> &str {
        "only_tools"
    }

    async fn before_model(
        &self,
        _ctx: &mut RunContext<OpenHumanRunContext>,
        _state: &(),
        request: &mut ModelRequest,
    ) -> TaResult<()> {
        let kept: Vec<_> = request
            .tools
            .iter()
            .filter(|tool| self.tools.iter().any(|name| name == &tool.name))
            .cloned()
            .collect();
        if kept.is_empty() {
            // None of the named tools is on this belt; see the module note.
            return Ok(());
        }
        request.tools = kept;
        request.tool_choice = ToolChoice::Required;
        Ok(())
    }
}
