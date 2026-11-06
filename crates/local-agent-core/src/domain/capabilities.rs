//! General capability metadata is independent of the session source picker.
use super::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ReadOperation {
    Describe,
    Scan,
    ReadConversation,
    CollectActivity,
    ListMemory,
    ReadMemory,
    ScanCalls,
    CollectProjectEvidence,
    CollectAssets,
    Snapshot,
    Diff,
    CollectListeners,
    RefreshQuota,
    CatalogFacts,
    ExecutionFacts,
    ReadRequests,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityEntry {
    pub provider: Provider,
    pub status: ProviderStatus,
    pub operations: Vec<ReadOperation>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityCatalog {
    pub api_version: String,
    pub revision: Decimal,
    pub entries: Vec<CapabilityEntry>,
    pub external_problem: Option<String>,
}
impl CapabilityCatalog {
    pub fn validate(&self) -> Result<()> {
        if self.api_version != API_VERSION {
            return Err(Failure::new(ErrorCode::IncompatibleApi));
        }
        if self.entries.len() > 512 {
            return Err(Failure::new(ErrorCode::ResourceLimit));
        }
        if self.external_problem.as_ref().is_some_and(|s| {
            s.is_empty() || s.len() > 64 || !s.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
        }) {
            return Err(Failure::new(ErrorCode::InvalidInput));
        }
        let mut keys = HashSet::new();
        let mut slots = HashSet::new();
        for entry in &self.entries {
            let p = &entry.provider;
            let allowed = allowed_operations(&p.capability.name);
            let canonical_key = if p.execution_kind == ExecutionKind::Builtin {
                format!(
                    "builtin:{}@{}:{}",
                    p.capability.name, p.capability.major, p.capability.instance_id
                )
            } else {
                let local = p
                    .capability
                    .instance_id
                    .strip_prefix(&format!("{}:", p.plugin_id))
                    .unwrap_or("");
                format!(
                    "{}:{}@{}:{local}",
                    p.plugin_id, p.capability.name, p.capability.major
                )
            };
            if p.provider_key.is_empty()
                || p.provider_key != canonical_key
                || p.provider_key.len() > 512
                || p.plugin_id.is_empty()
                || p.plugin_id.len() > 100
                || p.implementation_version.is_empty()
                || p.implementation_version.len() > 100
                || p.implementation_identity.is_empty()
                || p.implementation_identity.len() > 256
                || p.capability.major != 1
                || p.capability.instance_id.is_empty()
                || p.capability.instance_id.len() > 240
                || allowed.is_empty()
                || entry.operations.is_empty()
                || entry.operations.iter().any(|op| !allowed.contains(op))
                || entry.operations.iter().collect::<HashSet<_>>().len() != entry.operations.len()
                || !keys.insert(&p.provider_key)
                || !slots.insert((
                    &p.capability.name,
                    p.capability.major,
                    &p.capability.instance_id,
                ))
                || (p.execution_kind != ExecutionKind::Builtin
                    && !super::catalog::external_id(&p.capability.instance_id, &p.plugin_id))
            {
                return Err(Failure::new(ErrorCode::CapabilityConflict));
            }
        }
        Ok(())
    }
}
fn allowed_operations(capability: &str) -> &'static [ReadOperation] {
    use ReadOperation::*;
    match capability {
        "domain.agent-directory" => &[Describe],
        "domain.session-source" => &[Scan, ReadConversation],
        "domain.activity-source" => &[CollectActivity],
        "domain.memory-source" => &[ListMemory, ReadMemory],
        "domain.call-source" => &[ScanCalls],
        "domain.project-source" => &[CollectProjectEvidence, CollectAssets],
        "domain.git-reader" => &[Snapshot, Diff],
        "domain.listener-source" => &[CollectListeners],
        "domain.account-source" => &[RefreshQuota],
        "domain.mcp-source" => &[CatalogFacts, ExecutionFacts],
        "domain.request-source" => &[ReadRequests],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn catalog() -> CapabilityCatalog {
        CapabilityCatalog {
            api_version: API_VERSION.into(),
            revision: 1u64.into(),
            external_problem: None,
            entries: vec![CapabilityEntry {
                provider: Provider {
                    provider_key: "builtin:domain.git-reader@1:native".into(),
                    plugin_id: "project.git".into(),
                    implementation_version: "0.1.8".into(),
                    implementation_identity: "fixed-build".into(),
                    capability: Capability {
                        name: "domain.git-reader".into(),
                        major: 1,
                        instance_id: "native".into(),
                    },
                    execution_kind: ExecutionKind::Builtin,
                },
                status: ProviderStatus {
                    enabled: true,
                    authorization: Authorization::Authorized,
                    grant_revision: 1u64.into(),
                    runtime_active: true,
                    freshness: Freshness::Unread,
                },
                operations: vec![ReadOperation::Snapshot, ReadOperation::Diff],
            }],
        }
    }
    #[test]
    fn general_metadata_is_separate_and_rejects_conflicting_or_undeclared_operations() {
        let original = catalog();
        original.validate().unwrap();
        let mut invalid = original.clone();
        invalid.entries[0]
            .operations
            .push(ReadOperation::RefreshQuota);
        assert!(invalid.validate().is_err());
        let mut duplicate = original.clone();
        duplicate.entries.push(original.entries[0].clone());
        assert_eq!(
            duplicate.validate().unwrap_err().code,
            ErrorCode::CapabilityConflict
        );
        let mut invalid = original.clone();
        invalid.entries[0].provider.capability.name = "domain.cleanup-planner".into();
        assert!(invalid.validate().is_err());
        let mut invalid = original;
        invalid.entries[0].provider.execution_kind = ExecutionKind::WasmComponent;
        assert!(invalid.validate().is_err());
        invalid.entries[0].provider.plugin_id = "org.example.reader".into();
        invalid.entries[0].provider.capability.instance_id = "org.example.reader:git".into();
        assert!(invalid.validate().is_err());
        invalid.entries[0].provider.provider_key =
            "org.example.reader:domain.git-reader@1:git".into();
        invalid.validate().unwrap();
        let mut json = serde_json::to_value(invalid).unwrap();
        json["entries"][0]["provider"]["credentialRef"] = serde_json::json!("forbidden");
        assert!(serde_json::from_value::<CapabilityCatalog>(json).is_err());
    }
    #[test]
    fn native_metadata_has_three_sources_and_does_not_authorize_reading() {
        let home = tempfile::tempdir().unwrap();
        let sessions = crate::sessions::NativeSessions::new(home.path().to_owned(), &[]);
        let entries = sessions.domain_capabilities().unwrap();
        assert_eq!(entries.len(), 16);
        assert!(
            entries
                .iter()
                .all(|e| e.status.authorization == Authorization::Required
                    && e.status.freshness == Freshness::Unread)
        );
        let mut all = catalog();
        all.entries = entries;
        all.validate().unwrap();
        assert_eq!(sessions.domain_catalog().unwrap().products.len(), 5);
        assert!(
            sessions
                .query(crate::sessions::SessionQuery::default())
                .unwrap()
                .items
                .is_empty()
        );
    }
}
