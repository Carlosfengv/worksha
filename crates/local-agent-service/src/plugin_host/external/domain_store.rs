//! Separate opt-in domain state. Creating Manager performs no domain database I/O.
use super::{Cached, Checked, Result, fail};
use local_agent_core::{domain as d, plugin::external::SessionCandidate};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Consent {
    pub digest: String,
    pub epoch: u64,
    pub revision: u64,
    pub enabled: bool,
    pub persist: bool,
    pub identity: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Projection {
    observed_at: String,
    digest: String,
    epoch: String,
    revision: String,
    snapshot: String,
    source_revision: String,
    sessions: Vec<SessionCandidate>,
}
#[derive(Clone)]
pub(crate) struct FrozenQuery {
    pub query: d::UnifiedSessionQuery,
    pub signature: String,
    pub as_of: String,
    pub cutoff: Option<i64>,
    pub created: Instant,
}
#[derive(Clone)]
pub(super) struct FrozenFacts {
    pub query: d::facts::query::FactQuery,
    pub signature: String,
    pub as_of: String,
    pub created: Instant,
}
#[derive(Clone)]
pub(super) struct ConsentReview {
    pub plugin_id: String,
    pub digest: String,
    pub epoch: u64,
    pub revision: u64,
    pub consent_identity: Option<String>,
    pub created: Instant,
    pub applied: Option<(bool, bool)>,
}
pub(super) struct Host {
    path: PathBuf,
    db: Option<Connection>,
    pub queries: BTreeMap<String, FrozenQuery>,
    pub facts: BTreeMap<String, FrozenFacts>,
    pub catalog_signature: String,
    pub catalog_revision: u64,
    pub persistence_errors: BTreeMap<String, String>,
    pub reviews: BTreeMap<String, ConsentReview>,
}
impl Host {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            db: None,
            queries: BTreeMap::new(),
            facts: BTreeMap::new(),
            catalog_signature: String::new(),
            catalog_revision: 0,
            persistence_errors: BTreeMap::new(),
            reviews: BTreeMap::new(),
        }
    }
    fn db(&mut self, create: bool) -> Result<Option<&mut Connection>> {
        if self.db.is_none() {
            if !self.path.try_exists().checked("unavailable")? && !create {
                return Ok(None);
            }
            use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(create)
                .truncate(false)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&self.path)
                .checked("permission_denied")?;
            let metadata = file.metadata().checked("permission_denied")?;
            if !metadata.is_file() || metadata.nlink() != 1 {
                return Err(fail("permission_denied"));
            }
            let db = Connection::open(&self.path).checked("unavailable")?;
            db.busy_timeout(Duration::from_millis(200))
                .checked("unavailable")?;
            let exists:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='domain_version')",[],|r|r.get(0)).checked("invalid_record")?;
            if exists {
                super::store::validate_version(&db, "domain_version")?;
                let count: i64 = db
                    .query_row("SELECT count(*) FROM domain_version", [], |r| r.get(0))
                    .checked("invalid_record")?;
                if count != 1 {
                    return Err(fail("invalid_record"));
                }
            } else if metadata.len() != 0 || !create {
                return Err(fail("invalid_record"));
            }
            if create {
                db.execute_batch("PRAGMA journal_mode=WAL;
                    CREATE TABLE IF NOT EXISTS domain_version(version INTEGER NOT NULL);
                    INSERT INTO domain_version SELECT 1 WHERE NOT EXISTS(SELECT 1 FROM domain_version);
                    CREATE TABLE IF NOT EXISTS source_consents(plugin_id TEXT PRIMARY KEY,record TEXT NOT NULL);
                    CREATE TABLE IF NOT EXISTS source_projections(plugin_id TEXT PRIMARY KEY,record TEXT NOT NULL);").checked("unavailable")?;
            }
            self.db = Some(db);
        }
        Ok(self.db.as_mut())
    }
    pub fn consent(&mut self, id: &str) -> Result<Option<Consent>> {
        let Some(db) = self.db(false)? else {
            return Ok(None);
        };
        let text: Option<String> = db
            .query_row(
                "SELECT record FROM source_consents WHERE plugin_id=?1",
                [id],
                |r| r.get(0),
            )
            .optional()
            .checked("invalid_record")?;
        text.map(|s| serde_json::from_str(&s).checked("invalid_record"))
            .transpose()
    }
    pub fn grant(&mut self, id: &str, consent: &Consent) -> Result<()> {
        let db = self.db(true)?.ok_or(fail("unavailable"))?;
        let tx = db.transaction().checked("unavailable")?;
        tx.execute("INSERT INTO source_consents(plugin_id,record) VALUES(?1,?2) ON CONFLICT(plugin_id) DO UPDATE SET record=excluded.record",
            params![id,serde_json::to_string(consent).checked("invalid_input")?]).checked("unavailable")?;
        tx.execute("DELETE FROM source_projections WHERE plugin_id=?1", [id])
            .checked("unavailable")?;
        tx.commit().checked("unavailable")?;
        self.queries.clear();
        self.facts.clear();
        self.persistence_errors.remove(id);
        Ok(())
    }
    pub fn persist(&mut self, id: &str, c: &Cached) -> Result<()> {
        let record = Projection {
            observed_at: c.observed_at.clone(),
            digest: c.digest.clone(),
            epoch: c.epoch.clone(),
            revision: c.revision.clone(),
            snapshot: c.snapshot.clone(),
            source_revision: c.source_revision.clone().ok_or(fail("invalid_record"))?,
            sessions: c.sessions.clone(),
        };
        let text = serde_json::to_string(&record).checked("invalid_record")?;
        if text.len() > 16 * 1024 * 1024 {
            return Err(fail("resource_limit"));
        }
        self.db(true)?.ok_or(fail("unavailable"))?.execute("INSERT INTO source_projections(plugin_id,record) VALUES(?1,?2) ON CONFLICT(plugin_id) DO UPDATE SET record=excluded.record",params![id,text]).checked("unavailable")?;
        Ok(())
    }
    pub fn projection(&mut self, id: &str) -> Result<Option<Cached>> {
        let Some(db) = self.db(false)? else {
            return Ok(None);
        };
        let text:Option<String>=db.query_row("SELECT record FROM source_projections WHERE plugin_id=?1 AND length(record)<=16777216",[id],|r|r.get(0)).optional().checked("invalid_record")?;
        let Some(text) = text else { return Ok(None) };
        let p: Projection = serde_json::from_str(&text).checked("invalid_record")?;
        if p.sessions.len() > 10240
            || p.sessions
                .iter()
                .any(|s| super::broker::validate_session(s).is_err())
            || chrono::DateTime::parse_from_rfc3339(&p.observed_at).is_err()
        {
            return Err(fail("invalid_record"));
        }
        Ok(Some(Cached {
            observed_at: p.observed_at,
            report: None,
            collected_at: Instant::now()
                .checked_sub(Duration::from_secs(301))
                .ok_or(fail("unavailable"))?,
            digest: p.digest,
            epoch: p.epoch,
            revision: p.revision,
            snapshot: p.snapshot,
            sessions: p.sessions,
            source_revision: Some(p.source_revision),
            quota: None,
        }))
    }
    pub fn prune_queries(&mut self) {
        self.queries
            .retain(|_, q| q.created.elapsed() < Duration::from_secs(300));
        if self.queries.len() >= 64
            && let Some(id) = self
                .queries
                .iter()
                .min_by_key(|(_, q)| q.created)
                .map(|(id, _)| id.clone())
        {
            self.queries.remove(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn consent() -> Consent {
        Consent {
            digest: "a".repeat(64),
            epoch: 1,
            revision: 2,
            enabled: true,
            persist: true,
            identity: "scope-one".into(),
        }
    }
    #[test]
    fn domain_reads_are_lazy_and_consent_isolated_from_legacy_databases() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("domain-host.sqlite3");
        let mut host = Host::new(path.clone());
        assert!(host.consent("org.example.reader").unwrap().is_none());
        assert!(!path.exists());
        host.grant("org.example.reader", &consent()).unwrap();
        assert!(path.exists());
        drop(host);
        let mut host = Host::new(path);
        let restored = host.consent("org.example.reader").unwrap().unwrap();
        assert!(restored.enabled);
        assert!(restored.persist);
        assert_eq!(restored.identity, "scope-one");
        let mut revoked = consent();
        revoked.enabled = false;
        host.grant("org.example.reader", &revoked).unwrap();
        assert!(!host.consent("org.example.reader").unwrap().unwrap().enabled);
        assert!(!root.path().join("plugin-host.sqlite3").exists());
        assert!(!root.path().join("local-agent.sqlite3").exists());
    }
    #[test]
    fn domain_future_and_corrupt_stores_fail_closed_without_schema_writes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("domain-host.sqlite3");
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE domain_version(version INTEGER);INSERT INTO domain_version VALUES(2)",
        )
        .unwrap();
        drop(db);
        let original = std::fs::read(&path).unwrap();
        let mut host = Host::new(path.clone());
        assert_eq!(host.consent("x").err().unwrap().code, "incompatible_api");
        assert_eq!(
            host.grant("x", &consent()).err().unwrap().code,
            "incompatible_api"
        );
        assert_eq!(original, std::fs::read(&path).unwrap());
        std::fs::write(&path, b"not a database").unwrap();
        let mut host = Host::new(path.clone());
        assert_eq!(host.consent("x").err().unwrap().code, "invalid_record");
        assert_eq!(std::fs::read(path).unwrap(), b"not a database");
    }
}
