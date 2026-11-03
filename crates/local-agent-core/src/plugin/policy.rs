//! Pure routing policy capabilities of the shared external plugin API.
pub use super::external::{API_VERSION, Manifest, Runtime, SIGNING_DOMAIN};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
pub const CAPABILITY: &str = "domain.routing-policy@1";
pub const FEATURE: &str = "host.pure-policy@1";
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PackageKind {
    RoutingPolicy,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageQuery {
    pub plugin_id: String,
    pub digest: String,
}
/// The host supplies the digest; a guest only echoes it, never establishes authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RankInvocation {
    pub request: crate::domain::routing_policy::RankRequest,
    pub request_identity: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Review {
    pub package_ref: String,
    pub digest: String,
    pub manifest: Manifest,
    pub signing_key: String,
    pub trusted_publisher: bool,
    pub compressed_bytes: String,
    pub expanded_bytes: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum FailureMode {
    Reject,
    Builtin,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum StateAction {
    Enable,
    Disable,
    RevokeGrants,
    RevokePublisher,
    Remove,
    Rollback,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageStatus {
    pub manifest: Manifest,
    pub digest: String,
    pub signing_key: String,
    pub enabled: bool,
    pub active: bool,
    pub publisher_trusted: bool,
    pub epoch: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BindingStatus {
    pub store_id: String,
    pub service_id: String,
    pub service_version_id: String,
    pub revision: String,
    pub package: PackageQuery,
    pub failure_mode: FailureMode,
    pub authorized: bool,
    pub error_code: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScopeStatus {
    pub store_id: String,
    pub service_id: String,
    pub revision: String,
    pub pending_operation_id: Option<String>,
    pub error_code: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishStatus {
    pub store_id: String,
    pub operation_id: String,
    pub service_id: String,
    pub state: String,
    pub error_code: Option<String>,
    pub binding: Option<BindingStatus>,
    pub request: crate::model_services::operations::OperationRequest,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Action {
    List {
        store_id: String,
    },
    Inspect {
        store_id: String,
        path: String,
    },
    Install {
        store_id: String,
        package_ref: String,
        digest: String,
        expected_signing_key: String,
        approve_publisher: bool,
    },
    State {
        store_id: String,
        package: PackageQuery,
        action: StateAction,
    },
    Bind {
        store_id: String,
        service_id: String,
        service_version_id: String,
        expected_binding_revision: Option<String>,
        package: PackageQuery,
        failure_mode: FailureMode,
    },
    Unbind {
        store_id: String,
        service_id: String,
        expected_binding_revision: String,
    },
    Publish {
        request: crate::model_services::operations::OperationRequest,
        expected_binding_revision: Option<String>,
        package: PackageQuery,
        failure_mode: FailureMode,
    },
    QueryPublish {
        store_id: String,
        operation_id: String,
    },
    ResumePublish {
        store_id: String,
        operation_id: String,
    },
    CancelPublish {
        store_id: String,
        operation_id: String,
    },
}
impl Action {
    pub fn store_id(&self) -> &str {
        match self {
            Self::List { store_id }
            | Self::Inspect { store_id, .. }
            | Self::Install { store_id, .. }
            | Self::State { store_id, .. }
            | Self::Bind { store_id, .. }
            | Self::Unbind { store_id, .. }
            | Self::QueryPublish { store_id, .. }
            | Self::ResumePublish { store_id, .. }
            | Self::CancelPublish { store_id, .. } => store_id,
            Self::Publish { request, .. } => &request.store_id,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Reply {
    Inventory {
        store_id: String,
        packages: Vec<PackageStatus>,
        bindings: Vec<BindingStatus>,
        scopes: Vec<ScopeStatus>,
        #[serde(default)]
        unavailable_service_ids: Vec<String>,
        publishes: Vec<PublishStatus>,
    },
    Review {
        review: Review,
    },
    Package {
        status: PackageStatus,
    },
    Binding {
        binding: BindingStatus,
    },
    Unbound {
        store_id: String,
        service_id: String,
        revision: String,
    },
    Publish {
        status: Box<PublishStatus>,
    },
}
pub fn schemas() -> serde_json::Value {
    serde_json::json!({
     "manifest":schemars::schema_for!(Manifest),"package-query":schemars::schema_for!(PackageQuery),
     "review":schemars::schema_for!(Review),"package-status":schemars::schema_for!(PackageStatus),
     "binding-status":schemars::schema_for!(BindingStatus),"publish-status":schemars::schema_for!(PublishStatus),
     "action":schemars::schema_for!(Action),"reply":schemars::schema_for!(Reply),
     "rank-invocation":schemars::schema_for!(RankInvocation),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn contracts_match_rust_and_policy_uses_the_common_manifest() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../contracts/plugins/routing");
        for (name, schema) in schemas().as_object().unwrap() {
            let checked: serde_json::Value = serde_json::from_slice(
                &std::fs::read(root.join(format!("{name}.schema.json"))).unwrap(),
            )
            .unwrap();
            assert_eq!(&checked, schema);
        }
        let manifest: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../sdk/plugins/examples/pure-policy/package/manifest.json"
        ))
        .unwrap();
        assert!(serde_json::from_value::<Manifest>(manifest.clone()).is_ok());
        assert!(serde_json::from_value::<crate::plugin::external::Manifest>(manifest).is_ok());
        let mut action = serde_json::json!({"kind":"list","storeId":"owned-test"});
        action["credential"] = "must never be accepted".into();
        assert!(serde_json::from_value::<Action>(action).is_err());
        let mut resume = serde_json::json!({"kind":"resume-publish","storeId":"owned-test","operationId":"original-id"});
        assert!(serde_json::from_value::<Action>(resume.clone()).is_ok());
        resume["request"] = serde_json::json!({"operationId":"replacement-id"});
        assert!(serde_json::from_value::<Action>(resume).is_err());
    }
}
