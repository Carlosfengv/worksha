use crate::providers::{ConfigError, fail};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RouteRule {
    pub id: String,
    pub name: String,
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchedRule {
    pub id: String,
    pub name: String,
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentClassifier {
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<crate::providers::upstream_accounts::AccountRef>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IntentOutcome {
    Matched(String),
    Unknown,
    Unavailable,
}

#[derive(Clone, Default)]
pub struct RuleFacts {
    pub agent: String,
    pub agent_product_id: Option<String>,
    pub project_id: Option<String>,
    pub tools: bool,
    pub stream: bool,
    pub input_tokens: u64,
    pub intent: Option<IntentOutcome>,
}

pub(super) fn validate(rules: &[RouteRule], models: &HashSet<String>) -> Result<(), ConfigError> {
    let mut ids = HashSet::new();
    if rules.len() > 16
        || rules.iter().any(|rule| {
            !crate::providers::operations::valid_id(&rule.id)
                || !ids.insert(&rule.id)
                || rule.name.trim().is_empty()
                || rule.name.chars().count() > 80
                || rule.name.chars().any(char::is_control)
                || !models.contains(&rule.model_id)
                || rule.agent.as_deref().is_some_and(|agent| {
                    !matches!(
                        agent,
                        "claude" | "opencode" | "opencode-cli" | "opencode-desktop" | "codex"
                    )
                })
                || rule
                    .project_id
                    .as_deref()
                    .is_some_and(|id| !crate::projects::visibility::valid_id(id))
                || rule
                    .min_input_tokens
                    .is_some_and(|n| !(1..=2_000_000).contains(&n))
                || rule.intent.as_ref().is_some_and(|intent| {
                    intent.is_empty()
                        || intent.trim() != intent
                        || intent.chars().count() > 48
                        || intent.chars().any(char::is_control)
                })
                || (rule.agent.is_none()
                    && rule.project_id.is_none()
                    && rule.tools.is_none()
                    && rule.stream.is_none()
                    && rule.min_input_tokens.is_none()
                    && rule.intent.is_none())
        })
    {
        return fail(
            "invalid_policy",
            "最多可配置 16 条条件规则，每条须填写名称、条件和服务内目标模型",
        );
    }
    Ok(())
}

impl RouteRule {
    pub(super) fn matches(&self, facts: &RuleFacts) -> bool {
        self.matches_metadata(facts)
            && self.intent.as_ref().is_none_or(|wanted| {
                matches!(&facts.intent, Some(IntentOutcome::Matched(actual)) if actual == wanted)
            })
    }

    pub(super) fn matches_metadata(&self, facts: &RuleFacts) -> bool {
        self.agent.as_deref().is_none_or(|id| {
            id == facts.agent
                || (facts.agent == "opencode" && facts.agent_product_id.as_deref() == Some(id))
        }) && self
            .project_id
            .as_ref()
            .is_none_or(|id| facts.project_id.as_ref() == Some(id))
            && self.tools.is_none_or(|required| required == facts.tools)
            && self.stream.is_none_or(|required| required == facts.stream)
            && self
                .min_input_tokens
                .is_none_or(|min| facts.input_tokens >= min)
    }

    pub(super) fn evidence(&self) -> MatchedRule {
        MatchedRule {
            id: self.id.clone(),
            name: self.name.clone(),
            model_id: self.model_id.clone(),
            intent: self.intent.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn opencode_product_rules_require_evidence_and_family_rules_still_match() {
        let rule = |agent| {
            serde_json::from_value::<RouteRule>(json!({
                "id": "f91fe08e-1d80-46cd-9938-cc95fd72f014",
                "name": "OpenCode", "modelId": "model", "agent": agent
            }))
            .unwrap()
        };
        let desktop = rule("opencode-desktop");
        let cli = rule("opencode-cli");
        let family = rule("opencode");
        validate(
            std::slice::from_ref(&desktop),
            &HashSet::from(["model".into()]),
        )
        .unwrap();
        let mut facts = RuleFacts {
            agent: "opencode".into(),
            ..Default::default()
        };
        assert!(family.matches(&facts));
        assert!(!desktop.matches(&facts));
        assert!(!cli.matches(&facts));

        facts.agent_product_id = Some("opencode-desktop".into());
        assert!(desktop.matches(&facts));
        assert!(!cli.matches(&facts));
        assert!(family.matches(&facts));

        facts.agent_product_id = Some("opencode-cli".into());
        assert!(cli.matches(&facts));
        assert!(!desktop.matches(&facts));
        assert!(family.matches(&facts));
    }
}
