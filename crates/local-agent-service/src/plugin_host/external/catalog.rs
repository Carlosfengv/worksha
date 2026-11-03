//! Explicit catalog grants are separate from quota grants and all original native IPC.
pub(crate) mod api;
pub(crate) mod ownership;
mod records;
mod runtime;

use super::{Checked, Manager, Result, fail, package, store::Installed};
use local_agent_core::{Provider, domain::provider_connection::Selection};
use local_agent_core::{
    domain::provider_access as a,
    plugin::external::{Manifest, Permission},
};
use std::collections::BTreeMap;

/// Opaque authority retained by the host across directory, probe and publication.
#[derive(Clone)]
pub(crate) struct Lease(records::Record);
impl Lease {
    pub(crate) fn ensure(
        &self,
        manager: &Manager,
        providers: &[Provider],
        input: &Provider,
    ) -> Result<()> {
        manager.catalog_check(&self.0, providers)?;
        if self.0.input.endpoint != input.endpoint
            || self.0.input.protocol != input.protocol
            || self.0.input.vendor != input.vendor
            || self.0.input.credential_ref != input.credential_ref
        {
            return Err(fail("stale_snapshot"));
        }
        if let a::Target::Saved {
            provider_id,
            expected_revision,
        } = &self.0.target
            && (provider_id != &input.id || expected_revision != &input.revision.to_string())
        {
            return Err(fail("stale_snapshot"));
        }
        if providers
            .iter()
            .find(|p| p.id == input.id)
            .map_or(0, |p| p.revision)
            != input.revision
        {
            return Err(fail("stale_snapshot"));
        }
        Ok(())
    }
}
pub(crate) async fn read_for_connection(
    state: crate::AppState,
    selection: &Selection,
) -> Result<crate::provider_models::Catalog> {
    runtime::read_catalog(
        state,
        a::Query {
            plugin_id: selection.plugin_id.clone(),
            digest: selection.digest.clone(),
        },
        selection.binding_id.clone(),
        selection.target.clone(),
    )
    .await
}
pub(crate) fn scope_changed() -> super::Error {
    fail("stale_snapshot")
}
pub(crate) async fn read_for_connection_checked<C, F>(
    state: crate::AppState,
    selection: &Selection,
    guard: C,
) -> Result<crate::provider_models::Catalog>
where
    C: Fn() -> F,
    F: std::future::Future<Output = Result<()>>,
{
    runtime::read_catalog_checked(
        state,
        a::Query {
            plugin_id: selection.plugin_id.clone(),
            digest: selection.digest.clone(),
        },
        selection.binding_id.clone(),
        selection.target.clone(),
        guard,
    )
    .await
}

pub(crate) fn opted_in(m: &Manifest) -> bool {
    m.capabilities.iter().any(|c| c == a::CAPABILITY)
}
pub(crate) fn validate(m: &Manifest, files: &BTreeMap<String, Vec<u8>>) -> Result<a::Policy> {
    if !opted_in(m) {
        return Err(fail("unsupported"));
    }
    let bytes = files
        .get("provider-catalog.json")
        .ok_or(fail("invalid_input"))?;
    if bytes.len() > 4096 {
        return Err(fail("resource_limit"));
    }
    let policy: a::Policy = serde_json::from_slice(bytes).checked("invalid_input")?;
    if !policy.valid()
        || !m.requires.iter().any(|r| r == a::BROKER)
        || m.capabilities.iter().any(|c| {
            ![a::CAPABILITY, "provider.catalog@1", "ui.contribution@1"].contains(&c.as_str())
        })
        || m.permissions
            .iter()
            .filter(|p| matches!(p, Permission::ConnectionRequest { .. }))
            .count()
            != 1
        || !m
            .permissions
            .iter()
            .any(|p| matches!(p,Permission::ConnectionRequest{path} if path==&policy.path))
        || m.permissions.iter().any(|p| {
            !matches!(
                p,
                Permission::ConnectionRequest { .. } | Permission::UiContribute
            )
        })
        || m.requires.iter().any(|r| {
            ![
                a::BROKER,
                local_agent_core::plugin::contributions::OPTIONAL,
                local_agent_core::plugin::contributions::DETAILS,
            ]
            .contains(&r.as_str())
        })
    {
        return Err(fail("permission_denied"));
    }
    Ok(policy)
}
impl Manager {
    pub(crate) fn catalog_policy(&self, plugin_id: &str, digest: &str) -> Result<a::Policy> {
        self.catalog_plugin(&a::Query {
            plugin_id: plugin_id.into(),
            digest: digest.into(),
        })
        .map(|(_, policy)| policy)
    }
    pub(crate) fn catalog_connection_lease(
        &self,
        selection: &Selection,
        providers: &[Provider],
        input: &Provider,
    ) -> Result<Lease> {
        let q = a::Query {
            plugin_id: selection.plugin_id.clone(),
            digest: selection.digest.clone(),
        };
        let lease =
            Lease(self.catalog_record(&q, &selection.binding_id, &selection.target, providers)?);
        lease.ensure(self, providers, input)?;
        Ok(lease)
    }
    pub(super) fn catalog_plugin(&self, q: &a::Query) -> Result<(Installed, a::Policy)> {
        self.ensure_running()?;
        let p = self.verified(&q.plugin_id, &q.digest)?;
        let selected = self.selection(&q.plugin_id)?;
        if !selected.enabled || selected.digest != q.digest || selected.status != "active" {
            return Err(fail("plugin_disabled"));
        }
        self.dependencies(&p)?;
        let checked = package::inspect(
            &package::read_file(&p.path, package::MAX_ARCHIVE)?,
            self.development,
        )?;
        let policy = validate(&p.manifest, &checked.files)?;
        Ok((p, policy))
    }
}
