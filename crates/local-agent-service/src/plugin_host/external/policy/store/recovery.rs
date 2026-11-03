//! Bounded, explicit scope repair. This module never calls a publisher, guest or vault.
use super::*;
use local_agent_core::{
    Store,
    domain::routing_policy_recovery::{
        PolicyRecoveryEntry as Entry, PolicyRecoveryInspection as Inspection,
        PolicyRecoveryMode as Mode, PolicyRecoveryProblem as Problem,
        PolicyRecoveryReference as Ref, PolicyRecoveryScope as Scope, PolicyRecoveryState as State,
    },
};
use std::{collections::BTreeSet, os::unix::fs::MetadataExt, time::Instant};

const LIMIT: usize = 256;
const MAX_EVIDENCE: usize = 8 * 1024 * 1024;
pub(in super::super) struct Reference {
    created: Instant,
    scope: Scope,
    fingerprint: String,
    native: Option<Control>,
    pending: Option<Control>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Evidence {
    scope: Scope,
    database_identity: (u64, u64),
    core: serde_json::Value,
    control: Option<String>,
    marker: Option<Vec<u8>>,
    marker_identity: Option<(u64, u64)>,
    bindings: Vec<(String, String)>,
    intents: Vec<(String, String)>,
    receipts: Vec<(String, serde_json::Value)>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Receipt {
    reference: Ref,
    mode: Mode,
    /// Preserve the original bytes before replacing either control copy.
    before: Backup,
    control: Control,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Backup {
    fingerprint: String,
    control: Option<String>,
    marker: Option<Vec<u8>>,
    marker_identity: Option<(u64, u64)>,
}
struct Snapshot {
    evidence: Evidence,
    inspection: Inspection,
    pending: Option<Control>,
}
fn fingerprint(e: &Evidence) -> Result<String> {
    Ok(package::hash(encode(e)?.as_bytes()))
}
fn valid_scope(scope: &Scope) -> Result<()> {
    for s in [&scope.store_id, &scope.service_id] {
        uuid(s)?;
    }
    Ok(())
}
fn core_state(core: &Store, scope: &Scope) -> Result<serde_json::Value> {
    core.require_model_service_store(&scope.store_id)
        .checked("stale_snapshot")?;
    match core.model_service(&scope.service_id) {
        Ok(s) => serde_json::to_value(s).checked("invalid_record"),
        Err(e) if e.code == "not_found" => Ok(serde_json::Value::Null),
        Err(_) => Err(fail("unavailable")),
    }
}
fn bounded_rows(
    db: &Connection,
    sql: &str,
    args: &[&dyn rusqlite::ToSql],
) -> Result<Vec<(String, String)>> {
    let mut st = db.prepare(sql).checked("unavailable")?;
    let mut cursor = st.query(args).checked("unavailable")?;
    let mut rows = vec![];
    let mut bytes = 0;
    while let Some(r) = cursor.next().checked("unavailable")? {
        let id: String = r.get(0).checked("invalid_record")?;
        let record: Option<String> = r.get(1).checked("invalid_record")?;
        let record = record.ok_or(fail("resource_limit"))?;
        bytes += id.len() + record.len();
        if rows.len() >= LIMIT || bytes > MAX_EVIDENCE {
            return Err(fail("resource_limit"));
        }
        rows.push((id, record));
    }
    Ok(rows)
}
impl Host {
    fn recovery_db(&self) -> Result<&Connection> {
        // Never create an optional repository just by opening recovery.
        let m = std::fs::symlink_metadata(&self.db_path).checked("unavailable")?;
        if !m.is_file() || m.file_type().is_symlink() || m.nlink() != 1 {
            return Err(fail("invalid_record"));
        }
        if self
            .db_identity
            .get()
            .is_some_and(|id| *id != (m.dev(), m.ino()))
        {
            return Err(fail("stale_snapshot"));
        }
        let _ = evidence::database(self.db_path.parent().ok_or(fail("invalid_record"))?)?;
        self.db()
    }
    pub(in super::super) fn recovery_list(
        &self,
        core: &Store,
        store: &str,
    ) -> Result<(Vec<Entry>, bool)> {
        uuid(store)?;
        core.require_model_service_store(store)
            .checked("stale_snapshot")?;
        let cache = self.presence.read().map_err(|_| fail("unavailable"))?;
        let mut scopes: BTreeSet<String> = cache
            .scopes
            .values()
            .filter(|c| c.store_id == store)
            .map(|c| c.service_id.clone())
            .collect();
        let mut limited = cache.unavailable;
        scopes.extend(
            cache
                .uncertain_scopes
                .iter()
                .filter(|(s, _)| s == store)
                .map(|(_, id)| id.clone()),
        );
        let unknown = cache.unknown.clone();
        drop(cache);
        if self.db_path.exists() {
            match self.recovery_db().and_then(|db| {
                let mut query = db.prepare("SELECT * FROM (SELECT CASE WHEN length(scope)<=128 THEN scope ELSE '' END FROM policy_controls LIMIT 257) UNION ALL SELECT * FROM (SELECT CASE WHEN length(record)<=2097152 AND json_valid(record) THEN CASE WHEN json_type(record,'$.status.storeId')='text' AND json_type(record,'$.status.serviceId')='text' AND length(json_extract(record,'$.status.storeId'))<=64 AND length(json_extract(record,'$.status.serviceId'))<=64 THEN json_extract(record,'$.status.storeId')||':'||json_extract(record,'$.status.serviceId') ELSE '' END ELSE '' END FROM policy_intents LIMIT 257)").checked("unavailable")?;
                let mut keys = query.query_map([], |r| r.get::<_, String>(0)).checked("unavailable")?.collect::<std::result::Result<Vec<_>,_>>().checked("invalid_record")?;
                // Startup has already identified missing scopes. Check those
                // bounded identities through the covering index rather than
                // scanning every historical version to list a scope once.
                for id in unknown {
                    let key = Self::scope(store,&id);
                    let exists: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM policy_bindings WHERE scope=?1)",[&key],|r|r.get(0)).checked("unavailable")?;
                    if exists { keys.push(key); }
                }
                Ok(keys)
            }) {
                Ok(rows) => {
                    for row in rows { if let Some((s,id)) = row.split_once(':') {
                        if s == store && uuid(id).is_ok() { scopes.insert(id.into()); }
                    } else { limited = true; } }
                }
                Err(_) => limited = true,
            }
        }
        // Corrupt markers still identify a scope by their host-generated name.
        let directory = match std::fs::symlink_metadata(&self.root) {
            Ok(m) if m.is_dir() && !m.file_type().is_symlink() => {
                std::fs::read_dir(&self.root).ok()
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            _ => {
                limited = true;
                None
            }
        };
        if let Some(rows) = directory {
            for (i, row) in rows.enumerate() {
                if i >= LIMIT {
                    limited = true;
                    break;
                }
                let Ok(row) = row else {
                    limited = true;
                    continue;
                };
                let name = row.file_name().to_string_lossy().into_owned();
                let fields: Vec<_> = name.split('-').collect();
                if fields.len() == 3
                    && let (Ok(s), Ok(id)) = (
                        uuid::Uuid::parse_str(fields[0]),
                        uuid::Uuid::parse_str(fields[1]),
                    )
                    && s.to_string() == store
                    && marker_name(store, &id.to_string()).ok().as_deref() == Some(&name)
                {
                    scopes.insert(id.to_string());
                }
            }
        }
        limited |= scopes.len() > LIMIT;
        let entries = scopes
            .into_iter()
            .take(LIMIT)
            .map(|service_id| {
                let scope = Scope {
                    store_id: store.into(),
                    service_id,
                };
                let state = core_state(core, &scope);
                Entry {
                    service_name: state
                        .as_ref()
                        .ok()
                        .and_then(|v| v["name"].as_str().map(str::to_owned)),
                    service_exists: state.ok().map(|v| !v.is_null()),
                    scope,
                    state: State::Unchecked,
                    problem: None,
                    pending_operation_ids: vec![],
                }
            })
            .collect();
        Ok((entries, limited))
    }
    fn recovery_snapshot(&self, db: &Connection, core: &Store, scope: &Scope) -> Result<Snapshot> {
        valid_scope(scope)?;
        let core_value = core_state(core, scope)?;
        let scope_key = Self::scope(&scope.store_id, &scope.service_id);
        let control: Option<String> = db.query_row("SELECT CASE WHEN length(record)<=4096 THEN record END FROM policy_controls WHERE scope=?1",[&scope_key],|r| r.get::<_,Option<String>>(0)).optional().checked("unavailable")?.map(|v| v.ok_or(fail("resource_limit"))).transpose()?;
        let path = self
            .root
            .join(marker_name(&scope.store_id, &scope.service_id)?);
        if let Ok(m) = std::fs::symlink_metadata(&self.root)
            && (!m.is_dir() || m.file_type().is_symlink())
        {
            return Err(fail("invalid_record"));
        }
        let (marker, marker_identity) = match std::fs::symlink_metadata(&path) {
            Ok(m) => (
                Some(package::read_file(&path, 4096)?),
                Some((m.dev(), m.ino())),
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (None, None),
            Err(_) => return Err(fail("unavailable")),
        };
        let bindings = bounded_rows(
            db,
            "SELECT version,CASE WHEN length(record)<=16384 THEN record END FROM policy_bindings WHERE scope=?1 ORDER BY version LIMIT 257",
            &[&scope_key],
        )?;
        // All original publication identities must be readable, including any
        // that malformed scope data would otherwise hide from a WHERE filter.
        let all_intents = bounded_rows(
            db,
            "SELECT id,CASE WHEN length(record)<=2097152 THEN record END FROM policy_intents ORDER BY id LIMIT 257",
            &[],
        )?;
        let mut intents = vec![];
        let mut pending = vec![];
        let mut receipts = vec![];
        for (id, raw) in all_intents {
            let i: Intent = decode(&raw)?;
            if i.status.store_id != scope.store_id || i.status.service_id != scope.service_id {
                continue;
            }
            i.status.request.validate().checked("invalid_record")?;
            if id != i.status.operation_id
                || i.status.request.operation_id != id
                || i.status.request.store_id != scope.store_id
                || i.status.request.action.identity().1 != scope.service_id
                || i.status.request.action.identity().2 != i.before_revision
                || uuid(&i.control_revision).is_err()
                || !matches!(&i.status.request.action, local_agent_core::model_services::operations::OperationAction::Save { draft } if draft.routing_policy.enabled)
            {
                return Err(fail("invalid_record"));
            }
            let result = match core.model_service_operation_for(&i.status.request) {
                Ok(receipt) => serde_json::to_value(receipt).checked("invalid_record")?,
                Err(e) if e.code == "operation_not_found" => serde_json::Value::Null,
                Err(_) => return Err(fail("invalid_record")),
            };
            match i.status.state.as_str() {
                "prepared" | "committing" | "unknown" => pending.push(i.clone()),
                "committed" if !result.is_null() && i.status.binding.is_some() => {}
                "cancelled" if result.is_null() => {}
                _ => return Err(fail("invalid_record")),
            }
            receipts.push((id.clone(), result));
            intents.push((id, raw));
        }
        let sql = control.as_ref().map(|v| decode::<Control>(v)).transpose()?;
        let disk = marker
            .as_ref()
            .map(|v| serde_json::from_slice::<Control>(v).checked("invalid_record"))
            .transpose()?;
        for c in [&sql, &disk].into_iter().flatten() {
            if c.store_id != scope.store_id
                || c.service_id != scope.service_id
                || uuid(&c.revision).is_err()
                || uuid(&c.authority).is_err()
            {
                return Err(fail("invalid_record"));
            }
            if let Some(op) = &c.pending
                && !pending.iter().any(|i| &i.status.operation_id == op)
            {
                return Err(fail("invalid_record"));
            }
        }
        let cache = self.presence.read().map_err(|_| fail("unavailable"))?;
        if cache.unavailable {
            return Err(fail("resource_limit"));
        }
        let key = (scope.store_id.clone(), scope.service_id.clone());
        let normalized = uuid::Uuid::parse_str(&scope.service_id)
            .checked("invalid_input")?
            .to_string();
        let name = marker_name(&scope.store_id, &scope.service_id)?;
        let own_unknown =
            cache.uncertain_scopes.contains(&key) || cache.unknown_markers.contains_key(&name);
        let known = cache.scopes.contains_key(&key) || own_unknown;
        let capacity = known || cache.scope_count() < LIMIT;
        let consistent = sql.is_some()
            && sql == disk
            && !cache.unknown.contains(&normalized)
            && cache.scopes.get(&key) == sql.as_ref();
        let related_unknown = !own_unknown && cache.unknown.contains(&normalized);
        if sql.is_none() && disk.is_none() && bindings.is_empty() && intents.is_empty() {
            return Err(fail("not_found"));
        }
        drop(cache);
        let mut proposal = None;
        if pending.len() == 1 {
            let i = &pending[0];
            let receipt = &receipts
                .iter()
                .find(|(id, _)| id == &i.status.operation_id)
                .ok_or(fail("invalid_record"))?
                .1;
            let current_revision = core_value["revision"].as_u64();
            let identity = operation_identity(&i.status.request)?;
            let version_matches = if receipt.is_null() {
                current_revision.unwrap_or(0) == i.before_revision
            } else {
                core_value["versionId"] == receipt["service"]["versionId"]
                    && current_revision == Some(i.before_revision + 1)
            };
            let prior = match &i.expected {
                None => Some(None),
                Some(expected) => {
                    let rows = bindings
                        .iter()
                        .map(|(_, r)| decode::<Binding>(r))
                        .collect::<Result<Vec<_>>>()?;
                    let matches: Vec<_> = rows
                        .into_iter()
                        .filter(|b| {
                            b.status.revision == *expected
                                && b.status.store_id == scope.store_id
                                && b.status.service_id == scope.service_id
                                && uuid(&b.status.service_version_id).is_ok()
                        })
                        .collect();
                    if matches.len() == 1 {
                        Some(Some(matches[0].status.clone()))
                    } else {
                        // A surviving original pending control also witnesses
                        // a prior explicit unbind or cancellation revision.
                        // Its head need not share that control revision.
                        [&sql, &disk].into_iter().flatten().find_map(|c| {
                            if c.revision != i.control_revision || &c.revision != expected {
                                return None;
                            }
                            match (&c.version, &c.head) {
                                (None, None) => Some(None),
                                (Some(version), Some(head)) => {
                                    bindings.iter().find_map(|(_, raw)| {
                                        let b = decode::<Binding>(raw).ok()?;
                                        (b.status == *head
                                            && b.status.store_id == scope.store_id
                                            && b.status.service_id == scope.service_id
                                            && &b.status.service_version_id == version)
                                            .then(|| Some(head.clone()))
                                    })
                                }
                                _ => None,
                            }
                        })
                    }
                }
            };
            if version_matches
                && i.authorization.is_some()
                && [&sql, &disk].into_iter().flatten().all(|c| {
                    c.pending.as_deref() == Some(i.status.operation_id.as_str())
                        && c.pending_request_identity
                            .as_deref()
                            .is_none_or(|v| v == identity)
                })
                && let Some(head) = prior
            {
                proposal = Some(Control {
                    store_id: scope.store_id.clone(),
                    service_id: scope.service_id.clone(),
                    revision: i.control_revision.clone(),
                    authority: uuid::Uuid::new_v4().to_string(),
                    version: head.as_ref().map(|h| h.service_version_id.clone()),
                    head,
                    pending: Some(i.status.operation_id.clone()),
                    pending_request_identity: Some(identity),
                });
            }
        }
        // A deleted (or never-created) service has no normal binding editor.
        // Retire its remaining requirement explicitly without recreating the
        // service or discarding the historical binding.
        let orphan_requirement = core_value.is_null()
            && sql
                .as_ref()
                .is_some_and(|c| c.head.is_some() || c.version.is_some());
        let needs = !consistent || orphan_requirement;
        let can_restore_native = needs && capacity && !related_unknown && pending.is_empty();
        let can_restore_pending = needs && capacity && !related_unknown && proposal.is_some();
        let problem = if !capacity {
            Some(Problem::Capacity)
        } else if related_unknown {
            Some(Problem::IncompleteEvidence)
        } else if !pending.is_empty() {
            Some(Problem::PendingPublication)
        } else if sql.is_none() {
            Some(Problem::MissingControl)
        } else if !consistent {
            Some(Problem::MarkerMismatch)
        } else {
            None
        };
        let state = if !needs {
            State::Consistent
        } else if can_restore_native || can_restore_pending {
            State::NeedsRecovery
        } else {
            State::Blocked
        };
        let m = std::fs::symlink_metadata(&self.db_path).checked("unavailable")?;
        Ok(Snapshot {
            evidence: Evidence {
                scope: scope.clone(),
                database_identity: (m.dev(), m.ino()),
                core: core_value.clone(),
                control,
                marker,
                marker_identity,
                bindings,
                intents,
                receipts,
            },
            inspection: Inspection {
                entry: Entry {
                    scope: scope.clone(),
                    service_name: core_value["name"].as_str().map(str::to_owned),
                    service_exists: Some(!core_value.is_null()),
                    state,
                    problem,
                    pending_operation_ids: pending
                        .iter()
                        .map(|i| i.status.operation_id.clone())
                        .collect(),
                },
                reference: None,
                current_service_version: core_value["versionId"].as_str().map(str::to_owned),
                can_restore_native,
                can_restore_pending,
            },
            pending: proposal,
        })
    }
    pub(in super::super) fn recovery_inspect(
        &mut self,
        core: &Store,
        scope: Scope,
    ) -> Result<Inspection> {
        valid_scope(&scope)?;
        let result = self
            .recovery_db()
            .and_then(|db| self.recovery_snapshot(db, core, &scope));
        self.recovery_references
            .retain(|_, r| r.created.elapsed().as_secs() < 60 && r.scope != scope);
        let mut snapshot = match result {
            Ok(s) => s,
            Err(e) => {
                return Ok(Inspection {
                    entry: Entry {
                        scope,
                        service_name: None,
                        service_exists: None,
                        state: State::Blocked,
                        problem: Some(if e.code == "resource_limit" {
                            Problem::Capacity
                        } else {
                            Problem::Unavailable
                        }),
                        pending_operation_ids: vec![],
                    },
                    reference: None,
                    current_service_version: None,
                    can_restore_native: false,
                    can_restore_pending: false,
                });
            }
        };
        if snapshot.inspection.can_restore_native || snapshot.inspection.can_restore_pending {
            if self.recovery_references.len() >= 32 {
                return Err(fail("busy"));
            }
            let id = uuid::Uuid::new_v4().to_string();
            let native = snapshot.inspection.can_restore_native.then(|| Control {
                store_id: scope.store_id.clone(),
                service_id: scope.service_id.clone(),
                revision: uuid::Uuid::new_v4().to_string(),
                authority: uuid::Uuid::new_v4().to_string(),
                version: None,
                head: None,
                pending: None,
                pending_request_identity: None,
            });
            self.recovery_references.insert(
                id.clone(),
                Reference {
                    created: Instant::now(),
                    scope: scope.clone(),
                    fingerprint: fingerprint(&snapshot.evidence)?,
                    native,
                    pending: snapshot.pending,
                },
            );
            snapshot.inspection.reference = Some(Ref {
                scope,
                reference_id: id,
            });
        }
        Ok(snapshot.inspection)
    }
    pub(in super::super) fn recovery_resolve(
        &mut self,
        core: &Store,
        reference: Ref,
        mode: Mode,
    ) -> Result<String> {
        valid_scope(&reference.scope)?;
        uuid(&reference.reference_id)?;
        let db = self.recovery_db()?;
        let tx = rusqlite::Transaction::new_unchecked(db, rusqlite::TransactionBehavior::Immediate)
            .checked("unavailable")?;
        let has_table: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='policy_scope_recoveries')",[],|r|r.get(0)).checked("unavailable")?;
        let saved: Option<String> = if has_table {
            tx.query_row("SELECT CASE WHEN length(record)<=2097152 THEN record END FROM policy_scope_recoveries WHERE id=?1",[&reference.reference_id],|r|r.get::<_,Option<String>>(0)).optional().checked("unavailable")?.map(|v|v.ok_or(fail("invalid_record"))).transpose()?
        } else {
            None
        };
        let mut snapshot = self.recovery_snapshot(&tx, core, &reference.scope)?;
        let receipt: Receipt = if let Some(saved) = saved {
            let r: Receipt = decode(&saved)?;
            if r.reference != reference
                || r.mode != mode
                || snapshot.evidence.control.as_deref() != Some(encode(&r.control)?.as_str())
            {
                return Err(fail("stale_snapshot"));
            }
            // A receipt permits only finishing the same marker, never changing
            // a later scope choice, core version or original publication.
            let marker = encode(&r.control)?.into_bytes();
            if snapshot.evidence.marker != r.before.marker
                && snapshot.evidence.marker.as_ref() != Some(&marker)
            {
                return Err(fail("stale_snapshot"));
            }
            snapshot.evidence.control = r.before.control.clone();
            snapshot.evidence.marker = r.before.marker.clone();
            snapshot.evidence.marker_identity = r.before.marker_identity;
            if fingerprint(&snapshot.evidence)? != r.before.fingerprint {
                return Err(fail("stale_snapshot"));
            }
            r
        } else {
            let r = self
                .recovery_references
                .get(&reference.reference_id)
                .ok_or(fail("stale_snapshot"))?;
            if r.scope != reference.scope
                || r.created.elapsed().as_secs() >= 60
                || r.fingerprint != fingerprint(&snapshot.evidence)?
            {
                return Err(fail("stale_snapshot"));
            }
            let control = match &mode {
                Mode::Native {} => r
                    .native
                    .clone()
                    .filter(|_| snapshot.inspection.can_restore_native),
                Mode::PendingOriginal {} => r
                    .pending
                    .clone()
                    .filter(|_| snapshot.inspection.can_restore_pending),
            }
            .ok_or(fail("result_pending"))?;
            let count: u32 = if has_table {
                tx.query_row("SELECT COUNT(*) FROM policy_scope_recoveries", [], |r| {
                    r.get(0)
                })
                .checked("unavailable")?
            } else {
                0
            };
            if count >= LIMIT as u32 {
                return Err(fail("resource_limit"));
            }
            let receipt = Receipt {
                reference: reference.clone(),
                mode: mode.clone(),
                before: Backup {
                    fingerprint: fingerprint(&snapshot.evidence)?,
                    control: snapshot.evidence.control,
                    marker: snapshot.evidence.marker,
                    marker_identity: snapshot.evidence.marker_identity,
                },
                control,
            };
            tx.execute_batch("CREATE TABLE IF NOT EXISTS policy_scope_recoveries(id TEXT PRIMARY KEY,record TEXT NOT NULL)").checked("unavailable")?;
            tx.execute(
                "INSERT INTO policy_scope_recoveries VALUES(?1,?2)",
                params![reference.reference_id, encode(&receipt)?],
            )
            .checked("unavailable")?;
            tx.execute(
                "INSERT OR REPLACE INTO policy_controls VALUES(?1,?2)",
                params![
                    Self::scope(&reference.scope.store_id, &reference.scope.service_id),
                    encode(&receipt.control)?
                ],
            )
            .checked("unavailable")?;
            receipt
        };
        // The durable backup and control commit together. Failure from here is
        // explicitly unknown; the exact receipt supports an idempotent finish.
        tx.commit().checked("scope_recovery_unknown")?;
        self.recovery_marker(&receipt.control)
            .map_err(|_| fail("scope_recovery_unknown"))?;
        self.recovery_references.remove(&reference.reference_id);
        Ok(receipt.control.revision)
    }
    fn recovery_marker(&self, c: &Control) -> Result<()> {
        // Replacing this already-reserved unknown scope consumes no new slot.
        let cache = self.presence.read().map_err(|_| fail("unavailable"))?;
        if cache.unavailable
            || (!cache
                .scopes
                .contains_key(&(c.store_id.clone(), c.service_id.clone()))
                && !cache
                    .uncertain_scopes
                    .contains(&(c.store_id.clone(), c.service_id.clone()))
                && !cache
                    .unknown_markers
                    .contains_key(&marker_name(&c.store_id, &c.service_id)?)
                && cache.scope_count() >= LIMIT)
        {
            return Err(fail("resource_limit"));
        }
        drop(cache);
        super::super::super::full::store::directory(&self.root)?;
        write_private(
            &self.root.join(marker_name(&c.store_id, &c.service_id)?),
            encode(c)?.as_bytes(),
        )?;
        let mut cache = self.presence.write().map_err(|_| fail("unavailable"))?;
        cache.mark_known(c)
    }
}

#[cfg(test)]
mod tests;
