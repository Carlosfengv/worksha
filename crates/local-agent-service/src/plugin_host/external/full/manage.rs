use super::super::{Checked, Manager, Result, fail, package as legacy};
use super::{
    package,
    store::{self, Installed},
};
use local_agent_core::{domain as d, plugin::full as f};
use rusqlite::{OptionalExtension, params};
use std::{io::Write, path::Path};

impl Manager {
    pub(in crate::plugin_host::external) fn full_query(
        &self,
        plugin: &str,
        digest: &str,
    ) -> Result<bool> {
        match self.installed(plugin, digest) {
            Ok(_) => Ok(false),
            Err(e) if e.code == "not_found" => {
                self.full
                    .lock()
                    .checked("unavailable")?
                    .installed(&f::PackageQuery {
                        plugin_id: plugin.into(),
                        digest: digest.into(),
                    })?;
                Ok(true)
            }
            Err(e) => Err(e),
        }
    }
    pub(in crate::plugin_host::external) fn full_publisher_revision(
        &self,
        publisher: &str,
    ) -> Result<u64> {
        self.full_publisher_revisions
            .lock()
            .checked("unavailable")?
            .get(publisher)
            .copied()
            .unwrap_or(Some(0))
            .ok_or(fail("grant_revoked"))
    }
    pub(in crate::plugin_host::external) fn full_revoke_publisher(&self, publisher: &str) {
        // Never wait for the optional store's filesystem work during legacy revocation.
        // A poisoned map makes subsequent source lease checks fail closed.
        let mut revisions = self
            .full_publisher_revisions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let revision = revisions.entry(publisher.into()).or_insert(Some(0));
        *revision = revision.and_then(|n| n.checked_add(1));
    }
    pub(in crate::plugin_host::external) fn full_publisher_key(
        &self,
        publisher: &str,
    ) -> Result<Option<String>> {
        self.db
            .lock()?
            .query_row(
                "SELECT public_key FROM publishers WHERE id=?1",
                [publisher],
                |r| r.get(0),
            )
            .optional()
            .checked("unavailable")
    }
    pub(in crate::plugin_host::external) fn revoke_shared_publisher(
        &self,
        publisher: &str,
    ) -> Result<()> {
        let db = self.db.lock()?;
        db.execute("DELETE FROM publishers WHERE id=?1", [publisher])
            .checked("unavailable")?;
        // Keep the revision change under the same lock as the trust deletion, so an
        // install which already started cannot silently restore revoked trust.
        self.full_revoke_publisher(publisher);
        Ok(())
    }
    pub(super) fn full_trusted(&self, installed: &Installed) -> Result<bool> {
        Ok(self
            .full_publisher_key(&installed.manifest.publisher)?
            .as_ref()
            == Some(&installed.signing_key))
    }
    pub(super) fn full_status(&self, query: &f::PackageQuery) -> Result<f::Status> {
        self.ensure_running()?;
        let (p, s, active, grant, freshness, projection, failure) = {
            let host = self.full.lock().checked("unavailable")?;
            let p = host.installed(query)?;
            let s = host.selection(&query.plugin_id)?;
            let active = host
                .active
                .get(&query.plugin_id)
                .filter(|l| {
                    l.digest == query.digest && l.epoch == s.epoch && l.revision == s.revision
                })
                .cloned();
            let grant = host
                .grants
                .get(&query.plugin_id)
                .filter(|g| Some(&g.lease) == active.as_ref())
                .cloned();
            let freshness = if let Some(v) = host
                .projections
                .get(&query.plugin_id)
                .filter(|v| Some(&v.lease) == active.as_ref())
            {
                if v.collected.elapsed().as_secs() <= 300 {
                    d::Freshness::Current
                } else {
                    d::Freshness::Stale
                }
            } else {
                let saved: bool = host
                    .db()?
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM full_projections WHERE plugin_id=?1)",
                        [&query.plugin_id],
                        |r| r.get(0),
                    )
                    .checked("unavailable")?;
                if saved {
                    d::Freshness::Stale
                } else {
                    d::Freshness::Unread
                }
            };
            let projection = host
                .projections
                .get(&query.plugin_id)
                .filter(|v| Some(&v.lease) == active.as_ref())
                .cloned();
            let failure = host
                .failures
                .get(&query.plugin_id)
                .filter(|(lease, _)| Some(lease) == active.as_ref())
                .map(|(_, code)| *code);
            (p, s, active, grant, freshness, projection, failure)
        };
        let trusted = self.full_trusted(&p)?;
        let selected = s.digest == query.digest;
        let marker_matches = legacy::read_file(
            &self
                .root
                .with_file_name("plugin-sources")
                .join("selected")
                .join(&query.plugin_id),
            64,
        )
        .is_ok_and(|bytes| bytes == query.digest.as_bytes());
        let revision = self.full_publisher_revision(&p.manifest.publisher).ok();
        let runtime_active = selected
            && s.enabled
            && marker_matches
            && trusted
            && active
                .as_ref()
                .is_some_and(|l| Some(l.publisher_revision) == revision);
        let current_stamp = grant.as_ref().and_then(|g| super::runtime::stamp(g).ok());
        let source_authorized = runtime_active && current_stamp.is_some();
        let freshness = if failure.is_some() {
            d::Freshness::Failed
        } else if (!source_authorized
            || projection
                .as_ref()
                .is_some_and(|p| Some(&p.source_stamp) != current_stamp.as_ref()))
            && freshness == d::Freshness::Current
        {
            d::Freshness::Stale
        } else {
            freshness
        };
        Ok(f::Status {
            manifest: p.manifest,
            digest: p.digest,
            signing_key: p.signing_key,
            enabled: selected && s.enabled,
            runtime_active,
            publisher_trusted: trusted,
            activation_epoch: s.epoch.into(),
            grant_revision: s.revision.into(),
            source_authorized,
            source_label: grant
                .as_ref()
                .filter(|_| source_authorized)
                .map(|g| g.path.display().to_string()),
            unified_authorized: source_authorized && grant.as_ref().is_some_and(|g| g.unified),
            persist_metadata: selected && s.persist_metadata,
            freshness,
            fact_summary: projection.filter(|_| source_authorized).map(|p| p.summary),
            problem: if !trusted {
                Some("publisher_untrusted".into())
            } else if selected && s.enabled && !marker_matches {
                Some("grant_revoked".into())
            } else if let Some(code) = failure {
                Some(code.into())
            } else if selected && s.enabled && !runtime_active {
                Some("plugin_disabled".into())
            } else {
                None
            },
        })
    }
    pub(super) fn full_list(&self) -> Result<Vec<f::Status>> {
        let packages = self.full.lock().checked("unavailable")?.packages()?;
        packages
            .into_iter()
            .map(|p| {
                self.full_status(&f::PackageQuery {
                    plugin_id: p.manifest.id,
                    digest: p.digest,
                })
            })
            .collect()
    }
    pub(super) fn full_inspect(&self, path: &Path) -> Result<f::Review> {
        self.ensure_running()?;
        let bytes = legacy::read_file(path, legacy::MAX_ARCHIVE)?;
        let p = package::inspect(&bytes)?;
        let trusted = self.full_publisher_key(&p.manifest.publisher)?;
        if trusted.as_ref().is_some_and(|key| key != &p.signing_key) {
            return Err(fail("publisher_changed"));
        }
        let review = f::Review {
            package_ref: uuid::Uuid::new_v4().to_string(),
            digest: p.digest,
            manifest: p.manifest,
            signing_key: p.signing_key.clone(),
            trusted_publisher: trusted.as_ref() == Some(&p.signing_key),
            compressed_bytes: (bytes.len() as u64).into(),
            expanded_bytes: p.expanded.into(),
        };
        let host = self.full.lock().checked("unavailable")?;
        let db = host.db()?;
        let now = chrono::Utc::now().timestamp_millis();
        let mut expired = db
            .prepare("SELECT id FROM full_reviews WHERE expires<?1")
            .checked("unavailable")?;
        let ids = expired
            .query_map([now], |r| r.get::<_, String>(0))
            .checked("unavailable")?
            .collect::<std::result::Result<Vec<_>, _>>()
            .checked("unavailable")?;
        drop(expired);
        for id in ids {
            uuid::Uuid::parse_str(&id).checked("invalid_record")?;
            let file = host.root.join("staging").join(format!("{id}.laplug"));
            match std::fs::remove_file(file) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(fail("unavailable")),
            }
        }
        db.execute("DELETE FROM full_reviews WHERE expires<?1", [now])
            .checked("unavailable")?;
        let mut statement = db
            .prepare("SELECT record FROM full_reviews LIMIT 65")
            .checked("unavailable")?;
        let existing = statement
            .query_map([], |r| r.get::<_, String>(0))
            .checked("unavailable")?
            .collect::<std::result::Result<Vec<_>, _>>()
            .checked("unavailable")?;
        let mut total = bytes.len() as u64;
        if existing.len() >= 64 {
            return Err(fail("resource_limit"));
        }
        for r in existing {
            let r: f::Review = store::decode(&r)?;
            total = total.saturating_add(r.compressed_bytes.value());
        }
        if total > 128 * 1_048_576 {
            return Err(fail("resource_limit"));
        }
        // The full-host lock serializes staging writes; no install lock is needed
        // for this independent, existing 128 MiB staging allowance.
        super::super::installation::check_staging(
            [&self.root, &host.root],
            super::super::installation::HostKind::History,
            bytes.len() as u64,
        )?;
        let staged = host
            .root
            .join("staging")
            .join(format!("{}.laplug", review.package_ref));
        use std::os::unix::fs::OpenOptionsExt;
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&staged)
            .checked("unavailable")?;
        if out.write_all(&bytes).and_then(|_| out.sync_all()).is_err() {
            let _ = std::fs::remove_file(&staged);
            return Err(fail("unavailable"));
        }
        if db
            .execute(
                "INSERT INTO full_reviews VALUES(?1,?2,?3)",
                params![review.package_ref, store::encode(&review)?, now + 300_000],
            )
            .is_err()
        {
            let _ = std::fs::remove_file(staged);
            return Err(fail("unavailable"));
        }
        Ok(review)
    }
    pub(super) fn full_install(&self, request: &f::Install) -> Result<f::Status> {
        self.ensure_running()?;
        let _installation = self.installation.try_lock().checked("busy")?;
        uuid::Uuid::parse_str(&request.package_ref).checked("invalid_input")?;
        let (review, staged) = {
            let host = self.full.lock().checked("unavailable")?;
            let text: String = host
                .db()?
                .query_row(
                    "SELECT record FROM full_reviews WHERE id=?1 AND expires>=?2",
                    params![request.package_ref, chrono::Utc::now().timestamp_millis()],
                    |r| r.get(0),
                )
                .checked("stale_snapshot")?;
            let review: f::Review = store::decode(&text)?;
            (
                review,
                host.root
                    .join("staging")
                    .join(format!("{}.laplug", request.package_ref)),
            )
        };
        if review.digest != request.digest || review.signing_key != request.expected_signing_key {
            return Err(fail("stale_snapshot"));
        }
        let publisher_revision = self.full_publisher_revision(&review.manifest.publisher)?;
        let bytes = legacy::read_file(&staged, legacy::MAX_ARCHIVE)?;
        let checked = package::inspect(&bytes)?;
        if checked.digest != review.digest
            || checked.manifest != review.manifest
            || checked.signing_key != review.signing_key
        {
            return Err(fail("signature_invalid"));
        }
        let trusted = self.full_publisher_key(&checked.manifest.publisher)?;
        if trusted.as_ref().is_some_and(|k| k != &checked.signing_key) {
            return Err(fail("publisher_changed"));
        }
        if trusted.is_none() && !request.approve_publisher {
            return Err(fail("publisher_untrusted"));
        }
        let p = Installed {
            manifest: checked.manifest,
            digest: checked.digest,
            signing_key: checked.signing_key,
            compressed_bytes: bytes.len() as u64,
            expanded_bytes: checked.expanded,
        };
        let (mut full_inventory, full_root, target) = {
            let full = self.full.try_lock().checked("busy")?;
            (
                full.installation_inventory()?,
                full.cache_root().to_owned(),
                full.package_path(&p),
            )
        };
        full_inventory.extend(self.policy_inventory()?);
        {
            let db = self.db.lock()?;
            super::super::installation::check(
                &db,
                &full_inventory,
                &super::super::installation::Entry {
                    host_kind: super::super::installation::HostKind::History,
                    id: p.manifest.id.clone(),
                    version: p.manifest.version.clone(),
                    digest: p.digest.clone(),
                    publisher: p.manifest.publisher.clone(),
                    key: Some(p.signing_key.clone()),
                    bytes: p.compressed_bytes,
                },
            )?;
            super::super::installation::check_archives(
                [&self.root, &full_root],
                &target,
                bytes.len() as u64,
            )?;
        }
        {
            let host = self.full.lock().checked("unavailable")?;
            host.publish_package(&p, &bytes)?;
        }
        // This is the one shared publisher trust table. Installation never enables a source.
        let db = self.db.lock()?;
        if self.full_publisher_revision(&p.manifest.publisher)? != publisher_revision {
            return Err(fail("grant_revoked"));
        }
        let current: Option<String> = db
            .query_row(
                "SELECT public_key FROM publishers WHERE id=?1",
                [&p.manifest.publisher],
                |r| r.get(0),
            )
            .optional()
            .checked("unavailable")?;
        if current.as_ref().is_some_and(|key| key != &p.signing_key) {
            return Err(fail("publisher_changed"));
        }
        if current.is_none() && !request.approve_publisher {
            return Err(fail("publisher_untrusted"));
        }
        if current.is_none() {
            db.execute(
                "INSERT OR IGNORE INTO publishers VALUES(?1,?2)",
                params![p.manifest.publisher, p.signing_key],
            )
            .checked("unavailable")?;
        }
        let current: String = db
            .query_row(
                "SELECT public_key FROM publishers WHERE id=?1",
                [&p.manifest.publisher],
                |r| r.get(0),
            )
            .checked("unavailable")?;
        if current != p.signing_key {
            return Err(fail("publisher_changed"));
        }
        drop(db);
        self.full_status(&f::PackageQuery {
            plugin_id: p.manifest.id,
            digest: p.digest,
        })
    }
}
