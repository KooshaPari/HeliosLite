use forge_domain::AgentId;
use insta::{assert_snapshot, assert_yaml_snapshot};
use pretty_assertions::assert_eq;

use super::*;

#[tokio::test]
async fn test_parse_basic_agent() {
    let content = forge_test_kit::fixture!("/src/fixtures/agents/basic.md").await;

    let actual = parse_agent_file(&content).unwrap();

    assert_eq!(actual.id.as_str(), "test-basic");
    assert_eq!(actual.title.as_ref().unwrap(), "Basic Test Agent");
    assert_eq!(
        actual.description.as_ref().unwrap(),
        "A simple test agent for basic functionality"
    );
    assert_eq!(
        actual.system_prompt.as_ref().unwrap().template,
        "This is a basic test agent used for testing fundamental functionality."
    );
}

#[tokio::test]
async fn test_parse_advanced_agent() {
    let content = forge_test_kit::fixture!("/src/fixtures/agents/advanced.md").await;

    let actual = parse_agent_file(&content).unwrap();

    assert_eq!(actual.id.as_str(), "test-advanced");
    assert_eq!(actual.title.as_ref().unwrap(), "Advanced Test Agent");
    assert_eq!(
        actual.description.as_ref().unwrap(),
        "An advanced test agent with full configuration"
    );
}

#[test]
fn test_parse_agent_file_renders_conditional_frontmatter_when_subagents_enabled() {
    let fixture = r#"---
id: "forge"
tools:
  - read
  - task
  - sage
  - mcp_*
---
Body keeps {{tool_names.read}} untouched.
"#;
    let config = ForgeConfig { subagents: true, ..Default::default() };

    let actual = apply_subagent_tool_config(parse_agent_file(fixture).unwrap(), &config).unwrap();

    assert_eq!(actual.id, AgentId::new("forge"));
    assert_eq!(
        actual.system_prompt.unwrap().template,
        "Body keeps {{tool_names.read}} untouched."
    );
    assert_yaml_snapshot!("parse_agent_file_subagents_enabled_tools", actual.tools);
}

#[test]
fn test_parse_agent_file_renders_conditional_frontmatter_when_subagents_disabled() {
    let fixture = r#"---
id: "forge"
tools:
  - read
  - task
  - sage
  - mcp_*
---
Body keeps {{tool_names.read}} untouched.
"#;
    let config = ForgeConfig { subagents: false, ..Default::default() };

    let actual = apply_subagent_tool_config(parse_agent_file(fixture).unwrap(), &config).unwrap();

    assert_eq!(actual.id, AgentId::new("forge"));
    assert_snapshot!(
        "parse_agent_file_subagents_disabled_prompt",
        actual.system_prompt.unwrap().template
    );
    assert_yaml_snapshot!("parse_agent_file_subagents_disabled_tools", actual.tools);
}

#[test]
fn test_parse_agent_file_preserves_runtime_user_prompt_variables() {
    let fixture = r#"---
id: "forge"
tools:
  - read
  - task
  - sage
  - mcp_*
user_prompt: |-
  <{{event.name}}>{{event.value}}</{{event.name}}>
  <system_date>{{current_date}}</system_date>
---
Body keeps {{tool_names.read}} untouched.
"#;

    let actual = parse_agent_file(fixture).unwrap();
    let actual_user_prompt = actual.user_prompt.clone().unwrap().template;

    assert_eq!(actual.id, AgentId::new("forge"));
    assert_snapshot!(
        "parse_agent_file_preserves_runtime_user_prompt_variables",
        actual_user_prompt
    );
    assert_yaml_snapshot!(
        "parse_agent_file_preserves_runtime_user_prompt_variables_tools",
        apply_subagent_tool_config(
            actual,
            &ForgeConfig { subagents: true, ..Default::default() }
        )
        .unwrap()
        .tools
    );
}

#[test]
fn interactive_agents_offer_real_followup_tool() {
    for source in [
        include_str!("agents/forge.md"),
        include_str!("agents/muse.md"),
    ] {
        let agent = parse_agent_file(source).unwrap();
        assert!(
            agent
                .tools
                .unwrap()
                .iter()
                .any(|tool| tool.as_str() == "followup")
        );
    }
}
