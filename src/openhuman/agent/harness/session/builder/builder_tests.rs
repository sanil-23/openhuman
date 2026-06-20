//! Tests for the builder module — dedup_visible_tool_specs and related logic.

use super::{
    dedup_visible_tool_specs, should_synthesize_delegation_tools, visible_tool_specs_for_policy,
};
use crate::openhuman::tools::ToolSpec;
use serde_json::json;

fn spec(name: &str) -> ToolSpec {
    ToolSpec {
        name: name.to_string(),
        description: format!("description for {name}"),
        parameters: json!({}),
    }
}

/// A permissive policy session that allows every name in `allowed`.
fn policy_allowing(allowed: &[&str]) -> crate::openhuman::agent_tool_policy::ToolPolicySession {
    use crate::openhuman::agent_tool_policy::{TaskProfile, TaskRiskLevel, ToolPolicySession};
    use crate::openhuman::tools::traits::PermissionLevel;
    ToolPolicySession {
        profile: TaskProfile {
            agent_id: "test".into(),
            channel: "test".into(),
            entrypoint: "test".into(),
            risk_level: TaskRiskLevel::Low,
            allowed_permission: PermissionLevel::Dangerous,
        },
        capabilities: Vec::new(),
        allowed_tool_names: allowed.iter().map(|s| s.to_string()).collect(),
        blocked_tool_names: Default::default(),
        hidden_tool_names: Default::default(),
        decisions: Default::default(),
    }
}

#[test]
fn recovery_tool_is_advertised_even_under_named_visibility() {
    use crate::openhuman::agent::harness::compaction::RECOVERY_TOOL_NAME;
    use std::collections::HashSet;

    let specs = vec![spec("file_read"), spec(RECOVERY_TOOL_NAME), spec("grep")];
    // A Named-scope agent that only allow-lists `file_read` — the recovery tool
    // is NOT in its visibility set...
    let visible: HashSet<String> = ["file_read".to_string()].into_iter().collect();
    let policy = policy_allowing(&["file_read", RECOVERY_TOOL_NAME]);

    let out = visible_tool_specs_for_policy(&specs, &visible, &policy);
    let names: Vec<&str> = out.iter().map(|s| s.name.as_str()).collect();

    // ...yet retrieve_tool_output is still advertised, so the agent can act on
    // a compaction footer; `grep` (not allow-listed) stays hidden.
    assert!(names.contains(&"file_read"));
    assert!(
        names.contains(&RECOVERY_TOOL_NAME),
        "recovery tool must be advertised: {names:?}"
    );
    assert!(!names.contains(&"grep"));
}

#[test]
fn recovery_tool_still_respects_explicit_policy_block() {
    use crate::openhuman::agent::harness::compaction::RECOVERY_TOOL_NAME;
    use std::collections::HashSet;
    // If policy genuinely disallows it, the visibility bypass must NOT override
    // policy (defense-in-depth).
    let specs = vec![spec(RECOVERY_TOOL_NAME)];
    let visible: HashSet<String> = HashSet::new();
    let policy = policy_allowing(&["something_else"]); // recovery NOT allowed
    let out = visible_tool_specs_for_policy(&specs, &visible, &policy);
    assert!(
        out.is_empty(),
        "policy block must win over the visibility bypass"
    );
}

#[test]
fn drops_duplicates_first_wins() {
    // Real-world collision: researcher's `delegate_name = "research"`
    // synthesises a delegate tool that shadows a same-named skill.
    // Anthropic 400s on duplicate tool names; the dedup helper must
    // keep the *first* occurrence so registration order semantics
    // are preserved (the underlying tool dispatch lookup-by-name
    // still resolves the right tool).
    let specs = vec![
        spec("research"), // skill
        spec("plan"),
        spec("research"), // delegate, dropped
        spec("run_code"),
        spec("plan"), // dropped
    ];

    let deduped = dedup_visible_tool_specs(specs);

    let names: Vec<&str> = deduped.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["research", "plan", "run_code"]);
}

#[test]
fn passes_through_when_no_duplicates() {
    let specs = vec![spec("a"), spec("b"), spec("c")];
    let deduped = dedup_visible_tool_specs(specs);
    assert_eq!(deduped.len(), 3);
    assert_eq!(deduped[0].name, "a");
    assert_eq!(deduped[1].name, "b");
    assert_eq!(deduped[2].name, "c");
}

#[test]
fn handles_empty_input() {
    let deduped = dedup_visible_tool_specs(Vec::<ToolSpec>::new());
    assert!(deduped.is_empty());
}

#[test]
fn preserves_full_spec_content_for_kept_entries() {
    // Description + parameters must survive the dedup pass intact —
    // the LLM uses both for tool-call decisions, and corrupting them
    // would silently degrade function-calling quality.
    let mut spec_a = spec("alpha");
    spec_a.description = "first alpha — should win".to_string();
    spec_a.parameters = json!({"type": "object", "required": ["x"]});

    let mut spec_a_dup = spec("alpha");
    spec_a_dup.description = "second alpha — should be dropped".to_string();

    let deduped = dedup_visible_tool_specs(vec![spec_a.clone(), spec_a_dup]);

    assert_eq!(deduped.len(), 1);
    assert_eq!(deduped[0].description, "first alpha — should win");
    assert_eq!(
        deduped[0].parameters,
        json!({"type": "object", "required": ["x"]})
    );
}

#[test]
fn automatic_memory_policy_does_not_synthesize_delegate_tools() {
    let defs = crate::openhuman::agent_registry::agents::load_builtins().unwrap();
    let help = defs
        .iter()
        .find(|def| def.id == "help")
        .expect("help agent is built in");
    let orchestrator = defs
        .iter()
        .find(|def| def.id == "orchestrator")
        .expect("orchestrator is built in");

    assert!(
        !should_synthesize_delegation_tools(help),
        "automatic memory policy should not add delegate tools"
    );
    assert!(
        should_synthesize_delegation_tools(orchestrator),
        "orchestrator still needs synthesized delegate tools"
    );
}
