//! Signed non-content declarations, scoped to the host's object-summary broker.
use super::{Checked, Result, fail};
use local_agent_core::plugin::{external::*, object_contributions as c};
use std::collections::{BTreeMap, BTreeSet};
pub(super) mod api;
mod broker;
mod refs;
mod store;
pub(super) use broker::Host;

pub(super) fn validate_manifest(manifest: &Manifest) -> Result<()> {
    // The initial object contract is declaration-only. It cannot borrow source,
    // network, configuration-plan, gateway-report or arbitrary guest authority.
    if !manifest.requires.iter().any(|r| r == c::FEATURE)
        || !matches!(manifest.runtime, Runtime::Declarative)
        || manifest.capabilities != ["ui.contribution@1"]
        || manifest.permissions.len() != 2
        || !manifest.permissions.contains(&Permission::UiContribute)
        || !manifest
            .permissions
            .iter()
            .any(|p| matches!(p,Permission::DomainQuery{query} if query==c::QUERY_PERMISSION))
        || manifest.agent.is_some()
        || !manifest.providers.is_empty()
        || !manifest.settings.is_empty()
        || manifest
            .views
            .iter()
            .any(|view| view.kind != ViewKind::Report)
        || manifest.requires.iter().any(|r| {
            r != c::FEATURE
                && r != local_agent_core::plugin::contributions::OPTIONAL
                && !r
                    .strip_prefix("plugin:")
                    .is_some_and(super::package::valid_id)
        })
    {
        return Err(fail("permission_denied"));
    }
    Ok(())
}
pub(super) fn parse(
    manifest: &Manifest,
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<Option<c::ObjectContributions>> {
    if !manifest.requires.iter().any(|r| r == c::FEATURE) {
        // Old packages may contain previously ignored signed payload files.
        return Ok(None);
    }
    validate_manifest(manifest)?;
    let bytes = files
        .get(c::DECLARATION_FILE)
        .ok_or(fail("invalid_input"))?;
    if bytes.len() > 16_384 {
        return Err(fail("resource_limit"));
    }
    let declared: c::ObjectContributions =
        serde_json::from_slice(bytes).checked("invalid_input")?;
    declared.validate().checked("invalid_input")?;
    let views: BTreeSet<_> = manifest.views.iter().map(|v| v.id.as_str()).collect();
    let used: BTreeSet<_> = declared
        .detail_slots
        .iter()
        .map(|s| s.view_id.as_str())
        .collect();
    if manifest.views.is_empty()
        || manifest.views.len() > 8
        || views.len() != manifest.views.len()
        || views != used
        || manifest.views.iter().any(|v| v.kind != ViewKind::Report)
    {
        return Err(fail("invalid_input"));
    }
    Ok(Some(declared))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn manifest() -> Manifest {
        serde_json::from_value(serde_json::json!({
            "manifestVersion":1,"id":"org.example.object-details","publisher":"org.example",
            "displayName":"对象摘要","version":"1.0.0","hostApi":"^1.0",
            "runtime":{"kind":"declarative"},"capabilities":["ui.contribution@1"],
            "requires":[c::FEATURE],"permissions":[{"kind":"ui.contribute"},{"kind":"domain.query","query":c::QUERY_PERMISSION}],
            "agent":null,"views":[{"id":"project-summary","title":"项目摘要","description":"显示当前项目已有记录的数量","kind":"report"}]
        })).unwrap()
    }
    fn files() -> BTreeMap<String, Vec<u8>> {
        BTreeMap::from([(c::DECLARATION_FILE.into(),serde_json::to_vec(&serde_json::json!({"schemaVersion":1,"detailSlots":[{"id":"project-usage","target":"project","placement":"card","viewId":"project-summary","title":"项目摘要","queryKind":"summary","fields":["session-count"],"purpose":"在项目详情中显示已有会话数量"}]})).unwrap())])
    }
    #[test]
    fn object_sdk_example_declares_all_targets_within_existing_view_and_payload_limits() {
        let manifest: Manifest = serde_json::from_str(include_str!(
            "../../../../../sdk/plugins/examples/object-summary/package/manifest.json"
        ))
        .unwrap();
        let files = BTreeMap::from([
            (c::DECLARATION_FILE.into(), include_bytes!("../../../../../sdk/plugins/examples/object-summary/package/object-contributions.json").to_vec()),
            ("contributions.json".into(), include_bytes!("../../../../../sdk/plugins/examples/object-summary/package/contributions.json").to_vec()),
        ]);
        let declared = parse(&manifest, &files).unwrap().unwrap();
        assert_eq!(declared.detail_slots.len(), 13);
        assert_eq!(manifest.views.len(), 7);
        for target in [
            "agent", "provider", "account", "project", "session", "request",
        ] {
            let target: local_agent_core::domain::object_summary::ObjectTarget =
                serde_json::from_value(serde_json::json!(target)).unwrap();
            assert!(
                declared
                    .detail_slots
                    .iter()
                    .any(|slot| slot.target == target)
            );
        }
        let dependencies = super::super::contributions::parse(&manifest, &files)
            .unwrap()
            .unwrap();
        assert!(dependencies.detail_slots.is_empty());
        assert_eq!(dependencies.optional_dependencies[0].views, ["enhanced"]);
        super::super::package::validate(&manifest).unwrap();
    }
    #[test]
    fn explicit_declaration_is_signed_payload_and_admission_requires_optin() {
        let mut m = manifest();
        let declared = parse(&m, &files()).unwrap().unwrap();
        assert_eq!(
            declared.detail_slots[0].target,
            local_agent_core::domain::object_summary::ObjectTarget::Project
        );
        super::super::package::validate(&m).unwrap();
        m.requires.clear();
        assert_eq!(
            super::super::package::validate(&m).unwrap_err().code,
            "unsupported"
        );
        assert!(parse(&m, &files()).unwrap().is_none());
        let garbage = BTreeMap::from([(c::DECLARATION_FILE.into(), b"not a declaration".to_vec())]);
        assert!(parse(&m, &garbage).unwrap().is_none());
    }
    #[test]
    fn object_package_cannot_gain_network_source_guest_or_unrelated_view_authority() {
        let mut m = manifest();
        m.permissions.push(Permission::SourceContentRead {
            resource: "history".into(),
        });
        assert_eq!(parse(&m, &files()).unwrap_err().code, "permission_denied");
        let mut m = manifest();
        m.runtime = Runtime::WasmComponent {
            entry: "plugin.wasm".into(),
            abi: "local-agent:plugin@1.0.0".into(),
        };
        assert_eq!(parse(&m, &files()).unwrap_err().code, "permission_denied");
        let mut m = manifest();
        m.views[0].id = "other-view".into();
        assert_eq!(parse(&m, &files()).unwrap_err().code, "invalid_input");
        let mut m = manifest();
        m.permissions[1] = Permission::DomainQuery {
            query: "gateway.summary".into(),
        };
        assert_eq!(parse(&m, &files()).unwrap_err().code, "permission_denied");
    }
    #[test]
    fn malformed_oversized_and_content_declarations_never_become_metadata_reads() {
        let m = manifest();
        assert_eq!(
            parse(&m, &BTreeMap::new()).unwrap_err().code,
            "invalid_input"
        );
        let big = BTreeMap::from([(c::DECLARATION_FILE.into(), vec![b' '; 16_385])]);
        assert_eq!(parse(&m, &big).unwrap_err().code, "resource_limit");
        let mut f = files();
        let mut raw: serde_json::Value =
            serde_json::from_slice(f.get(c::DECLARATION_FILE).unwrap()).unwrap();
        raw["detailSlots"][0]["fields"] = serde_json::json!(["conversation"]);
        f.insert(
            c::DECLARATION_FILE.into(),
            serde_json::to_vec(&raw).unwrap(),
        );
        assert_eq!(parse(&m, &f).unwrap_err().code, "invalid_input");
    }
}
