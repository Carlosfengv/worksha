use super::super::{Result, fail, package};
use local_agent_core::plugin::{
    external::*,
    policy::{CAPABILITY, FEATURE, PackageKind},
};

pub(crate) fn validate(m: &Manifest) -> Result<()> {
    package::validate_header(m)?;
    let Runtime::WasmComponent { entry, abi } = &m.runtime else {
        return Err(fail("permission_denied"));
    };
    if m.kind != Some(PackageKind::RoutingPolicy)
        || m.source.is_some()
        || m.agent.is_some()
        || !m.providers.is_empty()
        || !m.settings.is_empty()
        || !m.views.is_empty()
        || m.id.starts_with("io.localagent.")
        || m.publisher.starts_with("io.localagent.")
        || m.capabilities != [CAPABILITY]
        || m.requires != [FEATURE]
        || !m.permissions.is_empty()
        || entry != "plugin.wasm"
        || abi != "local-agent:plugin@1.0.0"
    {
        return Err(fail("permission_denied"));
    }
    Ok(())
}
pub(crate) use package::SignedPackage as Package;
pub(crate) fn inspect(bytes: &[u8]) -> Result<Package> {
    let checked = package::inspect(bytes, false)?;
    validate(&checked.manifest)?;
    checked.require_signature()
}
