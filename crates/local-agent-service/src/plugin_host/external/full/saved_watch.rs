use super::super::{Checked, Manager, Result, fail, package as legacy, run_sync};
use super::{
    package::can_persist_watch,
    runtime::{self, Context},
    store::{Grant, Installed, Lease, SavedWatch, decode},
};
use local_agent_core::plugin::full as f;
use rusqlite::{Connection, OptionalExtension, params};
use std::sync::Arc;

/// Publisher trust has a distinct identity for each approval, including
/// approval of the same key after revocation.
pub(super) fn publisher_ticket(
    db: &Connection,
    package: &Installed,
    create: bool,
) -> Result<Option<String>> {
    let publisher = &package.manifest.publisher;
    if create {
        db.execute(
            "INSERT OR IGNORE INTO ext_host_publisher_source_tickets
             SELECT id,?3 FROM ext_host_publishers WHERE id=?1 AND public_key=?2",
            params![
                publisher,
                package.signing_key,
                uuid::Uuid::new_v4().to_string()
            ],
        )
        .checked("unavailable")?;
    }
    db.query_row(
        "SELECT ticket FROM ext_host_publisher_source_tickets
         JOIN ext_host_publishers ON publisher=id WHERE id=?1 AND public_key=?2",
        params![publisher, package.signing_key],
        |row| row.get(0),
    )
    .optional()
    .checked("unavailable")
}

impl Manager {
    pub(super) fn invalidate_changed_watch(&self, grant: &Grant) -> Result<()> {
        if !grant.persist_watch {
            return Ok(());
        }
        let mut host = self.full.lock().checked("unavailable")?;
        let id = &grant.lease.id;
        if host.grants.get(id) != Some(grant) {
            return Ok(());
        }
        let db = host.db()?;
        let tx = db.unchecked_transaction().checked("unavailable")?;
        tx.execute(
            "DELETE FROM ext_source_full_saved_sources WHERE plugin_id=?1",
            [id],
        )
        .checked("unavailable")?;
        tx.execute(
            "DELETE FROM ext_source_full_projections WHERE plugin_id=?1",
            [id],
        )
        .checked("unavailable")?;
        tx.commit().checked("unavailable")?;
        drop(db);
        host.grants.remove(id);
        host.projections.remove(id);
        host.failures
            .insert(id.clone(), (grant.lease.clone(), "grant_revoked"));
        Ok(())
    }

    pub(crate) async fn restore_saved_sources(
        self: &Arc<Self>,
        intake: Arc<crate::plugin_host::intake::Intake>,
    ) -> Result<()> {
        let manager = self.clone();
        let ids = run_sync(move || {
            // Earlier persisted display consent implied a watch only for one
            // source. New authority always has an explicit true marker.
            let db = manager.db.lock()?;
            db.execute(
                "DELETE FROM ext_source_full_saved_sources
                 WHERE CASE WHEN json_valid(record) THEN json_type(record,'$.persistWatch') IS NOT 'true' ELSE 1 END",
                [],
            ).checked("unavailable")?;
            let mut stmt = db.prepare(
                "SELECT plugin_id FROM ext_source_full_saved_sources ORDER BY plugin_id LIMIT 65",
            ).checked("unavailable")?;
            let ids = stmt.query_map([], |row| row.get::<_,String>(0))
                .checked("unavailable")?.collect::<rusqlite::Result<Vec<_>>>().checked("unavailable")?;
            if ids.len() > 64 { return Err(fail("resource_limit")); }
            Ok(ids)
        }).await?;
        for id in ids {
            let gate = self.gate(&id)?.lock_owned().await;
            let manager = self.clone();
            let restored = run_sync(move || manager.restore_saved_watch(&id)).await;
            drop(gate);
            match restored {
                Ok(Some(query)) => match self.watch_source(query.clone(), intake.clone()) {
                    Ok(()) => {
                        let _ = self.full_refresh(query).await;
                    }
                    Err(error) => eprintln!("会话来源的监听未启动（{}）。", error.code),
                },
                Ok(None) => (),
                Err(error) => eprintln!("会话来源未恢复（{}）。", error.code),
            }
        }
        Ok(())
    }

    fn restore_saved_watch(&self, id: &str) -> Result<Option<f::PackageQuery>> {
        let saved_record = self.full.lock().checked("unavailable")?.saved_watch(id)?;
        let Some(record) = saved_record else {
            return Ok(None);
        };
        let restore =
            decode::<SavedWatch>(&record).and_then(|saved| self.restore_watch(id, &saved));
        match restore {
            Ok(query) => Ok(Some(query)),
            Err(error)
                if matches!(
                    error.code,
                    "grant_revoked"
                        | "not_found"
                        | "invalid_record"
                        | "permission_denied"
                        | "publisher_untrusted"
                        | "signature_invalid"
                ) =>
            {
                let host = self.full.lock().checked("unavailable")?;
                host.db()?.execute(
                    "DELETE FROM ext_source_full_saved_sources WHERE plugin_id=?1 AND record=?2",
                    params![id, record],
                ).checked("unavailable")?;
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    fn restore_watch(&self, id: &str, saved: &SavedWatch) -> Result<f::PackageQuery> {
        let selected = self.full.lock().checked("unavailable")?.selection(id)?;
        if !saved.persist_watch
            || !selected.enabled
            || selected.digest != saved.digest
            || selected.epoch != saved.epoch
            || selected.revision != saved.revision
        {
            return Err(fail("grant_revoked"));
        }
        let query = f::PackageQuery {
            plugin_id: id.into(),
            digest: saved.digest.clone(),
        };
        let (package, path) = self.full_verified(&query)?;
        if !can_persist_watch(&package.manifest) || package.manifest.publisher != saved.publisher {
            return Err(fail("grant_revoked"));
        }
        let lease = Lease {
            id: query.plugin_id.clone(),
            digest: query.digest.clone(),
            publisher: saved.publisher.clone(),
            epoch: selected.epoch,
            revision: selected.revision,
            publisher_revision: self.full_publisher_revision(&saved.publisher)?,
        };
        let grant = Grant {
            lease: lease.clone(),
            path: saved.path.clone(),
            device: saved.device,
            inode: saved.inode,
            created: Some(saved.created),
            unified: true,
            persist_watch: true,
            connection: None,
        };
        let context = Context {
            package,
            path,
            before: selected,
            lease: lease.clone(),
            preflight: true,
            id: uuid::Uuid::new_v4().to_string(),
            resource: uuid::Uuid::new_v4().to_string(),
            source: None,
        };
        self.full_check(&context)?;
        let mut host = self.full.lock().checked("unavailable")?;
        let db = self.db.lock()?;
        if publisher_ticket(&db, &context.package, false)?.as_ref() != Some(&saved.publisher_ticket)
            || self.full_publisher_revision(&saved.publisher)? != lease.publisher_revision
            || host.selection(id)? != context.before
            || host.active.contains_key(id)
            || !legacy::read_file(&host.root.join("selected").join(id), 64)
                .is_ok_and(|bytes| bytes == saved.digest.as_bytes())
        {
            return Err(fail("grant_revoked"));
        }
        runtime::stamp(&grant)?;
        host.active.insert(id.into(), lease);
        host.grants.insert(id.into(), grant);
        Ok(query)
    }
}
