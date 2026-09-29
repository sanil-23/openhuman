//! [`TextToolCallSalvageMiddleware`]: read a tool call the model wrote as
//! prose, when the provider handed us none.

use async_trait::async_trait;
use serde_json::{Map, Value};

use tinyagents_harness::context::RunContext;
use tinyagents_harness::error::Result as TaResult;
use tinyagents_harness::middleware::Middleware;
use tinyinference_llm::message::ContentBlock;
use tinyinference_llm::model::ModelResponse;
use tinyinference_llm::tool::ToolCall;

/// DeepSeek writes its calls with a fullwidth vertical line (U+FF5C), not an
/// ASCII pipe. Matching the ASCII form finds nothing.
const OPEN_CALLS: &str = "<｜DSML｜tool_calls>";
const OPEN_INVOKE: &str = "<｜DSML｜invoke name=\"";
const CLOSE_INVOKE: &str = "</｜DSML｜invoke>";
const OPEN_PARAM: &str = "<｜DSML｜parameter name=\"";
const CLOSE_PARAM: &str = "</｜DSML｜parameter>";

/// Whether `text` carries a call the provider did not parse.
#[must_use]
pub(crate) fn looks_like_markup(text: &str) -> bool {
    text.contains(OPEN_CALLS)
}

/// The calls a model wrote into its message text, in the order it wrote them.
///
/// Reads DeepSeek's DSML block only — the one dialect this has evidence for.
/// A second dialect guessed at from its documentation would be a parser
/// nothing has ever exercised, and a wrong one mints a call the model did not
/// make, which is worse than the silence it replaces.
///
/// `string="true"` marks a value to take verbatim; anything else is parsed as
/// JSON and falls back to the verbatim string when that fails, so a malformed
/// number arrives as text rather than losing the whole call.
#[must_use]
pub(crate) fn parse_markup(text: &str) -> Vec<ToolCall> {
    let mut calls = Vec::new();
    let mut rest = match text.find(OPEN_CALLS) {
        Some(at) => &text[at + OPEN_CALLS.len()..],
        None => return calls,
    };
    while let Some(at) = rest.find(OPEN_INVOKE) {
        let after = &rest[at + OPEN_INVOKE.len()..];
        let Some(end_name) = after.find('"') else { break };
        let name = &after[..end_name];
        // The body runs to this invoke's close, or to the end of what we were
        // given: a block cut off mid-flight still names its tool, and the
        // arguments it did finish are better than nothing.
        let body_from = &after[end_name..];
        let body = body_from
            .find(CLOSE_INVOKE)
            .map_or(body_from, |close| &body_from[..close]);
        if !name.trim().is_empty() {
            calls.push(ToolCall {
                id: format!("salvaged_{}", calls.len()),
                name: name.to_owned(),
                arguments: Value::Object(parse_params(body)),
                invalid: None,
            });
        }
        let consumed = end_name
            + body_from
                .find(CLOSE_INVOKE)
                .map_or(body_from.len(), |close| close + CLOSE_INVOKE.len());
        rest = &after[consumed.min(after.len())..];
    }
    calls
}

/// One invoke's `<parameter>` children, as an argument object.
fn parse_params(body: &str) -> Map<String, Value> {
    let mut args = Map::new();
    let mut rest = body;
    while let Some(at) = rest.find(OPEN_PARAM) {
        let after = &rest[at + OPEN_PARAM.len()..];
        let Some(end_name) = after.find('"') else { break };
        let name = &after[..end_name];
        let tail = &after[end_name..];
        // `string="true"` sits between the name and the closing `>`.
        let Some(open_end) = tail.find('>') else { break };
        let attrs = &tail[..open_end];
        let value_from = &tail[open_end + 1..];
        let Some(close) = value_from.find(CLOSE_PARAM) else {
            break;
        };
        let raw = &value_from[..close];
        let verbatim = attrs.contains("string=\"true\"");
        let value = if verbatim {
            Value::String(raw.to_owned())
        } else {
            serde_json::from_str(raw.trim()).unwrap_or_else(|_| Value::String(raw.to_owned()))
        };
        args.insert(name.to_owned(), value);
        rest = &value_from[close + CLOSE_PARAM.len()..];
    }
    args
}

/// Hands the loop a call the model wrote as text.
///
/// # The failure this repairs
///
/// A model whose structured tool-calling degrades — long context, repeated
/// compression — falls back to emitting its provider's native markup as
/// message *content*. The provider reports no `tool_calls`, so the loop reads
/// a turn that asked for nothing and ends it. The agent did call a tool; the
/// harness could not see it.
///
/// Observed live across two episodes on one seat: five consecutive turns, each
/// after seven or more compressions, each ending with a `file_read` or
/// `file_write` written out in DSML. Every one of them was recorded as a seat
/// that said nothing, and the research behind them was thrown away.
///
/// Forcing the call cannot fix this — [`super::MustRecordMiddleware`] held the
/// floor for exactly this seat and set `tool_choice = Required`, and the model
/// answered with more markup. A provider-side constraint binds only a model
/// still speaking the structured protocol. This one is not, so the markup has
/// to be read.
///
/// # Only where there is nothing to lose
///
/// Runs only when the provider parsed **no** calls. A response that carries
/// even one structured call is left exactly as it is: a model that wrote about
/// a tool while calling another must not have its prose mined for a second
/// call it never meant to make.
pub(crate) struct TextToolCallSalvageMiddleware;

#[async_trait]
impl Middleware<(), crate::agent::tinyagents::host::OpenHumanRunContext>
    for TextToolCallSalvageMiddleware
{
    fn name(&self) -> &str {
        "text_tool_call_salvage"
    }

    async fn after_model(
        &self,
        _ctx: &mut RunContext<crate::agent::tinyagents::host::OpenHumanRunContext>,
        _state: &(),
        response: &mut ModelResponse,
    ) -> TaResult<()> {
        if !response.message.tool_calls.is_empty() {
            return Ok(());
        }
        let text: String = response
            .message
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        if !looks_like_markup(&text) {
            return Ok(());
        }
        let salvaged = parse_markup(&text);
        if salvaged.is_empty() {
            tracing::warn!(
                "[tinyagents::mw] the reply carries tool-call markup this cannot read; \
                 the turn will record nothing"
            );
            return Ok(());
        }
        tracing::info!(
            calls = ?salvaged.iter().map(|call| call.name.as_str()).collect::<Vec<_>>(),
            "[tinyagents::mw] the model wrote its tool calls as text; reading them as calls"
        );
        response.message.tool_calls = salvaged;
        Ok(())
    }
}

#[cfg(test)]
#[path = "text_tool_calls_tests.rs"]
mod tests;
