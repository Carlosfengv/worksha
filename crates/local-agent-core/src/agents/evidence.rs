//! Explicit client evidence. Prompt text, paths and current routing never identify products.
use crate::plugin::{
    BuiltinPlugin, CapabilityKey, PluginDescriptor, PluginError, PluginHost, PluginResources,
};
use serde_json::Value;
use std::sync::LazyLock;

pub trait ClientEvidence: BuiltinPlugin {
    fn family(&self) -> &'static str;
    fn native(&self, row: &Value) -> Option<Option<String>>;
    fn request(
        &self,
        configured: Option<&str>,
        originator: Option<&str>,
        user_agent: Option<&str>,
    ) -> Option<String>;
    fn reason(&self, row: &Value) -> Option<&'static str>;
}
struct Evidence(&'static str);
fn key(family: &'static str) -> CapabilityKey {
    CapabilityKey {
        name: "agent.identity-evidence",
        version: 1,
        instance: family,
    }
}
impl BuiltinPlugin for Evidence {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor {
            id: self.0,
            version: "1.0.0",
            provides: vec![key(self.0)],
            requires: vec![],
        }
    }
    fn prepare(&self) -> Result<PluginResources, PluginError> {
        Ok(PluginResources::default())
    }
}
impl ClientEvidence for Evidence {
    fn family(&self) -> &'static str {
        self.0
    }
    fn native(&self, row: &Value) -> Option<Option<String>> {
        native_builtin(self.0, row)
    }
    fn request(
        &self,
        configured: Option<&str>,
        originator: Option<&str>,
        user_agent: Option<&str>,
    ) -> Option<String> {
        if self.0 == "codex" {
            codex_request_builtin(configured, originator, user_agent)
        } else {
            None
        }
    }
    fn reason(&self, row: &Value) -> Option<&'static str> {
        if self.0 == "claude" {
            claude_evidence_builtin(row)
        } else {
            None
        }
    }
}
static EVIDENCE: [Evidence; 3] = [Evidence("claude"), Evidence("codex"), Evidence("opencode")];
pub struct EvidenceRegistry {
    host: PluginHost,
    evidence: Vec<&'static dyn ClientEvidence>,
}
impl EvidenceRegistry {
    pub fn builtin() -> Result<Self, PluginError> {
        Self::from_contributions(EVIDENCE.iter().map(|e| e as &dyn ClientEvidence).collect())
    }
    pub fn from_contributions(
        evidence: Vec<&'static dyn ClientEvidence>,
    ) -> Result<Self, PluginError> {
        for entry in &evidence {
            if !entry.descriptor().provides.contains(&key(entry.family())) {
                return Err(PluginError {
                    code: "invalid_input",
                    message: "客户端证据与能力描述不匹配".into(),
                });
            }
        }
        let host = PluginHost::mount(
            &evidence
                .iter()
                .map(|e| *e as &dyn BuiltinPlugin)
                .collect::<Vec<_>>(),
        )?;
        for entry in &evidence {
            host.acquire(&key(entry.family()))?;
        }
        Ok(Self { host, evidence })
    }
    pub fn resolve(&self, family: &str) -> Option<&'static dyn ClientEvidence> {
        let evidence = self
            .evidence
            .iter()
            .copied()
            .find(|e| e.family() == family)?;
        self.host.acquire(&key(evidence.family())).ok()?;
        Some(evidence)
    }
    pub fn stop(&self, family: &str) -> Result<(), PluginError> {
        let id = self
            .evidence
            .iter()
            .find(|entry| entry.family() == family)
            .map(|entry| entry.descriptor().id)
            .unwrap_or(family);
        self.host.stop(id)
    }
}
fn builtin() -> &'static EvidenceRegistry {
    static REGISTRY: LazyLock<EvidenceRegistry> =
        LazyLock::new(|| EvidenceRegistry::builtin().expect("builtin evidence"));
    &REGISTRY
}

pub fn codex_request_product(
    configured: Option<&str>,
    originator: Option<&str>,
    user_agent: Option<&str>,
) -> Option<String> {
    builtin()
        .resolve("codex")?
        .request(configured, originator, user_agent)
}
pub fn native_client(family: &str, row: &Value) -> Option<Option<String>> {
    if let Some(evidence) = builtin().resolve(family) {
        evidence.native(row)
    } else if EVIDENCE.iter().any(|e| e.0 == family) {
        Some(None)
    } else {
        native_builtin(family, row)
    }
}
pub fn claude_client_evidence(row: &Value) -> Option<&'static str> {
    builtin().resolve("claude")?.reason(row)
}

fn codex_product(name: &str) -> Option<&'static str> {
    match name {
        "codex_cli_rs" | "codex_exec" | "codex-tui" => Some("codex-cli"),
        "codex_work_desktop" | "codex_desktop" | "Codex Desktop" => Some("codex-desktop"),
        _ => None,
    }
}

/// Client-declared request identity, only for this authenticated gateway request.
/// Never use the current configured provider, prompt, or generic Codex user agent.
fn codex_request_builtin(
    configured: Option<&str>,
    originator: Option<&str>,
    user_agent: Option<&str>,
) -> Option<String> {
    let origin = originator.and_then(codex_product);
    let ua = user_agent
        .and_then(|s| s.split_once('/'))
        .and_then(|(name, _)| codex_product(name));
    if originator.is_some() && origin.is_none() || origin.zip(ua).is_some_and(|(a, b)| a != b) {
        return None;
    }
    let reported = origin.or(ua);
    if configured.zip(reported).is_some_and(|(a, b)| a != b) {
        return None;
    }
    reported.or(configured).map(str::to_owned)
}

/// Only explicit native client metadata is used, never prompt text or file paths.
/// `Some(None)` distinguishes an explicit but unrecognised client from absent metadata.
fn native_builtin(family: &str, row: &serde_json::Value) -> Option<Option<String>> {
    if family == "opencode" {
        return Some(Some("opencode".into()));
    }
    if family == "claude" {
        let value = row.get("entrypoint").or_else(|| row.get("entryPoint"))?;
        let product = if claude_client_conflict(row) {
            None
        } else {
            claude_product(value.as_str())
        };
        return Some(product.map(str::to_owned));
    }
    let value = if row["type"] == "session_meta" {
        // A generic `source: vscode` is also used by desktop hosts; it is ambiguous.
        row["payload"]
            .get("originator")
            .unwrap_or(&serde_json::Value::Null)
    } else {
        row.get("originator")?
    };
    Some(match (family, value.as_str()) {
        ("codex", Some(name)) => codex_product(name).map(str::to_owned),
        _ => None,
    })
}

fn claude_product(value: Option<&str>) -> Option<&'static str> {
    match value {
        Some("cli" | "claude-cli") => Some("claude-code-cli"),
        Some("claude-desktop" | "claude-desktop-3p") => Some("claude-code-desktop"),
        _ => None,
    }
}
fn claude_client_conflict(row: &serde_json::Value) -> bool {
    let (Some(a), Some(b)) = (row.get("entrypoint"), row.get("entryPoint")) else {
        return false;
    };
    a != b
        && claude_product(a.as_str())
            .zip(claude_product(b.as_str()))
            .is_none_or(|(a, b)| a != b)
}
fn claude_evidence_builtin(row: &serde_json::Value) -> Option<&'static str> {
    let client = native_builtin("claude", row)?;
    Some(if client.is_some() {
        "native_record"
    } else if claude_client_conflict(row) {
        "conflicting_evidence"
    } else if matches!(
        row.get("entrypoint")
            .or_else(|| row.get("entryPoint"))
            .and_then(|v| v.as_str()),
        Some("sdk-cli" | "sdk-ts" | "sdk-py")
    ) {
        "sdk_client"
    } else {
        "unrecognized_client"
    })
}

#[cfg(test)]
mod registry_tests {
    use super::*;
    #[test]
    fn composition_only_exposes_registered_families() {
        let registry = EvidenceRegistry::from_contributions(vec![&EVIDENCE[0]]).unwrap();
        assert!(registry.resolve("claude").is_some());
        assert!(registry.resolve("codex").is_none());
        assert!(EvidenceRegistry::from_contributions(vec![&EVIDENCE[0], &EVIDENCE[0]]).is_err());
    }
    #[test]
    fn stopped_client_cannot_claim_identity_and_other_families_remain_available() {
        let registry = EvidenceRegistry::builtin().unwrap();
        assert_eq!(
            registry
                .resolve("codex")
                .unwrap()
                .request(None, Some("codex_cli_rs"), None)
                .as_deref(),
            Some("codex-cli")
        );
        registry.stop("codex").unwrap();
        assert!(registry.resolve("codex").is_none());
        assert!(registry.resolve("claude").is_some());
        assert!(registry.resolve("unknown").is_none());
    }
}
