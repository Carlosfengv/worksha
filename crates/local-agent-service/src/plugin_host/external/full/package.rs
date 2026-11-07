use super::super::{Result, fail, package};
#[cfg(test)]
use ed25519_dalek::SigningKey;
use local_agent_core::plugin::{
    external::*,
    full::{AGENT_CONNECTION, AGENT_CONNECTION_FEATURE, Capability, FACT_FEATURE, SOURCE_BROKER},
};
use std::collections::BTreeSet;
#[cfg(test)]
use std::path::Path;

pub(crate) fn validate(m: &Manifest) -> Result<()> {
    package::validate_header(m)?;
    let source = m.source.as_ref().ok_or(fail("unsupported"))?;
    let Runtime::WasmComponent { entry, abi } = &m.runtime else {
        return Err(fail("permission_denied"));
    };
    let connection = m.has_agent_connection();
    let mut permissions = vec![Permission::SourceContentRead {
        resource: "history".into(),
    }];
    if connection {
        permissions.push(Permission::ConnectionRequest {
            path: "agent-connection".into(),
        });
    }
    if m.kind.is_some()
        || m.agent.is_some()
        || !m.providers.is_empty()
        || !m.settings.is_empty()
        || !m.views.is_empty()
        || m.id.starts_with("io.localagent.")
        || m.publisher.starts_with("io.localagent.")
        || entry != "plugin.wasm"
        || abi != "local-agent:plugin@1.0.0"
        || m.permissions != permissions
        || m.requires.len()
            != 2 + usize::from(source.refresh_seconds.is_some()) + usize::from(connection)
        || (connection
            && (!m.requires.iter().any(|r| r == AGENT_CONNECTION_FEATURE)
                || source.products.len() != 1))
        || source
            .refresh_seconds
            .is_some_and(|seconds| !(5..=300).contains(&seconds))
        || source.max_age_seconds.is_some_and(|age| {
            source
                .refresh_seconds
                .is_none_or(|seconds| age < seconds.saturating_mul(2) || age > 900)
        })
        || (source.refresh_seconds.is_some()
            && !m.requires.iter().any(|r| r == "host.source-watch@1"))
        || !m.requires.iter().any(|r| r == SOURCE_BROKER)
        || !m.requires.iter().any(|r| r == FACT_FEATURE)
    {
        return Err(fail("permission_denied"));
    }
    let label = |s: &str| !s.is_empty() && s.len() <= 100 && !s.chars().any(char::is_control);
    if !m
        .capabilities
        .iter()
        .any(|c| c == Capability::AgentDirectory.as_str())
        || !m
            .capabilities
            .iter()
            .any(|c| c == Capability::SessionSource.as_str())
        || m.capabilities
            .iter()
            .any(|c| Capability::parse(c).is_none() && c != AGENT_CONNECTION)
        || m.capabilities.len() > 6 + usize::from(connection)
        || m.capabilities.iter().collect::<BTreeSet<_>>().len() != m.capabilities.len()
        || !package::local_id(&source.id)
        || source.products.is_empty()
        || source.products.len() > 16
        || source
            .products
            .iter()
            .any(|p| !package::local_id(&p.id) || !label(&p.display_name))
        || source
            .products
            .iter()
            .map(|p| &p.id)
            .collect::<BTreeSet<_>>()
            .len()
            != source.products.len()
    {
        return Err(fail("unsupported"));
    }
    Ok(())
}

pub(crate) use package::SignedPackage as Package;
#[cfg(test)]
pub(crate) use package::signing_bytes;
pub(crate) fn inspect(bytes: &[u8]) -> Result<Package> {
    let checked = package::inspect(bytes, false)?;
    validate(&checked.manifest)?;
    checked.require_signature()
}
#[cfg(test)]
pub(crate) fn pack(root: &Path, key: &SigningKey) -> Result<Vec<u8>> {
    super::super::sdk::pack(root, Some(key))
}
#[cfg(test)]
mod tests;
