use super::*;
use local_agent_core::domain::provider_draft_recovery::{
    DraftRecoveryEntry, DraftRecoveryOwnership, DraftRecoveryState,
};

pub(crate) struct RecoverySnapshot {
    pub entry: DraftRecoveryEntry,
    pub pin: Option<Pin>,
    pub selection: Option<Selection>,
    pub last_managed: bool,
    pub request: Option<Box<SourceRequest>>,
}

impl Manager {
    /// One bounded metadata pass. SQL results and large original inputs are
    /// inspected individually, not decoded for every row of the inventory.
    pub(crate) fn draft_recovery_entries(&self, store_id: &str) -> Result<Vec<DraftRecoveryEntry>> {
        let _guard = self.db.lock().checked("unavailable")?;
        let mut entries = std::collections::BTreeMap::new();
        let requests = self.request_owners()?;
        for record in self.owner_records(false)? {
            if record.owner.store_id != store_id {
                continue;
            }
            let entry = DraftRecoveryEntry {
                owner: record.owner.clone(),
                label: record.scope.label,
                plugin_id: Some(record.scope.catalog.plugin_id),
                digest: Some(record.scope.catalog.digest),
                endpoint_origin: Some(origin(&record.scope.draft.endpoint)?),
                ownership: if record.grant.is_none() {
                    DraftRecoveryOwnership::Prepared
                } else if record.managed {
                    DraftRecoveryOwnership::Managed
                } else {
                    DraftRecoveryOwnership::Shared
                },
                operation_id: record.operation.as_ref().map(|p| p.operation_id.clone()),
                state: if record.operation.is_none() && !requests.contains(&record.owner) {
                    DraftRecoveryState::Unattached
                } else {
                    DraftRecoveryState::Unchecked
                },
                can_finish: false,
            };
            entries.insert(record.owner.owner_id, entry);
        }
        for owner in requests {
            if owner.store_id != store_id || entries.contains_key(&owner.owner_id) {
                continue;
            }
            entries.insert(
                owner.owner_id.clone(),
                DraftRecoveryEntry {
                    owner,
                    label: "待核查的草稿目录授权".into(),
                    plugin_id: None,
                    digest: None,
                    endpoint_origin: None,
                    ownership: DraftRecoveryOwnership::Missing,
                    operation_id: None,
                    state: DraftRecoveryState::Unchecked,
                    can_finish: false,
                },
            );
        }
        Ok(entries.into_values().collect())
    }
    pub(crate) fn draft_recovery_snapshot(
        &self,
        owner: &DraftGrantOwner,
        inspect: bool,
    ) -> Result<RecoverySnapshot> {
        let _guard = self.db.lock().checked("unavailable")?;
        let record = self.read_owner(owner)?;
        let request = if inspect || record.is_none() {
            self.read_draft_request(owner)?
        } else {
            None
        };
        if let Some(record) = record {
            if let Some(request) = &request {
                Self::check_draft_request(&record, request)?;
            }
            let last_managed = record.managed
                && !self.owners()?.iter().any(|r| {
                    r.owner != record.owner && r.grant.is_some() && r.grant == record.grant
                });
            let entry = DraftRecoveryEntry {
                owner: owner.clone(),
                label: record.scope.label,
                plugin_id: Some(record.scope.catalog.plugin_id),
                digest: Some(record.scope.catalog.digest),
                endpoint_origin: Some(origin(&record.scope.draft.endpoint)?),
                ownership: if record.grant.is_none() {
                    DraftRecoveryOwnership::Prepared
                } else if record.managed {
                    DraftRecoveryOwnership::Managed
                } else {
                    DraftRecoveryOwnership::Shared
                },
                operation_id: record.operation.as_ref().map(|p| p.operation_id.clone()),
                state: DraftRecoveryState::Unavailable,
                can_finish: false,
            };
            Ok(RecoverySnapshot {
                entry,
                pin: record.operation,
                selection: record.grant.as_ref().map(selection),
                last_managed,
                request,
            })
        } else {
            // Lost owner metadata cannot prove grant deletion ownership. The
            // original request remains inspectable/cancellable, and ending a
            // confirmed terminal orphan only removes this request copy.
            let request = request.ok_or(fail("not_found"))?;
            let selected = request.source.catalog().ok_or(fail("invalid_record"))?;
            let a::Target::Draft { label, .. } = &selected.target else {
                return Err(fail("invalid_record"));
            };
            let entry = DraftRecoveryEntry {
                owner: owner.clone(),
                label: label.clone(),
                plugin_id: Some(selected.plugin_id.clone()),
                digest: Some(selected.digest.clone()),
                endpoint_origin: Some(origin(&request.operation.provider.endpoint)?),
                ownership: DraftRecoveryOwnership::Missing,
                operation_id: Some(request.operation.operation_id.clone()),
                state: DraftRecoveryState::Unavailable,
                can_finish: false,
            };
            Ok(RecoverySnapshot {
                entry,
                pin: Some(journal::pin(&request)?),
                selection: None,
                last_managed: false,
                request: Some(request),
            })
        }
    }
    pub(crate) fn finish_orphan_draft_request(
        &self,
        owner: &DraftGrantOwner,
        safe: impl FnOnce(&Pin) -> Result<bool>,
    ) -> Result<bool> {
        let _guard = self.db.lock().checked("unavailable")?;
        if self.read_owner(owner)?.is_some() {
            return Err(fail("stale_snapshot"));
        }
        let Some(request) = self.read_draft_request(owner)? else {
            return Ok(true);
        };
        if !safe(&journal::pin(&request)?)? {
            return Ok(false);
        }
        if self.read_owner(owner)?.is_some() {
            return Err(fail("stale_snapshot"));
        }
        self.remove_draft_request(owner, Some(&journal::hash(&request)?))?;
        Ok(true)
    }
}

fn origin(endpoint: &str) -> Result<String> {
    Ok(reqwest::Url::parse(endpoint)
        .checked("invalid_record")?
        .origin()
        .ascii_serialization())
}
