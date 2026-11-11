use super::super::{Checked, Manager, Result, fail};
use super::{dsh, runtime};
use local_agent_core::plugin::full as f;
use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

impl Manager {
    pub(crate) fn watch_source(self: &Arc<Self>, query: f::PackageQuery) -> Result<()> {
        if !self.full_query(&query.plugin_id, &query.digest)? {
            return Ok(());
        }
        let (interval, lease) = {
            let host = self.full.lock().checked("unavailable")?;
            let p = host.installed(&query)?;
            let Some(seconds) = p.manifest.source.as_ref().and_then(|s| s.refresh_seconds) else {
                return Ok(());
            };
            let lease = host
                .active
                .get(&query.plugin_id)
                .cloned()
                .ok_or(fail("plugin_disabled"))?;
            if !host
                .grants
                .get(&query.plugin_id)
                .is_some_and(|g| g.lease == lease)
            {
                return Err(fail("permission_denied"));
            }
            let interval = if query.plugin_id == dsh::PLUGIN {
                Duration::from_secs(1)
            } else {
                Duration::from_secs(seconds.into())
            };
            (interval, lease)
        };
        let weak = Arc::downgrade(self);
        let id = query.plugin_id.clone();
        let task = tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let Some(manager) = weak.upgrade() else {
                    break;
                };
                if manager.stopping.load(Ordering::Acquire) {
                    break;
                }
                let Some((grant, stamp)) = manager.full.lock().ok().and_then(|host| {
                    let grant = host.grants.get(&query.plugin_id).filter(|grant| {
                        grant.lease == lease && host.active.get(&query.plugin_id) == Some(&lease)
                    })?;
                    let stamp = host
                        .projections
                        .get(&query.plugin_id)
                        .filter(|p| {
                            p.lease == lease
                                && !host.failures.contains_key(&query.plugin_id)
                                && (!grant.unified || p.captured_source.is_some())
                        })
                        .map(|p| p.source_stamp.clone());
                    Some((grant.clone(), stamp))
                }) else {
                    break;
                };
                if query.plugin_id == dsh::PLUGIN
                    && stamp.as_ref().is_some_and(|stamp| {
                        runtime::stamp(&grant).is_ok_and(|current| &current == stamp)
                    })
                {
                    continue;
                }
                // Every scan rechecks the source identity and lease before publish.
                if let Err(error) = manager.full_refresh(query.clone()).await
                    && matches!(
                        error.code,
                        "grant_revoked"
                            | "permission_denied"
                            | "plugin_disabled"
                            | "publisher_untrusted"
                    )
                {
                    break;
                }
            }
        });
        if let Some(previous) = self
            .source_watches
            .lock()
            .checked("unavailable")?
            .insert(id, task)
        {
            previous.abort();
        }
        Ok(())
    }
}
