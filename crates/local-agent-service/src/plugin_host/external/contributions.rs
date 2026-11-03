use super::{Checked, Manager, Result, fail, package, store::Installed};
use local_agent_core::plugin::{contributions::*, external::*};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn opted_in(m: &Manifest) -> bool {
    m.requires.iter().any(|r| r == OPTIONAL || r == DETAILS)
}
pub(super) fn parse(
    m: &Manifest,
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<Option<Contributions>> {
    let bytes = files.get("contributions.json");
    if !opted_in(m) {
        // Previously ignored payload files remain compatible, but never register a contribution.
        return Ok(None);
    }
    let bytes = bytes.ok_or(fail("invalid_input"))?;
    if bytes.len() > 16384 {
        return Err(fail("resource_limit"));
    }
    let c: Contributions = serde_json::from_slice(bytes).checked("invalid_input")?;
    if c.schema_version != 1 || c.optional_dependencies.len() > 16 || c.detail_slots.len() > 16 {
        return Err(fail("invalid_input"));
    }
    if (!c.optional_dependencies.is_empty() && !m.requires.iter().any(|r| r == OPTIONAL))
        || (!c.detail_slots.is_empty()
            && (!m.requires.iter().any(|r| r == DETAILS)
                || !m.permissions.contains(&Permission::UiContribute)))
    {
        return Err(fail("permission_denied"));
    }
    let views: BTreeSet<_> = m.views.iter().map(|v| v.id.as_str()).collect();
    let mut ids = BTreeSet::new();
    for d in &c.optional_dependencies {
        let mut used = BTreeSet::new();
        if !package::valid_id(&d.plugin_id)
            || d.plugin_id == m.id
            || !ids.insert(&d.plugin_id)
            || m.requires
                .iter()
                .any(|r| r == &format!("plugin:{}", d.plugin_id))
            || d.version.len() > 120
            || semver::VersionReq::parse(&d.version).is_err()
            || d.views.is_empty()
            || d.views.len() > 8
            || d.views
                .iter()
                .any(|v| !views.contains(v.as_str()) || !used.insert(v))
        {
            return Err(fail("invalid_input"));
        }
    }
    ids.clear();
    for slot in &c.detail_slots {
        let mut objects = BTreeSet::new();
        if !package::local_id(&slot.id)
            || !ids.insert(&slot.id)
            || !views.contains(slot.view_id.as_str())
            || slot.title.is_empty()
            || slot.title.len() > 100
            || slot.title.chars().any(char::is_control)
            || slot.object_ids.is_empty()
            || slot.object_ids.len() > 32
            || (slot.object_ids.contains(&"*".into()) && slot.object_ids.len() != 1)
            || slot.object_ids.iter().any(|id| {
                id.is_empty()
                    || id.len() > 160
                    || !objects.insert(id)
                    || (id != "*"
                        && !id
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b)))
            })
        {
            return Err(fail("invalid_input"));
        }
    }
    Ok(Some(c))
}

impl Manager {
    pub(super) fn contributions_for(&self, p: &Installed) -> Result<Option<Contributions>> {
        if !opted_in(&p.manifest) {
            return Ok(None);
        }
        let bytes = package::read_file(&p.path, package::MAX_ARCHIVE)?;
        if package::hash(&bytes) != p.digest {
            return Err(fail("signature_invalid"));
        }
        let checked = package::inspect(&bytes, self.development)?;
        parse(&checked.manifest, &checked.files)
    }
    pub(super) fn dependency_statuses(
        &self,
        c: &Contributions,
        items: &[Status],
    ) -> Vec<DependencyStatus> {
        c.optional_dependencies
            .iter()
            .map(|d| {
                let selected = items
                    .iter()
                    .find(|p| p.manifest.id == d.plugin_id && p.enabled);
                let reason = match selected {
                    None => Some("not-enabled"),
                    Some(p) if p.runtime_status != "active" => Some("unavailable"),
                    Some(p)
                        if !semver::VersionReq::parse(&d.version).is_ok_and(|r| {
                            semver::Version::parse(&p.manifest.version).is_ok_and(|v| r.matches(&v))
                        }) =>
                    {
                        Some("version-mismatch")
                    }
                    Some(p) if self.verified(&p.manifest.id, &p.digest).is_err() => {
                        Some("unavailable")
                    }
                    Some(_) => None,
                };
                DependencyStatus {
                    plugin_id: d.plugin_id.clone(),
                    version: d.version.clone(),
                    views: d.views.clone(),
                    available: reason.is_none(),
                    selected_version: selected.map(|p| p.manifest.version.clone()),
                    reason: reason.map(str::to_owned),
                }
            })
            .collect()
    }
    pub(crate) fn contribution_catalog(&self, items: &[Status]) -> ContributionCatalog {
        let records = items
            .iter()
            .filter(|s| opted_in(&s.manifest))
            .map(|s| {
                let result = self
                    .installed(&s.manifest.id, &s.digest)
                    .and_then(|p| self.contributions_for(&p));
                match result {
                    Ok(Some(c)) => ContributionStatus {
                        plugin_id: s.manifest.id.clone(),
                        digest: s.digest.clone(),
                        dependencies: self.dependency_statuses(&c, items),
                        slots: c.detail_slots,
                        error_code: None,
                    },
                    Ok(None) => ContributionStatus {
                        plugin_id: s.manifest.id.clone(),
                        digest: s.digest.clone(),
                        dependencies: vec![],
                        slots: vec![],
                        error_code: Some("unsupported".into()),
                    },
                    Err(e) => ContributionStatus {
                        plugin_id: s.manifest.id.clone(),
                        digest: s.digest.clone(),
                        dependencies: vec![],
                        slots: vec![],
                        error_code: Some(e.code.into()),
                    },
                }
            })
            .collect();
        ContributionCatalog {
            schema_version: 1,
            items: records,
        }
    }
    pub(super) fn check_optional_view(&self, p: &Installed, view: &str) -> Result<()> {
        if let Some(c) = self.contributions_for(p)? {
            let statuses = self.dependency_statuses(&c, &self.list()?);
            if statuses
                .iter()
                .any(|d| !d.available && d.views.iter().any(|v| v == view))
            {
                return Err(fail("dependency_missing"));
            }
        }
        Ok(())
    }
}
