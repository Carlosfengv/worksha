use super::super::{Checked, Manager, Result, fail};
use super::{
    runtime::{self, Context, Job, Output},
    store::{Grant, Lease, Projection},
};
use local_agent_core::{
    domain as d,
    plugin::{external as common, full as f},
};
use rusqlite::params;
use std::{path::Path, sync::Arc};

impl Manager {
    pub(super) async fn full_change(self: &Arc<Self>, input: f::StateChange) -> Result<f::Status> {
        if !matches!(
            input.action,
            f::StateAction::Enable | f::StateAction::Rollback
        ) {
            let manager = self.clone();
            return super::super::run_sync(move || manager.full_stop_change(input)).await;
        }
        let gate = self.gate(&input.package.plugin_id)?;
        let _gate = gate.try_lock_owned().checked("busy")?;
        let _activation = self.activation.clone().try_lock_owned().checked("busy")?;
        let manager = self.clone();
        let query = input.package.clone();
        let rollback = matches!(input.action, f::StateAction::Rollback);
        let context = super::super::run_sync(move || {
            let before = manager
                .full
                .lock()
                .checked("unavailable")?
                .selection(&query.plugin_id)?;
            if rollback && before.digest != query.digest {
                return Err(fail("grant_revoked"));
            }
            let digest = if rollback {
                before.previous.clone().ok_or(fail("not_found"))?
            } else {
                query.digest
            };
            let (p, path) = manager.full_verified(&f::PackageQuery {
                plugin_id: query.plugin_id.clone(),
                digest,
            })?;
            let mut after = before.clone();
            after.advance()?;
            let lease = Lease {
                id: query.plugin_id,
                digest: p.digest.clone(),
                publisher: p.manifest.publisher.clone(),
                epoch: after.epoch,
                revision: after.revision,
                publisher_revision: manager.full_publisher_revision(&p.manifest.publisher)?,
            };
            let context = Context {
                package: p,
                path,
                before,
                lease,
                preflight: true,
                id: uuid::Uuid::new_v4().to_string(),
                resource: uuid::Uuid::new_v4().to_string(),
                source: None,
            };
            manager.full_check(&context)?;
            Ok(context)
        })
        .await?;
        if !rollback {
            let manager = self.clone();
            let query = input.package.clone();
            let status = super::super::run_sync(move || manager.full_status(&query)).await?;
            if status.enabled && status.runtime_active {
                return Ok(status);
            }
        }
        if !matches!(
            self.full_run(context.clone(), Job::Preflight).await?,
            Output::Ready
        ) {
            return Err(fail("invalid_record"));
        }
        let manager = self.clone();
        super::super::run_sync(move || {
            manager.full_check(&context)?;
            let mut host = manager.full.lock().checked("unavailable")?;
            let mut selected = host.selection(&context.lease.id)?;
            if selected != context.before {
                return Err(fail("grant_revoked"));
            }
            selected.previous =
                host.previous_for_activation(&context.lease.id, &selected, &context.lease.digest)?;
            selected.digest = context.lease.digest.clone();
            selected.epoch = context.lease.epoch;
            selected.revision = context.lease.revision;
            selected.enabled = true;
            selected.persist_metadata = false;
            host.save_selection_and_clear_projection(&context.lease.id, &selected)?;
            host.grants.remove(&context.lease.id);
            host.failures.remove(&context.lease.id);
            host.projections.remove(&context.lease.id);
            host.active
                .insert(context.lease.id.clone(), context.lease.clone());
            drop(host);
            manager.full_status(&f::PackageQuery {
                plugin_id: context.lease.id,
                digest: context.lease.digest,
            })
        })
        .await
    }
    fn full_stop_change(&self, input: f::StateChange) -> Result<f::Status> {
        self.ensure_running()?;
        let p = self
            .full
            .lock()
            .checked("unavailable")?
            .installed(&input.package)?;
        let trusted = self.full_trusted(&p).unwrap_or(false);
        let mut selected = self
            .full
            .lock()
            .checked("unavailable")?
            .selection(&input.package.plugin_id)?;
        if selected.digest != input.package.digest
            && !matches!(
                input.action,
                f::StateAction::Remove | f::StateAction::RevokePublisher
            )
        {
            return Err(fail("grant_revoked"));
        }
        if matches!(input.action, f::StateAction::RevokePublisher) {
            self.revoke_shared_publisher(&p.manifest.publisher)?;
            if let Some(old) = self
                .list()?
                .into_iter()
                .find(|s| s.manifest.publisher == p.manifest.publisher)
            {
                let installed = self.installed(&old.manifest.id, &old.digest)?;
                self.change_synchronous(
                    common::StateRequest {
                        plugin_id: old.manifest.id,
                        digest: old.digest,
                        action: common::StateAction::RevokePublisher,
                    },
                    installed,
                )?;
            }
        }
        let mut host = self.full.lock().checked("unavailable")?;
        if matches!(input.action, f::StateAction::RevokePublisher) {
            let ids: std::collections::BTreeSet<_> = host
                .packages()?
                .into_iter()
                .filter(|v| v.manifest.publisher == p.manifest.publisher)
                .map(|v| v.manifest.id)
                .collect();
            for id in ids {
                let mut current = host.selection(&id)?;
                let current_package = host.installed(&f::PackageQuery {
                    plugin_id: id.clone(),
                    digest: current.digest.clone(),
                })?;
                if current_package.manifest.publisher != p.manifest.publisher {
                    continue;
                }
                current.advance()?;
                current.enabled = false;
                current.persist_metadata = false;
                host.save_selection_and_clear_projection(&id, &current)?;
                host.active.remove(&id);
                host.grants.remove(&id);
                host.projections.remove(&id);
                host.failures.remove(&id);
            }
        } else if matches!(input.action, f::StateAction::Remove)
            && selected.digest != input.package.digest
        {
            if selected.previous.as_ref() == Some(&input.package.digest) {
                selected.previous = None;
            }
            host.remove_unselected_package(
                &input.package.plugin_id,
                &input.package.digest,
                &selected,
            )?;
        } else {
            selected.advance()?;
            if matches!(input.action, f::StateAction::RevokeGrants) {
                if let Some(active) = host.active.get_mut(&input.package.plugin_id) {
                    active.epoch = selected.epoch;
                    active.revision = selected.revision;
                }
            } else {
                selected.enabled = false;
                host.active.remove(&input.package.plugin_id);
            }
            selected.persist_metadata = false;
            if matches!(input.action, f::StateAction::Remove) {
                host.remove_selected_package(
                    &input.package.plugin_id,
                    &input.package.digest,
                    &selected,
                )?;
            } else {
                host.save_selection_and_clear_projection(&input.package.plugin_id, &selected)?;
            }
            host.grants.remove(&input.package.plugin_id);
            host.projections.remove(&input.package.plugin_id);
            host.failures.remove(&input.package.plugin_id);
        }
        if matches!(input.action, f::StateAction::Remove) {
            host.remove_archive(&p)?;
            drop(host);
            Ok(f::Status {
                manifest: p.manifest,
                digest: p.digest,
                signing_key: p.signing_key,
                enabled: false,
                runtime_active: false,
                publisher_trusted: trusted,
                activation_epoch: selected.epoch.into(),
                grant_revision: selected.revision.into(),
                source_authorized: false,
                source_label: None,
                unified_authorized: false,
                persist_metadata: false,
                freshness: d::Freshness::Unread,
                fact_summary: None,
                problem: None,
            })
        } else {
            drop(host);
            self.full_status(&input.package)
        }
    }
    pub(in crate::plugin_host::external) fn full_grant_source(
        &self,
        q: &f::PackageQuery,
        path: &Path,
    ) -> Result<f::Status> {
        use std::os::unix::fs::MetadataExt;
        let _gate = self.gate(&q.plugin_id)?.try_lock_owned().checked("busy")?;
        let (p, package_path) = self.full_verified(q)?;
        let before = self
            .full
            .lock()
            .checked("unavailable")?
            .selection(&q.plugin_id)?;
        let lease = self
            .full
            .lock()
            .checked("unavailable")?
            .active
            .get(&q.plugin_id)
            .cloned()
            .ok_or(fail("plugin_disabled"))?;
        let check = Context {
            package: p,
            path: package_path,
            before: before.clone(),
            lease: lease.clone(),
            preflight: false,
            id: uuid::Uuid::new_v4().to_string(),
            resource: uuid::Uuid::new_v4().to_string(),
            source: None,
        };
        self.full_check(&check)?;
        if path.extension().is_none_or(|e| e != "jsonl") {
            return Err(fail("unsupported"));
        }
        let metadata = std::fs::symlink_metadata(path).checked("permission_denied")?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.nlink() != 1
            || metadata.len() > common::MAX_SOURCE
        {
            return Err(fail("permission_denied"));
        }
        let path = path.canonicalize().checked("permission_denied")?;
        let mut selected = before.clone();
        selected.advance()?;
        selected.persist_metadata = false;
        let lease = Lease {
            epoch: selected.epoch,
            revision: selected.revision,
            ..lease
        };
        let grant = Grant {
            lease: lease.clone(),
            path,
            device: metadata.dev(),
            inode: metadata.ino(),
            unified: false,
        };
        runtime::stamp(&grant)?;
        self.full_check(&check)?;
        let mut host = self.full.lock().checked("unavailable")?;
        if host.selection(&q.plugin_id)? != before {
            return Err(fail("grant_revoked"));
        }
        host.save_selection_and_clear_projection(&q.plugin_id, &selected)?;
        host.active.insert(q.plugin_id.clone(), lease);
        host.grants.insert(q.plugin_id.clone(), grant);
        host.projections.remove(&q.plugin_id);
        host.failures.remove(&q.plugin_id);
        drop(host);
        self.full_status(q)
    }
    pub(in crate::plugin_host::external) async fn full_refresh(
        self: &Arc<Self>,
        q: f::PackageQuery,
    ) -> Result<f::Status> {
        let gate = self.gate(&q.plugin_id)?;
        let _gate = gate.try_lock_owned().checked("busy")?;
        let manager = self.clone();
        let query = q.clone();
        let expected = super::super::run_sync(move || {
            Ok(manager
                .full
                .lock()
                .checked("unavailable")?
                .active
                .get(&query.plugin_id)
                .filter(|lease| lease.digest == query.digest)
                .cloned())
        })
        .await?;
        let result = self.full_refresh_current(q.clone()).await;
        if let Err(error) = &result
            && let Some(lease) = expected
        {
            let manager = self.clone();
            let code = error.code;
            super::super::run_sync(move || {
                let mut host = manager.full.lock().checked("unavailable")?;
                if host.active.get(&q.plugin_id) == Some(&lease) {
                    host.failures.insert(q.plugin_id, (lease, code));
                }
                Ok(())
            })
            .await?;
        }
        result
    }
    async fn full_refresh_current(self: &Arc<Self>, q: f::PackageQuery) -> Result<f::Status> {
        let manager = self.clone();
        let query = q.clone();
        let context = super::super::run_sync(move || {
            let (p, path) = manager.full_verified(&query)?;
            let (before, lease, grant) = {
                let host = manager.full.lock().checked("unavailable")?;
                (
                    host.selection(&query.plugin_id)?,
                    host.active
                        .get(&query.plugin_id)
                        .cloned()
                        .ok_or(fail("plugin_disabled"))?,
                    host.grants
                        .get(&query.plugin_id)
                        .cloned()
                        .ok_or(fail("permission_denied"))?,
                )
            };
            let mut c = Context {
                package: p,
                path,
                before,
                lease,
                preflight: false,
                id: uuid::Uuid::new_v4().to_string(),
                resource: uuid::Uuid::new_v4().to_string(),
                source: None,
            };
            manager.full_check(&c)?;
            c.source = Some(runtime::load_source(&grant)?);
            manager.full_check(&c)?;
            Ok(c)
        })
        .await?;
        let output = self.full_run(context.clone(), Job::Scan).await;
        let Output::Facts(facts) = output? else {
            return Err(fail("invalid_record"));
        };
        let manager = self.clone();
        super::super::run_sync(move || {
            manager.full_check(&context)?;
            let live_snapshot = context
                .package
                .manifest
                .source
                .as_ref()
                .is_some_and(|source| source.max_age_seconds.is_some());
            let source = context.source.ok_or(fail("permission_denied"))?;
            if live_snapshot {
                runtime::verify_live_frame(&source, facts.records.len())?;
            }
            let encoded = super::store::encode(&facts)?;
            let size = encoded.len();
            let summary = facts
                .summary(&facts.records)
                .map_err(|_| fail("resource_limit"))?;
            let mut host = manager.full.lock().checked("unavailable")?;
            if host.active.get(&q.plugin_id) != Some(&context.lease) {
                return Err(fail("grant_revoked"));
            }
            if facts.coverage != d::Coverage::Complete
                && host.projections.contains_key(&q.plugin_id)
                && !live_snapshot
            {
                host.failures.insert(
                    q.plugin_id.clone(),
                    (context.lease.clone(), "partial_source"),
                );
                return Err(fail("partial_source"));
            }
            let bytes = host
                .projections
                .iter()
                .filter(|(id, _)| *id != &q.plugin_id)
                .map(|(_, p)| p.resident_bytes())
                .sum::<usize>();
            let source_stamp = source.stamp.clone();
            let captured_source = (live_snapshot && source.grant.unified).then_some(source);
            let projection = Projection {
                lease: context.lease,
                facts: Arc::new(facts),
                collected: std::time::Instant::now(),
                encoded_bytes: size,
                summary,
                source_stamp,
                captured_source,
            };
            if bytes.saturating_add(projection.resident_bytes()) > 64 * 1_048_576 {
                return Err(fail("resource_limit"));
            }
            if host.selection(&q.plugin_id)?.persist_metadata {
                let mut saved = (*projection.facts).clone();
                for record in &mut saved.records {
                    if let d::facts::FactCandidate::Project(project) = &mut record.value {
                        project.path_hint = None;
                    }
                }
                host.db()?
                    .execute(
                        "INSERT OR REPLACE INTO full_projections VALUES(?1,?2)",
                        params![q.plugin_id, super::store::encode(&saved)?],
                    )
                    .checked("unavailable")?;
            }
            host.projections.insert(q.plugin_id.clone(), projection);
            host.failures.remove(&q.plugin_id);
            drop(host);
            manager.full_status(&q)
        })
        .await
    }
}
