//! Legacy signed manifests supply static advice, not grants or Provider writes.
use super::{Checked, Manager, Result, fail, package};
use local_agent_core::{
    domain::provider_presets::{
        PresetAdapter, PresetAdvice, PresetIssue, PresetOrigin, checked_presets,
    },
    plugin::external::Manifest,
    providers::ConfigError,
};
use std::collections::BTreeMap;

struct ExternalPresets<'a> {
    manifest: &'a Manifest,
    digest: &'a str,
}
impl PresetAdapter for ExternalPresets<'_> {
    fn presets(&self) -> std::result::Result<Vec<PresetAdvice>, ConfigError> {
        if self.manifest.providers.len() > 16 {
            return Err(ConfigError::new("capacity", "预设数量超过原包预算"));
        }
        Ok(self
            .manifest
            .providers
            .iter()
            .map(|p| PresetAdvice {
                origin: PresetOrigin::External {
                    plugin_id: self.manifest.id.clone(),
                    digest: self.digest.into(),
                    preset_id: p.id.clone(),
                },
                source_name: self.manifest.display_name.clone(),
                name: p.name.clone(),
                protocol: p.protocol.clone(),
                endpoints: BTreeMap::from([(p.protocol.clone(), p.endpoint.clone())]),
                model: p.model.clone(),
                documents: None,
            })
            .collect())
    }
}

/// Also used by the existing CreateProvider facade. Its stable preset mapping,
/// Store mutation and response remain owned by that original caller.
pub(super) fn legacy_advice(
    manifest: &Manifest,
    digest: &str,
    preset: &str,
) -> Result<PresetAdvice> {
    ExternalPresets { manifest, digest }.presets().checked("invalid_input")?
        .into_iter().find(|p| matches!(&p.origin, PresetOrigin::External { preset_id, .. } if preset_id == preset))
        .ok_or(fail("not_found"))
}

impl Manager {
    /// Bound to the existing 64-package / 16-presets limits. The list is only
    /// candidates; selection rechecks the actual signed package separately.
    pub(crate) fn preset_inventory(&self) -> (Vec<PresetAdvice>, Vec<PresetIssue>) {
        let rows = (|| -> Result<Vec<(String, String, String)>> {
            let db = self.db.lock().checked("unavailable")?;
            let mut stmt = db.prepare("SELECT plugin_id,digest,status FROM selections WHERE enabled=1 ORDER BY plugin_id LIMIT 65").checked("unavailable")?;
            let rows = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .checked("unavailable")?
                .collect::<std::result::Result<Vec<_>, _>>()
                .checked("unavailable")?;
            if rows.len() > 64 {
                return Err(fail("resource_limit"));
            }
            Ok(rows)
        })();
        let mut items = vec![];
        let mut issues = vec![];
        let rows = match rows {
            Ok(rows) => rows,
            Err(e) => {
                issues.push(PresetIssue {
                    plugin_id: None,
                    digest: None,
                    error_code: e.code.into(),
                });
                return (items, issues);
            }
        };
        for (id, digest, status) in rows {
            let batch = (|| -> Result<Vec<PresetAdvice>> {
                let installed = self.installed(&id, &digest)?;
                if installed.manifest.id != id {
                    return Err(fail("invalid_record"));
                }
                if installed.manifest.providers.is_empty() {
                    return Ok(vec![]);
                }
                if status != "active" {
                    return Err(fail("plugin_disabled"));
                }
                if !installed
                    .manifest
                    .capabilities
                    .iter()
                    .any(|c| c == "provider.catalog@1")
                {
                    return Err(fail("invalid_record"));
                }
                checked_presets(&ExternalPresets {
                    manifest: &installed.manifest,
                    digest: &digest,
                })
                .checked("invalid_record")
            })();
            match batch {
                Ok(batch) => items.extend(batch),
                Err(e) => issues.push(PresetIssue {
                    plugin_id: Some(id),
                    digest: Some(digest),
                    error_code: e.code.into(),
                }),
            }
        }
        (items, issues)
    }

    pub(crate) fn resolve_preset(&self, origin: &PresetOrigin) -> Result<PresetAdvice> {
        let PresetOrigin::External {
            plugin_id,
            digest,
            preset_id,
        } = origin
        else {
            return Err(fail("invalid_input"));
        };
        let before = self.selection(plugin_id)?;
        if !before.enabled || before.status != "active" || before.digest != *digest {
            return Err(fail("plugin_disabled"));
        }
        let installed = self.verified(plugin_id, digest)?;
        // Resolve from the signed bytes, not solely from the cached SQL manifest.
        // Existing verified() behavior is kept for every original caller.
        let bytes = package::read_file(&installed.path, package::MAX_ARCHIVE)?;
        let signed = package::inspect(&bytes, self.development)?;
        if signed.digest != *digest || signed.manifest.id != *plugin_id {
            return Err(fail("signature_invalid"));
        }
        if !signed
            .manifest
            .capabilities
            .iter()
            .any(|c| c == "provider.catalog@1")
        {
            return Err(fail("capability_conflict"));
        }
        let advice = legacy_advice(&signed.manifest, digest, preset_id)?;
        advice.validate().checked("invalid_input")?;
        let after = self.selection(plugin_id)?;
        if !after.enabled
            || after.status != "active"
            || after.digest != before.digest
            || after.epoch != before.epoch
            || after.revision != before.revision
        {
            return Err(fail("stale_snapshot"));
        }
        Ok(advice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_host::external::{
        StateAction, StateRequest,
        tests::{install, manifest, new_host},
    };
    fn fixture_manifest(id: &str) -> Manifest {
        let mut m = manifest();
        m.id = id.into();
        m.runtime = super::super::Runtime::Declarative;
        m.capabilities = vec!["provider.catalog@1".into()];
        m.permissions.clear();
        m.requires.clear();
        m.settings.clear();
        m.views.clear();
        m.providers = vec![local_agent_core::plugin::external::ProviderPreset {
            id: "openai".into(),
            name: "External initial values".into(),
            protocol: "responses".into(),
            endpoint: "http://127.0.0.1:9/common".into(),
            model: "starter".into(),
        }];
        m
    }
    async fn enabled(
        host: &std::sync::Arc<Manager>,
        root: &std::path::Path,
        manifest: &Manifest,
    ) -> super::super::Status {
        let p = install(host, root, manifest);
        host.change(StateRequest {
            plugin_id: p.manifest.id,
            digest: p.digest,
            action: StateAction::Enable,
        })
        .await
        .unwrap()
    }
    #[tokio::test]
    async fn listing_and_resolving_advice_never_allocate_a_provider_mapping_or_permission() {
        let root = tempfile::tempdir().unwrap();
        let host = new_host(root.path());
        let s = enabled(
            &host,
            root.path(),
            &fixture_manifest("org.example.presets-a"),
        )
        .await;
        let changes = host.db.lock().unwrap().total_changes();
        let (items, issues) = host.preset_inventory();
        assert_eq!(items.len(), 1);
        assert!(issues.is_empty());
        let resolved = host.resolve_preset(&items[0].origin).unwrap();
        assert_eq!(resolved, items[0]);
        assert_eq!(
            resolved.origin,
            PresetOrigin::External {
                plugin_id: s.manifest.id,
                digest: s.digest,
                preset_id: "openai".into()
            }
        );
        let db = host.db.lock().unwrap();
        assert_eq!(db.total_changes(), changes);
        assert_eq!(
            db.query_row("SELECT count(*) FROM provider_presets", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert!(!host.root.join("provider-catalog").exists());
        assert!(!host.root.join("draft-catalog-owners").exists());
    }
    #[tokio::test]
    async fn resolution_uses_signed_values_and_refuses_an_unavailable_or_replaced_package() {
        let root = tempfile::tempdir().unwrap();
        let host = new_host(root.path());
        let m = fixture_manifest("org.example.presets-b");
        let s = enabled(&host, root.path(), &m).await;
        let origin = host.preset_inventory().0[0].origin.clone();
        let mut cached = m.clone();
        cached.providers[0].endpoint = "https://wrong.invalid".into();
        host.db
            .lock()
            .unwrap()
            .execute(
                "UPDATE packages SET manifest=?1 WHERE digest=?2",
                rusqlite::params![serde_json::to_string(&cached).unwrap(), s.digest],
            )
            .unwrap();
        assert_eq!(
            host.resolve_preset(&origin).unwrap().endpoints["responses"],
            m.providers[0].endpoint
        );
        let file = host.installed(&m.id, &s.digest).unwrap().path;
        let bytes = std::fs::read(&file).unwrap();
        std::fs::write(&file, b"changed").unwrap();
        assert!(host.resolve_preset(&origin).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b"changed");
        std::fs::write(file, bytes).unwrap();
        host.change(StateRequest {
            plugin_id: m.id,
            digest: s.digest,
            action: StateAction::Disable,
        })
        .await
        .unwrap();
        assert!(host.preset_inventory().0.is_empty());
        assert_eq!(
            host.resolve_preset(&origin).unwrap_err().code,
            "plugin_disabled"
        );
    }
    #[tokio::test]
    async fn a_damaged_source_keeps_other_origins_and_builtin_catalog_available() {
        let root = tempfile::tempdir().unwrap();
        let host = new_host(root.path());
        let a = enabled(
            &host,
            root.path(),
            &fixture_manifest("org.example.presets-c"),
        )
        .await;
        let b = enabled(
            &host,
            root.path(),
            &fixture_manifest("org.example.presets-d"),
        )
        .await;
        host.db
            .lock()
            .unwrap()
            .execute(
                "UPDATE packages SET manifest='{}' WHERE digest=?1",
                [a.digest],
            )
            .unwrap();
        let (items, issues) = host.preset_inventory();
        assert_eq!(items.len(), 1);
        assert_eq!(issues.len(), 1);
        assert_eq!(
            issues[0].plugin_id.as_deref(),
            Some("org.example.presets-c")
        );
        assert!(
            matches!(&items[0].origin,PresetOrigin::External{plugin_id,..} if plugin_id==&b.manifest.id)
        );
        assert_eq!(
            checked_presets(&local_agent_core::domain::provider_presets::BuiltinPresets)
                .unwrap()
                .len(),
            8
        );
    }
    #[tokio::test]
    async fn restart_retains_static_advice_without_allocating_connection_authority() {
        let root = tempfile::tempdir().unwrap();
        let host = new_host(root.path());
        enabled(
            &host,
            root.path(),
            &fixture_manifest("org.example.presets-e"),
        )
        .await;
        let advice = host.preset_inventory().0[0].clone();
        drop(host);
        let reopened = new_host(root.path());
        assert_eq!(reopened.preset_inventory().0, vec![advice.clone()]);
        assert_eq!(reopened.resolve_preset(&advice.origin).unwrap(), advice);
        let PresetOrigin::External {
            plugin_id, digest, ..
        } = &advice.origin
        else {
            panic!("external fixture")
        };
        let status = reopened.status(plugin_id, digest).unwrap();
        assert!(
            !status.source_authorized && !status.connection_authorized && !status.report_authorized
        );
        assert!(!reopened.root.join("provider-catalog").exists());
        assert_eq!(
            reopened
                .db
                .lock()
                .unwrap()
                .query_row("SELECT count(*) FROM provider_presets", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}
