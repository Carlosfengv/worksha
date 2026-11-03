//! Metadata broker. Neither grant checks nor catalog reads run guests.
use super::{refs, store};
use crate::plugin_host::external::{Checked, Manager, Result, fail, package, store::Installed};
use local_agent_core::{
    domain::{self as d, object_summary::ObjectSummarySource},
    plugin::{external as p, object_contributions as c},
};
use std::path::Path;

pub(in crate::plugin_host::external) struct Host {
    grants: store::Store,
    references: refs::Registry,
}
impl Host {
    pub(in crate::plugin_host::external) fn new(data: &Path) -> Self {
        Self {
            grants: store::Store::new(data),
            references: refs::Registry::default(),
        }
    }
}
struct Definition {
    parent: p::Status,
    declaration: c::ObjectSlot,
    owner: store::Owner,
    installed: Installed,
}
fn owner(
    slot: c::ObjectSlotQuery,
    declaration: &c::ObjectSlot,
    parent: &p::Status,
) -> Result<store::Owner> {
    Ok(store::Owner {
        slot,
        descriptor_identity: package::hash(
            &serde_json::to_vec(declaration).checked("invalid_record")?,
        ),
        activation_epoch: parent
            .activation_epoch
            .parse::<u64>()
            .checked("invalid_record")?
            .into(),
        grant_revision: parent
            .grant_revision
            .parse::<u64>()
            .checked("invalid_record")?
            .into(),
    })
}
fn active(parent: &p::Status) -> Result<()> {
    if !parent.enabled {
        return Err(fail("plugin_disabled"));
    }
    if parent.runtime_status != "active" || parent.error_code.is_some() {
        return Err(fail("grant_revoked"));
    }
    Ok(())
}
fn domain_failure(error: d::Failure) -> crate::plugin_host::external::Error {
    fail(match error.code {
        d::ErrorCode::InvalidInput => "invalid_input",
        d::ErrorCode::NotFound => "not_found",
        d::ErrorCode::GrantRevoked => "grant_revoked",
        d::ErrorCode::StaleSnapshot => "stale_snapshot",
        d::ErrorCode::ResourceLimit => "resource_limit",
        d::ErrorCode::Unsupported => "unsupported",
        _ => "unavailable",
    })
}
impl Manager {
    fn object_context(
        &self,
        installed: &Installed,
        view: &str,
        grant_context: String,
        items: &[p::Status],
    ) -> Result<(String, bool)> {
        let Some(contributions) = self.contributions_for(installed)? else {
            return Ok((grant_context, false));
        };
        let dependencies: Vec<_> = self
            .dependency_statuses(&contributions, items)
            .into_iter()
            .filter(|dependency| dependency.views.iter().any(|id| id == view))
            .collect();
        if dependencies.is_empty() {
            return Ok((grant_context, false));
        }
        let missing = dependencies.iter().any(|dependency| !dependency.available);
        // Only affected views bind dependency epochs. Stop/start or replacement
        // cannot revive their old references, and base slots keep their context.
        let epochs: Vec<_> = dependencies
            .iter()
            .map(|dependency| {
                items
                    .iter()
                    .find(|item| item.manifest.id == dependency.plugin_id && item.enabled)
                    .map(|item| {
                        (
                            &item.manifest.id,
                            &item.digest,
                            &item.activation_epoch,
                            &item.grant_revision,
                        )
                    })
            })
            .collect();
        Ok((
            package::hash(
                &serde_json::to_vec(&serde_json::json!([grant_context, dependencies, epochs]))
                    .checked("invalid_record")?,
            ),
            missing,
        ))
    }
    fn object_package(
        &self,
        id: &str,
        digest: &str,
    ) -> Result<(Installed, c::ObjectContributions)> {
        let installed = self.verified(id, digest)?;
        let bytes = package::read_file(&installed.path, package::MAX_ARCHIVE)?;
        if package::hash(&bytes) != digest {
            return Err(fail("signature_invalid"));
        }
        let checked = package::inspect(&bytes, self.development)?;
        let declaration =
            super::parse(&checked.manifest, &checked.files)?.ok_or(fail("unsupported"))?;
        Ok((installed, declaration))
    }
    fn object_definition(&self, slot: &c::ObjectSlotQuery) -> Result<Definition> {
        slot.validate().checked("invalid_input")?;
        let (installed, declared) = self.object_package(&slot.plugin_id, &slot.digest)?;
        if self.selection(&slot.plugin_id)?.digest != slot.digest {
            return Err(fail("grant_revoked"));
        }
        let parent = self.status(&slot.plugin_id, &slot.digest)?;
        let declaration = declared
            .detail_slots
            .into_iter()
            .find(|s| s.id == slot.slot_id)
            .ok_or(fail("not_found"))?;
        let owner = owner(slot.clone(), &declaration, &parent)?;
        Ok(Definition {
            parent,
            declaration,
            owner,
            installed,
        })
    }
    fn object_status(&self, parent: p::Status, items: &[p::Status]) -> c::ObjectContributionStatus {
        let mut result = c::ObjectContributionStatus {
            plugin_id: parent.manifest.id.clone(),
            digest: parent.digest.clone(),
            enabled: parent.enabled,
            active: active(&parent).is_ok(),
            activation_epoch: parent.activation_epoch.parse::<u64>().unwrap_or(0).into(),
            grant_revision: parent.grant_revision.parse::<u64>().unwrap_or(0).into(),
            dependencies: vec![],
            slots: vec![],
            error_code: None,
        };
        let run = (|| {
            let (installed, declared) = self.object_package(&parent.manifest.id, &parent.digest)?;
            if let Some(common) = self.contributions_for(&installed)? {
                result.dependencies = self.dependency_statuses(&common, items);
            }
            for declaration in declared.detail_slots {
                let slot = c::ObjectSlotQuery {
                    plugin_id: parent.manifest.id.clone(),
                    digest: parent.digest.clone(),
                    slot_id: declaration.id.clone(),
                };
                let owner = owner(slot, &declaration, &parent)?;
                let status = self
                    .objects
                    .lock()
                    .checked("unavailable")?
                    .grants
                    .status(&owner);
                let (grant_context, authorized, error) = match status {
                    Ok(s) => (
                        s.context_identity,
                        s.authorized && result.active,
                        active(&parent).err().map(|e| e.code.to_owned()),
                    ),
                    Err(e) => (
                        package::hash(
                            &serde_json::to_vec(&serde_json::json!([
                                owner,
                                e.code,
                                self.catalog_instance
                            ]))
                            .checked("invalid_record")?,
                        ),
                        false,
                        Some(e.code.into()),
                    ),
                };
                let (context_identity, dependency_missing) =
                    self.object_context(&installed, &declaration.view_id, grant_context, items)?;
                result.slots.push(c::ObjectSlotStatus {
                    descriptor_identity: owner.descriptor_identity,
                    context_identity,
                    declaration,
                    authorized,
                    error_code: error
                        .or_else(|| dependency_missing.then(|| "dependency_missing".into())),
                });
            }
            Ok(())
        })();
        if let Err(e) = run {
            let e: crate::plugin_host::external::Error = e;
            result.active = false;
            result.error_code = Some(e.code.into());
            result.slots.clear();
        }
        result
    }
    pub(crate) fn object_contribution_catalog(&self) -> Result<c::ObjectContributionCatalog> {
        let items = self.list()?;
        Ok(c::ObjectContributionCatalog {
            schema_version: 1,
            items: items
                .iter()
                .filter(|s| s.manifest.requires.iter().any(|r| r == c::FEATURE))
                .cloned()
                .map(|s| self.object_status(s, &items))
                .collect(),
        })
    }
    pub(crate) fn object_contribution_grant(
        &self,
        request: c::ObjectGrantRequest,
        authorize: bool,
    ) -> Result<c::ObjectContributionStatus> {
        request.validate().checked("invalid_input")?;
        let definition = self.object_definition(&request.slot)?;
        if authorize {
            active(&definition.parent)?;
        }
        if definition.owner.descriptor_identity != request.expected_descriptor_identity
            || definition.owner.activation_epoch != request.expected_activation_epoch
            || definition.owner.grant_revision != request.expected_grant_revision
        {
            return Err(fail("stale_snapshot"));
        }
        let grant_context = self
            .objects
            .lock()
            .checked("unavailable")?
            .grants
            .status(&definition.owner)?
            .context_identity;
        let (context, _) = self.object_context(
            &definition.installed,
            &definition.declaration.view_id,
            grant_context.clone(),
            &self.list()?,
        )?;
        if context != request.expected_context_identity {
            return Err(fail("stale_snapshot"));
        }
        self.objects.lock().checked("unavailable")?.grants.apply(
            definition.owner.clone(),
            &grant_context,
            authorize,
        )?;
        if self.object_definition(&request.slot)?.owner != definition.owner {
            return Err(fail("grant_revoked"));
        }
        let parent = self.status(&request.slot.plugin_id, &request.slot.digest)?;
        Ok(self.object_status(parent, &self.list()?))
    }
    fn object_authority(&self, slot: &c::ObjectSlotQuery) -> Result<(Definition, String)> {
        let definition = self.object_definition(slot)?;
        active(&definition.parent)?;
        let status = self
            .objects
            .lock()
            .checked("unavailable")?
            .grants
            .status(&definition.owner)?;
        if !status.authorized {
            return Err(fail("permission_denied"));
        }
        let (context, missing) = self.object_context(
            &definition.installed,
            &definition.declaration.view_id,
            status.context_identity,
            &self.list()?,
        )?;
        if missing {
            return Err(fail("dependency_missing"));
        }
        Ok((definition, context))
    }
    pub(crate) fn issue_object_reference(
        &self,
        request: c::ObjectRefRequest,
        reader: &dyn ObjectSummarySource,
    ) -> Result<c::ObjectReference> {
        request.validate().checked("invalid_input")?;
        let (definition, authority) = self.object_authority(&request.slot)?;
        let ticket = self
            .objects
            .lock()
            .checked("unavailable")?
            .references
            .begin(&request, &definition.declaration, &authority)?;
        let result = (|| {
            let read = reader
                .read_object_summary(&request.object, &definition.declaration.fields)
                .map_err(domain_failure)?;
            read.validate(&request.object, &definition.declaration.fields)
                .checked("invalid_record")?;
            reader
                .ensure_object_summary_current(&read)
                .map_err(domain_failure)?;
            let (_, current) = self.object_authority(&request.slot)?;
            self.objects
                .lock()
                .checked("unavailable")?
                .references
                .finish(&ticket, read, &current)
        })();
        if result.is_err() {
            self.objects
                .lock()
                .checked("unavailable")?
                .references
                .abandon(&ticket);
        }
        result
    }
    pub(crate) fn read_object_reference(
        &self,
        query: c::ObjectRefQuery,
        reader: &dyn ObjectSummarySource,
    ) -> Result<c::ObjectContributionReply> {
        query.validate().checked("invalid_input")?;
        let (_, authority) = self.object_authority(&query.slot)?;
        let captured = self
            .objects
            .lock()
            .checked("unavailable")?
            .references
            .lookup(&query, &authority)?;
        reader
            .ensure_object_summary_current(&captured.read)
            .map_err(domain_failure)?;
        let (_, current) = self.object_authority(&query.slot)?;
        self.objects
            .lock()
            .checked("unavailable")?
            .references
            .lookup(&query, &current)?;
        Ok(c::ObjectContributionReply::Summary {
            object_ref: query.object_ref,
            summary: captured.read.summary,
        })
    }
    pub(crate) fn release_object_reference(
        &self,
        query: c::ObjectRefQuery,
    ) -> Result<c::ObjectContributionReply> {
        self.objects
            .lock()
            .checked("unavailable")?
            .references
            .release(&query)?;
        Ok(c::ObjectContributionReply::Released {
            object_ref: query.object_ref,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use d::object_summary::{ObjectSelector, ObjectSummaryRead, SummaryField};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct Reader(AtomicUsize);
    impl ObjectSummarySource for Reader {
        fn read_object_summary(
            &self,
            _: &ObjectSelector,
            _: &[SummaryField],
        ) -> d::Result<ObjectSummaryRead> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(d::Failure::new(d::ErrorCode::Unsupported))
        }
        fn ensure_object_summary_current(&self, _: &ObjectSummaryRead) -> d::Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(d::Failure::new(d::ErrorCode::Unsupported))
        }
    }
    fn slot() -> c::ObjectSlotQuery {
        c::ObjectSlotQuery {
            plugin_id: "org.example.details".into(),
            digest: "a".repeat(64),
            slot_id: "usage".into(),
        }
    }
    fn manager(root: &Path) -> Manager {
        Manager::open(
            root,
            Arc::new(local_agent_core::providers::vault::MemoryVault::default()),
        )
        .unwrap()
    }
    #[test]
    fn empty_catalog_and_reference_cleanup_do_not_create_grants_or_run_sources() {
        let root = tempfile::tempdir().unwrap();
        let host = manager(root.path());
        let catalog = host.object_contribution_catalog().unwrap();
        assert_eq!(catalog.schema_version, 1);
        assert!(catalog.items.is_empty());
        let reply = host
            .release_object_reference(c::ObjectRefQuery {
                slot: slot(),
                object_ref: uuid::Uuid::new_v4().to_string(),
            })
            .unwrap();
        assert!(matches!(reply, c::ObjectContributionReply::Released { .. }));
        assert!(!root.path().join("object-contributions.sqlite3").exists());
    }
    #[test]
    fn invalid_installation_never_reads_objects_or_persists_approval() {
        let root = tempfile::tempdir().unwrap();
        let host = manager(root.path());
        let reader = Reader(AtomicUsize::new(0));
        let issue = c::ObjectRefRequest {
            slot: slot(),
            expected_context_identity: "a".repeat(64),
            object: ObjectSelector::Provider {
                id: "fixture-provider".into(),
            },
        };
        assert!(host.issue_object_reference(issue, &reader).is_err());
        assert!(
            host.read_object_reference(
                c::ObjectRefQuery {
                    slot: slot(),
                    object_ref: uuid::Uuid::new_v4().to_string()
                },
                &reader
            )
            .is_err()
        );
        assert!(
            host.object_contribution_grant(
                c::ObjectGrantRequest {
                    slot: slot(),
                    expected_descriptor_identity: "b".repeat(64),
                    expected_context_identity: "a".repeat(64),
                    expected_activation_epoch: 1_u64.into(),
                    expected_grant_revision: 1_u64.into()
                },
                true
            )
            .is_err()
        );
        assert_eq!(reader.0.load(Ordering::SeqCst), 0);
        assert!(!root.path().join("object-contributions.sqlite3").exists());
    }
}
