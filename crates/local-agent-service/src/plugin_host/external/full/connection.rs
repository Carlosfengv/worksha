use super::super::{Checked, Manager, Result, fail};
use local_agent_core::plugin::full::PackageQuery;
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

use crate::agent_connections::DSH_DESKTOP;

pub(in crate::plugin_host::external) struct BundledDshConnection {
    directory: PathBuf,
    digest: String,
    grant: Mutex<Option<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConnectionAccess {
    pub directory: PathBuf,
    pub digest: String,
    pub grant: Option<String>,
    pub bundled: bool,
}

impl Manager {
    pub(crate) fn with_bundled_dsh(mut self, home: &Path) -> Self {
        self.bundled_dsh = Some(BundledDshConnection {
            directory: home.join(".local/share/local-agent/dsh-desktop"),
            digest: local_agent_core::providers::files::hash(
                b"net.worksha.dsh-desktop:desktop/bundled-connection@1",
            ),
            grant: Mutex::new(None),
        });
        self
    }

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
        self.ensure_running()?;
        let Some(bundled) = self.bundled_dsh.as_ref().filter(|_| agent == DSH_DESKTOP) else {
            return self.source_agent_connection_access(agent, change);
        };
        let mut grant = bundled.grant.lock().checked("unavailable")?;
        if grant.is_none() {
            // Keep existing authorized source directories and their binding identities usable.
            match self.source_agent_connection_access(agent, None) {
                Ok(_) => return self.source_agent_connection_access(agent, change),
                Err(error)
                    if matches!(
                        error.code,
                        "not_found" | "permission_denied" | "grant_revoked" | "unsupported"
                    ) => {}
                Err(error) => return Err(error),
            }
        }
        update_grant(&mut grant, change);
        Ok(ConnectionAccess {
            directory: bundled.directory.clone(),
            digest: bundled.digest.clone(),
            grant: grant.clone(),
            bundled: true,
        })
    }

    fn source_agent_connection_access(
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
        update_grant(&mut grant.connection, change);
        Ok(ConnectionAccess {
            directory,
            digest: query.digest,
            grant: grant.connection.clone(),
            bundled: false,
        })
    }
}

fn update_grant(grant: &mut Option<String>, change: Option<bool>) {
    match change {
        Some(true) => {
            grant.get_or_insert_with(|| uuid::Uuid::new_v4().to_string());
        }
        Some(false) => *grant = None,
        None => {}
    }
}
