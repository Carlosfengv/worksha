use super::super::{Checked, Manager, Result, fail};
use super::{
    runtime,
    store::{ConsentReview, Lease},
};
use local_agent_core::{domain as d, plugin::full as f};
use rusqlite::params;

impl Manager {
    pub(in crate::plugin_host::external) fn full_prepare_consent(
        &self,
        q: &d::SourceConsentQuery,
    ) -> Result<d::SourceConsentReview> {
        let query = f::PackageQuery {
            plugin_id: q.plugin_id.clone(),
            digest: q.digest.clone(),
        };
        let status = self.full_status(&query)?;
        if !status.runtime_active || !status.source_authorized {
            return Err(fail("permission_denied"));
        }
        let mut host = self.full.lock().checked("unavailable")?;
        let selected = host.selection(&q.plugin_id)?;
        let grant = host
            .grants
            .get(&q.plugin_id)
            .cloned()
            .ok_or(fail("permission_denied"))?;
        if grant.lease.digest != q.digest
            || grant.lease.epoch != status.activation_epoch.value()
            || grant.lease.revision != status.grant_revision.value()
        {
            return Err(fail("grant_revoked"));
        }
        host.consents
            .retain(|_, r| r.created.elapsed().as_secs() < 300);
        if host.consents.len() >= 64 {
            let oldest = host
                .consents
                .iter()
                .min_by_key(|(_, r)| r.created)
                .map(|(id, _)| id.clone())
                .ok_or(fail("resource_limit"))?;
            host.consents.remove(&oldest);
        }
        let reply = d::SourceConsentReview {
            review_ref: uuid::Uuid::new_v4().to_string(),
            plugin_id: q.plugin_id.clone(),
            digest: q.digest.clone(),
            resource_label: status.source_label,
            source_authorized: true,
            unified_authorized: grant.unified,
            persist_metadata: selected.persist_metadata,
        };
        host.consents.insert(
            reply.review_ref.clone(),
            ConsentReview {
                reply: reply.clone(),
                grant,
                persist: selected.persist_metadata,
                created: std::time::Instant::now(),
                applied: None,
            },
        );
        Ok(reply)
    }
    pub(in crate::plugin_host::external) fn full_consent(
        &self,
        input: &d::SourceConsent,
    ) -> Result<()> {
        self.ensure_running()?;
        if !input.enabled && input.persist_metadata {
            return Err(fail("invalid_input"));
        }
        let query = f::PackageQuery {
            plugin_id: input.plugin_id.clone(),
            digest: input.digest.clone(),
        };
        let installed = self.full.lock().checked("unavailable")?.installed(&query)?;
        if input.enabled && !self.full_trusted(&installed)? {
            return Err(fail("publisher_untrusted"));
        }
        let publisher_revision = self.full_publisher_revision(&installed.manifest.publisher)?;
        let mut host = self.full.lock().checked("unavailable")?;
        let mut selected = host.selection(&input.plugin_id)?;
        if selected.digest != input.digest {
            return Err(fail("grant_revoked"));
        }
        let active = host.active.get(&input.plugin_id).cloned();
        let grant = host.grants.get(&input.plugin_id).cloned();
        let review = if input.enabled {
            let id = &input.review_ref;
            if id.is_empty() {
                return Err(fail("invalid_input"));
            }
            let review = host
                .consents
                .get(id)
                .cloned()
                .ok_or(fail("stale_snapshot"))?;
            if review.created.elapsed().as_secs() >= 300
                || review.reply.plugin_id != input.plugin_id
                || review.reply.digest != input.digest
            {
                return Err(fail("stale_snapshot"));
            }
            let current = grant.as_ref().ok_or(fail("permission_denied"))?;
            runtime::stamp(current)?;
            let lease = active.as_ref().ok_or(fail("plugin_disabled"))?;
            if lease.publisher_revision != publisher_revision
                || current.lease != *lease
                || lease.epoch != selected.epoch
                || lease.revision != selected.revision
                || !selected.enabled
            {
                return Err(fail("grant_revoked"));
            }
            if let Some((applied, enabled, persist)) = &review.applied {
                if applied == lease
                    && *enabled == input.enabled
                    && *persist == input.persist_metadata
                    && current.unified == *enabled
                    && selected.persist_metadata == *persist
                {
                    return Ok(());
                }
                return Err(fail("stale_snapshot"));
            }
            if &review.grant != current || review.persist != selected.persist_metadata {
                return Err(fail("stale_snapshot"));
            }
            Some(review)
        } else {
            None
        };
        selected.advance()?;
        selected.persist_metadata = input.persist_metadata;
        let updated = active.map(|lease| Lease {
            epoch: selected.epoch,
            revision: selected.revision,
            ..lease
        });
        let tx = host.db()?.unchecked_transaction().checked("unavailable")?;
        // Persist only metadata from the same selected resource. Candidate paths are stripped.
        if input.persist_metadata {
            if let (Some(projection), Some(grant)) =
                (host.projections.get(&input.plugin_id), grant.as_ref())
                && runtime::stamp(grant).is_ok_and(|stamp| stamp == projection.source_stamp)
            {
                let mut saved = (*projection.facts).clone();
                for record in &mut saved.records {
                    if let d::facts::FactCandidate::Project(project) = &mut record.value {
                        project.path_hint = None;
                    }
                }
                tx.execute(
                    "INSERT OR REPLACE INTO full_projections VALUES(?1,?2)",
                    params![input.plugin_id, super::store::encode(&saved)?],
                )
                .checked("unavailable")?;
            }
        } else {
            tx.execute(
                "DELETE FROM full_projections WHERE plugin_id=?1",
                [&input.plugin_id],
            )
            .checked("unavailable")?;
        }
        tx.execute(
            "INSERT OR REPLACE INTO full_selections VALUES(?1,?2)",
            params![input.plugin_id, super::store::encode(&selected)?],
        )
        .checked("unavailable")?;
        tx.commit().checked("unavailable")?;
        if let Some(lease) = updated {
            host.active.insert(input.plugin_id.clone(), lease.clone());
            if let Some(mut grant) = grant {
                grant.lease = lease.clone();
                grant.unified = input.enabled;
                host.grants.insert(input.plugin_id.clone(), grant);
            }
            if let Some(projection) = host.projections.get_mut(&input.plugin_id) {
                projection.lease = lease.clone();
                projection.captured_source = None;
            }
            if let Some(review) = review
                && let Some(current) = host.consents.get_mut(&review.reply.review_ref)
            {
                current.applied = Some((lease, input.enabled, input.persist_metadata));
            }
        }
        host.failures.remove(&input.plugin_id);
        Ok(())
    }
}
