use super::super::{Checked, Manager, Result, fail};
use local_agent_core::plugin::full::PackageQuery;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConnectionAccess {
    pub directory: PathBuf,
    pub digest: String,
    pub grant: Option<String>,
}

impl Manager {
    pub(crate) fn agent_connection_gate(
        &self,
        agent: &str,
    ) -> Result<std::sync::Arc<tokio::sync::Mutex<()>>> {
        self.gate(&format!("agent-connection:{agent}"))
    }

    pub(crate) fn agent_connection_source_gate(
        &self,
        agent: &str,
    ) -> Result<std::sync::Arc<tokio::sync::Mutex<()>>> {
        let (plugin, _) = agent.split_once(':').ok_or(fail("unsupported"))?;
        self.gate(plugin)
    }

    pub(crate) fn agent_connection_access(
        &self,
        agent: &str,
        change: Option<bool>,
    ) -> Result<ConnectionAccess> {
        let (id, _) = agent.split_once(':').ok_or(fail("unsupported"))?;
        if !super::super::package::valid_id(id) {
            return Err(fail("invalid_input"));
        }
        let digest = self
            .full
            .lock()
            .checked("unavailable")?
            .selection(id)?
            .digest;
        let query = PackageQuery {
            plugin_id: id.into(),
            digest,
        };
        let status = self.full_status(&query)?;
        if !status.manifest.has_agent_connection()
            || !status
                .manifest
                .product_ids()
                .iter()
                .any(|product| product == agent)
        {
            return Err(fail("unsupported"));
        }
        if !status.runtime_active || !status.source_authorized {
            return Err(fail("permission_denied"));
        }
        if change.is_some() {
            self.full_verified(&query)?;
        }
        let mut host = self.full.lock().checked("unavailable")?;
        let lease = host
            .active
            .get(id)
            .filter(|lease| {
                lease.digest == query.digest
                    && lease.epoch == status.activation_epoch.value()
                    && lease.revision == status.grant_revision.value()
            })
            .cloned()
            .ok_or(fail("grant_revoked"))?;
        let grant = host
            .grants
            .get_mut(id)
            .filter(|grant| grant.lease == lease)
            .ok_or(fail("grant_revoked"))?;
        super::runtime::stamp(grant)?;
        let directory = grant
            .path
            .parent()
            .ok_or(fail("permission_denied"))?
            .to_path_buf();
        match change {
            Some(true) => {
                grant
                    .connection
                    .get_or_insert_with(|| uuid::Uuid::new_v4().to_string());
            }
            Some(false) => grant.connection = None,
            None => {}
        }
        Ok(ConnectionAccess {
            directory,
            digest: query.digest,
            grant: grant.connection.clone(),
        })
    }
}
