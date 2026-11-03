use super::super::{Checked, Manager, Result, fail, package as legacy};
use super::{
    runtime::Context,
    store::{self, Binding, Installed, Selection},
};
use local_agent_core::plugin::policy as p;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::Arc};
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewRecord {
    review: p::Review,
    publisher_revision: u64,
    created: String,
    done: bool,
}
impl Manager {
    pub(crate) fn policy_needed(&self, service: &str) -> bool {
        let Ok(c) = self.policy_presence.read() else {
            return true;
        };
        let normal = uuid::Uuid::parse_str(service)
            .map(|i| i.to_string())
            .unwrap_or_default();
        c.unavailable
            || c.unknown.contains(&normal)
            || c.scopes
                .values()
                .any(|c| c.service_id == service && (c.version.is_some() || c.pending.is_some()))
    }
    pub(crate) fn policy_service_enabled(
        &self,
        store: &str,
        service: &str,
        enabled: bool,
    ) -> Result<()> {
        if enabled {
            return Ok(());
        }
        let cache = self
            .policy_presence
            .read()
            .map_err(|_| fail("unavailable"))?;
        if cache
            .scopes
            .get(&(store.into(), service.into()))
            .is_some_and(|c| c.head.is_some())
        {
            return Err(fail("grant_revoked"));
        }
        Ok(())
    }
    pub(crate) fn policy_publication_guard(
        &self,
        store: &str,
        service: &str,
        operation: Option<&local_agent_core::model_services::operations::OperationRequest>,
    ) -> Result<()> {
        let cache = self
            .policy_presence
            .read()
            .map_err(|_| fail("unavailable"))?;
        if cache.unavailable
            || cache.unknown.contains(
                &uuid::Uuid::parse_str(service)
                    .checked("invalid_input")?
                    .to_string(),
            )
        {
            return Err(fail("invalid_record"));
        }
        if let Some(c) = cache.scopes.get(&(store.into(), service.into()))
            && let Some(id) = c.pending.as_deref()
        {
            let Some(request) = operation else {
                return Err(fail("result_pending"));
            };
            let identity = store::operation_identity(request)?;
            if request.operation_id != id
                || c.pending_request_identity.as_deref() != Some(identity.as_str())
            {
                return Err(fail("result_pending"));
            }
        }
        Ok(())
    }
    pub(in crate::plugin_host::external) fn policy_inventory(
        &self,
    ) -> Result<Vec<super::super::installation::Entry>> {
        self.policy.try_lock().checked("busy")?.inventory()
    }
    pub(super) fn policy_verified(
        &self,
        q: &p::PackageQuery,
    ) -> Result<(Installed, std::path::PathBuf)> {
        self.ensure_running()?;
        let (i, path) = {
            let h = self.policy.lock().checked("unavailable")?;
            let i = h.installed(q)?;
            let path = h.path(&i);
            (i, path)
        };
        let bytes = legacy::read_file(&path, legacy::MAX_ARCHIVE)?;
        let checked = super::package::inspect(&bytes)?;
        if checked.digest != i.digest
            || checked.manifest != i.manifest
            || checked.signing_key != i.signing_key
        {
            return Err(fail("signature_invalid"));
        }
        if self.full_publisher_key(&i.manifest.publisher)?.as_ref() != Some(&i.signing_key) {
            return Err(fail("publisher_untrusted"));
        }
        Ok((i, path))
    }
    pub(super) fn policy_status(&self, q: &p::PackageQuery) -> Result<p::PackageStatus> {
        let (i, selected, live) = {
            let h = self.policy.lock().checked("unavailable")?;
            let i = h.installed(q)?;
            let selected = h.selection(&q.plugin_id)?;
            let live = h.active.get(&q.plugin_id).copied();
            (i, selected, live)
        };
        let trusted =
            self.full_publisher_key(&i.manifest.publisher)?.as_ref() == Some(&i.signing_key);
        let enabled = selected
            .as_ref()
            .is_some_and(|s| s.digest == q.digest && s.enabled);
        Ok(p::PackageStatus {
            manifest: i.manifest,
            digest: i.digest,
            signing_key: i.signing_key,
            publisher_trusted: trusted,
            enabled,
            active: enabled && trusted && selected.as_ref().is_some_and(|s| Some(s.epoch) == live),
            epoch: selected
                .map(|s| s.epoch.to_string())
                .unwrap_or_else(|| "0".into()),
        })
    }
    pub(super) fn policy_inspect(&self, path: &Path) -> Result<p::Review> {
        self.ensure_running()?;
        let bytes = legacy::read_file(path, legacy::MAX_ARCHIVE)?;
        let pkg = super::package::inspect(&bytes)?;
        let key = self.full_publisher_key(&pkg.manifest.publisher)?;
        if key.as_ref().is_some_and(|key| key != &pkg.signing_key) {
            return Err(fail("publisher_changed"));
        }
        let pubrev = self.full_publisher_revision(&pkg.manifest.publisher)?;
        let r = p::Review {
            package_ref: uuid::Uuid::new_v4().to_string(),
            digest: pkg.digest,
            manifest: pkg.manifest,
            signing_key: pkg.signing_key,
            trusted_publisher: key.is_some(),
            compressed_bytes: bytes.len().to_string(),
            expanded_bytes: pkg.expanded.to_string(),
        };
        let full = self.full.try_lock().checked("busy")?;
        let h = self.policy.try_lock().checked("busy")?;
        prune_reviews(&h)?;
        let n: i64 = h
            .db()?
            .query_row(
                "SELECT count(*) FROM policy_reviews WHERE json_extract(record,'$.done')=0",
                [],
                |r| r.get(0),
            )
            .checked("unavailable")?;
        if n >= 32 {
            return Err(fail("resource_limit"));
        }
        super::super::installation::check_staging(
            [&self.root, &h.cache],
            super::super::installation::HostKind::History,
            bytes.len() as u64,
        )?;
        full.write_private(
            &h.cache
                .join("staging")
                .join(format!("{}.laplug", r.package_ref)),
            &bytes,
        )?;
        h.db()?
            .execute(
                "INSERT INTO policy_reviews VALUES(?1,?2)",
                params![
                    r.package_ref,
                    store::encode(&ReviewRecord {
                        review: r.clone(),
                        publisher_revision: pubrev,
                        created: chrono::Utc::now().to_rfc3339(),
                        done: false
                    })?
                ],
            )
            .checked("unavailable")?;
        Ok(r)
    }
    pub(super) fn policy_install(
        &self,
        reference: &str,
        digest: &str,
        signing_key: &str,
        approve: bool,
    ) -> Result<p::PackageStatus> {
        store::uuid(reference)?;
        let _install = self.installation.try_lock().checked("busy")?;
        let record = {
            let h = self.policy.lock().checked("unavailable")?;
            let row: String = h
                .db()?
                .query_row(
                    "SELECT record FROM policy_reviews WHERE id=?1",
                    [reference],
                    |r| r.get(0),
                )
                .checked("not_found")?;
            store::decode::<ReviewRecord>(&row)?
        };
        if record.review.digest != digest || record.review.signing_key != signing_key {
            return Err(fail("stale_snapshot"));
        }
        if record.done {
            return self.policy_status(&p::PackageQuery {
                plugin_id: record.review.manifest.id,
                digest: digest.into(),
            });
        }
        if chrono::DateTime::parse_from_rfc3339(&record.created).map_or(true, |t| {
            t < chrono::Utc::now() - chrono::Duration::minutes(10)
        }) {
            return Err(fail("stale_snapshot"));
        }
        let stage = {
            self.policy
                .lock()
                .checked("unavailable")?
                .cache
                .join("staging")
                .join(format!("{reference}.laplug"))
        };
        let bytes = legacy::read_file(&stage, legacy::MAX_ARCHIVE)?;
        let checked = super::package::inspect(&bytes)?;
        if checked.digest != digest
            || checked.manifest != record.review.manifest
            || checked.signing_key != signing_key
        {
            return Err(fail("signature_invalid"));
        }
        let p = Installed {
            manifest: checked.manifest,
            digest: checked.digest,
            signing_key: checked.signing_key,
            bytes: bytes.len() as u64,
            expanded: checked.expanded,
            removed: false,
        };
        let mut inventory = {
            self.full
                .try_lock()
                .checked("busy")?
                .installation_inventory()?
        };
        inventory.extend(self.policy_inventory()?);
        {
            let db = self.db.lock()?;
            super::super::installation::check(
                &db,
                &inventory,
                &super::super::installation::Entry {
                    host_kind: super::super::installation::HostKind::Routing,
                    id: p.manifest.id.clone(),
                    version: p.manifest.version.clone(),
                    digest: p.digest.clone(),
                    publisher: p.manifest.publisher.clone(),
                    key: Some(p.signing_key.clone()),
                    bytes: p.bytes,
                },
            )?;
            let old: Option<String> = db
                .query_row(
                    "SELECT public_key FROM publishers WHERE id=?1",
                    [&p.manifest.publisher],
                    |r| r.get(0),
                )
                .optional()
                .checked("unavailable")?;
            if old.as_ref().is_some_and(|key| key != &p.signing_key) {
                return Err(fail("publisher_changed"));
            }
            if old.is_none() && !approve {
                return Err(fail("publisher_untrusted"));
            }
            if self.full_publisher_revision(&p.manifest.publisher)? != record.publisher_revision {
                return Err(fail("grant_revoked"));
            }
            if old.is_none() {
                db.execute(
                    "INSERT INTO publishers VALUES(?1,?2)",
                    params![p.manifest.publisher, p.signing_key],
                )
                .checked("unavailable")?;
            }
        }
        {
            let full = self.full.try_lock().checked("busy")?;
            let h = self.policy.lock().checked("unavailable")?;
            let target = h.path(&p);
            super::super::installation::check_archives([&self.root, &h.cache], &target, p.bytes)?;
            full.write_private(&target, &bytes)?;
            let tx = h.db()?.unchecked_transaction().checked("unavailable")?;
            tx.execute(
                "INSERT OR REPLACE INTO policy_packages VALUES(?1,?2,?3)",
                params![p.digest, p.manifest.id, store::encode(&p)?],
            )
            .checked("unavailable")?;
            tx.execute(
                "INSERT OR IGNORE INTO policy_selections VALUES(?1,?2)",
                params![
                    p.manifest.id,
                    store::encode(&Selection {
                        digest: p.digest.clone(),
                        previous: None,
                        enabled: false,
                        epoch: 1
                    })?
                ],
            )
            .checked("unavailable")?;
            tx.execute(
                "UPDATE policy_reviews SET record=?2 WHERE id=?1",
                params![
                    reference,
                    store::encode(&ReviewRecord {
                        done: true,
                        ..record
                    })?
                ],
            )
            .checked("unavailable")?;
            tx.commit().checked("unavailable")?;
            if legacy::read_file(&stage, legacy::MAX_ARCHIVE)
                .is_ok_and(|b| legacy::hash(&b) == p.digest)
            {
                std::fs::remove_file(stage).checked("unavailable")?;
            }
        }
        self.policy_status(&p::PackageQuery {
            plugin_id: p.manifest.id,
            digest: p.digest,
        })
    }
    pub(super) async fn policy_change(
        self: &Arc<Self>,
        q: p::PackageQuery,
        action: p::StateAction,
    ) -> Result<p::PackageStatus> {
        let gate = self.gate(&q.plugin_id)?;
        let _guard = gate.try_lock_owned().checked("busy")?;
        if matches!(action, p::StateAction::Enable | p::StateAction::Rollback) {
            let actual = if action == p::StateAction::Rollback {
                let h = self.policy.lock().checked("unavailable")?;
                let selected = h.selection(&q.plugin_id)?.ok_or(fail("not_found"))?;
                if selected.digest != q.digest {
                    return Err(fail("stale_snapshot"));
                }
                p::PackageQuery {
                    plugin_id: q.plugin_id.clone(),
                    digest: selected.previous.ok_or(fail("not_found"))?,
                }
            } else {
                q.clone()
            };
            let (i, path) = self.policy_verified(&actual)?;
            let before = {
                self.policy
                    .lock()
                    .checked("unavailable")?
                    .selection(&q.plugin_id)?
                    .ok_or(fail("not_found"))?
            };
            let epoch = before.epoch.checked_add(1).ok_or(fail("resource_limit"))?;
            let pubrev = self.full_publisher_revision(&i.manifest.publisher)?;
            let c = Context {
                package: i,
                path,
                before: before.clone(),
                epoch,
                publisher_revision: pubrev,
                preflight: true,
                binding: None,
            };
            self.policy_run(c.clone(), None).await?;
            self.policy_check(&c)?;
            {
                let mut h = self.policy.lock().checked("unavailable")?;
                if h.selection(&q.plugin_id)? != Some(before.clone()) {
                    return Err(fail("stale_snapshot"));
                }
                let previous = if before.digest != actual.digest {
                    Some(before.digest)
                } else {
                    before.previous
                };
                let selected = h.cache.join("selected");
                super::super::full::store::directory(&selected)?;
                store::write_private(&selected.join(&q.plugin_id), actual.digest.as_bytes())?;
                h.save_selection(
                    &q.plugin_id,
                    &Selection {
                        digest: actual.digest.clone(),
                        previous,
                        enabled: true,
                        epoch,
                    },
                )?;
                h.active.insert(q.plugin_id.clone(), epoch);
            }
            return self.policy_status(&actual);
        }
        let p = { self.policy.lock().checked("unavailable")?.installed(&q)? };
        if action == p::StateAction::RevokePublisher {
            self.revoke_shared_publisher(&p.manifest.publisher)?;
        }
        {
            let mut h = self.policy.lock().checked("unavailable")?;
            let mut s = h.selection(&q.plugin_id)?.ok_or(fail("not_found"))?;
            if s.digest == q.digest {
                s.enabled = false;
                s.epoch = s.epoch.checked_add(1).ok_or(fail("resource_limit"))?;
                h.save_selection(&q.plugin_id, &s)?;
                h.active.remove(&q.plugin_id);
                let marker = h.cache.join("selected").join(&q.plugin_id);
                if legacy::read_file(&marker, 64).is_ok_and(|v| v == q.digest.as_bytes()) {
                    std::fs::remove_file(marker).checked("unavailable")?;
                }
            }
            if action == p::StateAction::Remove {
                h.db()?
                    .execute("DELETE FROM policy_packages WHERE digest=?1", [&q.digest])
                    .checked("unavailable")?;
                let file = h.path(&p);
                if legacy::read_file(&file, legacy::MAX_ARCHIVE)
                    .is_ok_and(|b| legacy::hash(&b) == q.digest)
                {
                    std::fs::remove_file(file).checked("unavailable")?;
                }
            }
        }
        if action == p::StateAction::Remove {
            return Ok(p::PackageStatus {
                manifest: p.manifest,
                digest: p.digest,
                signing_key: p.signing_key,
                enabled: false,
                active: false,
                publisher_trusted: false,
                epoch: "0".into(),
            });
        }
        self.policy_status(&q)
    }
    pub(super) fn policy_context(
        &self,
        q: &p::PackageQuery,
        binding: Option<Binding>,
    ) -> Result<Context> {
        let (i, path) = self.policy_verified(q)?;
        let h = self.policy.lock().checked("unavailable")?;
        let before = h.selection(&q.plugin_id)?.ok_or(fail("plugin_disabled"))?;
        if !before.enabled
            || before.digest != q.digest
            || h.active.get(&q.plugin_id) != Some(&before.epoch)
        {
            return Err(fail("plugin_disabled"));
        }
        drop(h);
        let pubrev = self.full_publisher_revision(&i.manifest.publisher)?;
        Ok(Context {
            package: i,
            path,
            epoch: before.epoch,
            before,
            publisher_revision: pubrev,
            preflight: false,
            binding,
        })
    }
    pub(super) fn policy_check(&self, c: &Context) -> Result<()> {
        self.ensure_running()?;
        if self
            .full_publisher_key(&c.package.manifest.publisher)?
            .as_ref()
            != Some(&c.package.signing_key)
            || self.full_publisher_revision(&c.package.manifest.publisher)? != c.publisher_revision
        {
            return Err(fail("grant_revoked"));
        }
        let legacy_enabled: bool = self
            .db
            .lock()?
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM selections WHERE plugin_id=?1 AND enabled=1)",
                [&c.package.manifest.id],
                |r| r.get(0),
            )
            .checked("unavailable")?;
        if legacy_enabled {
            return Err(fail("capability_conflict"));
        }
        let h = self.policy.lock().checked("unavailable")?;
        if h.selection(&c.package.manifest.id)?.as_ref() != Some(&c.before)
            || (!c.preflight && h.active.get(&c.package.manifest.id) != Some(&c.epoch))
        {
            return Err(fail("plugin_disabled"));
        }
        if !c.preflight
            && !legacy::read_file(&h.cache.join("selected").join(&c.package.manifest.id), 64)
                .is_ok_and(|v| v == c.package.digest.as_bytes())
        {
            return Err(fail("grant_revoked"));
        }
        if let Some(b) = &c.binding {
            if b.instance != self.catalog_instance
                || b.epoch != c.epoch
                || b.publisher_revision != c.publisher_revision
            {
                return Err(fail("grant_revoked"));
            }
            let ctrl = h
                .consistent_control(&b.status.store_id, &b.status.service_id)?
                .ok_or(fail("invalid_record"))?;
            let current = h
                .binding(
                    &b.status.store_id,
                    &b.status.service_id,
                    &b.status.service_version_id,
                )?
                .ok_or(fail("invalid_record"))?;
            if ctrl.authority != b.control_authority || current.status.revision != b.status.revision
            {
                return Err(fail("grant_revoked"));
            }
        }
        Ok(())
    }
}

// Bounded metadata and temporary files; installed package history is never pruned here.
fn prune_reviews(h: &store::Host) -> Result<()> {
    super::super::full::store::directory(&h.cache)?;
    super::super::full::store::directory(&h.cache.join("staging"))?;
    let mut statement = h
        .db()?
        .prepare("SELECT id,record FROM policy_reviews ORDER BY rowid LIMIT 65")
        .checked("unavailable")?;
    let rows = statement
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .checked("unavailable")?
        .collect::<std::result::Result<Vec<_>, _>>()
        .checked("unavailable")?;
    if rows.len() > 64 {
        return Err(fail("resource_limit"));
    }
    let mut removed = 0;
    for (id, text) in rows {
        let review: ReviewRecord = store::decode(&text)?;
        store::uuid(&id)?;
        if review.review.package_ref != id {
            return Err(fail("invalid_record"));
        }
        let expired = chrono::DateTime::parse_from_rfc3339(&review.created).map_or(true, |t| {
            t < chrono::Utc::now() - chrono::Duration::minutes(10)
        });
        if (!review.done && !expired) || removed >= 8 {
            continue;
        }
        let file = h.cache.join("staging").join(format!("{id}.laplug"));
        match std::fs::symlink_metadata(&file) {
            Ok(_) => {
                let bytes = legacy::read_file(&file, legacy::MAX_ARCHIVE)?;
                if legacy::hash(&bytes) != review.review.digest {
                    return Err(fail("stale_snapshot"));
                }
                std::fs::remove_file(&file).checked("unavailable")?;
                std::fs::File::open(file.parent().ok_or(fail("invalid_record"))?)
                    .checked("unavailable")?
                    .sync_all()
                    .checked("unavailable")?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(fail("unavailable")),
        }
        h.db()?
            .execute("DELETE FROM policy_reviews WHERE id=?1", [id])
            .checked("unavailable")?;
        removed += 1;
    }
    Ok(())
}
#[cfg(test)]
mod review_tests {
    use super::*;
    fn insert(h: &store::Host, done: bool, expired: bool, payload: &[u8]) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let manifest: p::Manifest = serde_json::from_str(include_str!(
            "../../../../../../sdk/plugins/examples/pure-policy/package/manifest.json"
        ))
        .unwrap();
        let record = ReviewRecord {
            review: p::Review {
                package_ref: id.clone(),
                digest: legacy::hash(payload),
                manifest,
                signing_key: "1".repeat(64),
                trusted_publisher: false,
                compressed_bytes: payload.len().to_string(),
                expanded_bytes: payload.len().to_string(),
            },
            publisher_revision: 0,
            created: (chrono::Utc::now() - chrono::Duration::minutes(if expired { 11 } else { 0 }))
                .to_rfc3339(),
            done,
        };
        h.db()
            .unwrap()
            .execute(
                "INSERT INTO policy_reviews VALUES(?1,?2)",
                params![id, store::encode(&record).unwrap()],
            )
            .unwrap();
        store::write_private(
            &h.cache.join("staging").join(format!("{id}.laplug")),
            payload,
        )
        .unwrap();
        id
    }
    #[test]
    fn expired_and_completed_reviews_are_reclaimed_in_bounded_batches_but_current_previews_survive()
    {
        let temp = tempfile::tempdir().unwrap();
        let h = store::Host::new(temp.path());
        super::super::super::full::store::directory(&h.cache).unwrap();
        super::super::super::full::store::directory(&h.cache.join("staging")).unwrap();
        let current = insert(&h, false, false, b"current");
        for n in 0..16 {
            insert(&h, n % 2 == 0, true, b"expired");
        }
        prune_reviews(&h).unwrap();
        let count: i64 = h
            .db()
            .unwrap()
            .query_row("SELECT count(*) FROM policy_reviews", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 9);
        prune_reviews(&h).unwrap();
        let count: i64 = h
            .db()
            .unwrap()
            .query_row("SELECT count(*) FROM policy_reviews", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
        assert!(
            h.cache
                .join("staging")
                .join(format!("{current}.laplug"))
                .is_file()
        );
    }
    #[test]
    fn altered_temporary_file_is_retained_and_never_silently_deleted() {
        let temp = tempfile::tempdir().unwrap();
        let h = store::Host::new(temp.path());
        super::super::super::full::store::directory(&h.cache).unwrap();
        super::super::super::full::store::directory(&h.cache.join("staging")).unwrap();
        let id = insert(&h, true, true, b"original");
        let file = h.cache.join("staging").join(format!("{id}.laplug"));
        std::fs::write(&file, b"changed").unwrap();
        assert_eq!(prune_reviews(&h).unwrap_err().code, "stale_snapshot");
        assert_eq!(std::fs::read(file).unwrap(), b"changed");
    }
}
