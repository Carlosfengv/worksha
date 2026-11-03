//! Client hashes and non-secret operation receipts share one atomic publication.
use super::{Client, Entry, operations::Outcome};
use local_agent_core::providers::{ConfigError, files};
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::Arc};
use tokio::sync::Semaphore;

#[derive(Clone)]
pub(super) struct Registry {
    pub entries: Vec<Entry>,
    pub store_id: Option<String>,
    pub operations: Vec<Outcome>,
    legacy_bytes: Option<Vec<u8>>,
    readable: bool,
    stamp: files::Stamp,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    version: u32,
    store_id: String,
    clients: Vec<Client>,
    operations: Vec<Outcome>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Marker {
    version: u32,
    store_id: String,
}
fn marker(directory: &Path) -> Result<Option<String>, ConfigError> {
    files::read_protected(&directory.join("operations-v1.json"))?
        .bytes
        .map(|bytes| {
            let m: Marker = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
            if m.version != 1 || !super::operations::uuid4(&m.store_id) {
                return Err(invalid());
            }
            Ok(m.store_id)
        })
        .transpose()
}
pub(super) fn invalid() -> ConfigError {
    ConfigError::new(
        "invalid_record",
        "客户端记录不可读取，原文件已保留；请核查原操作",
    )
}
impl Registry {
    pub fn healthy(&self) -> Result<(), ConfigError> {
        if self.readable {
            Ok(())
        } else {
            Err(invalid())
        }
    }
    pub fn ensure_store(&self, id: &str) -> Result<(), ConfigError> {
        self.healthy()?;
        if self.store_id.as_deref().is_some_and(|stored| stored != id) {
            return Err(ConfigError::new(
                "operation_store_changed",
                "客户端记录不属于当前数据库，未更换来源或清空原记录",
            ));
        }
        Ok(())
    }
}
pub(super) fn load(directory: &Path) -> Result<Registry, ConfigError> {
    let marker = marker(directory)?;
    let snapshot = files::read_protected(&directory.join("clients.json"))?;
    let bytes = snapshot.bytes;
    let (clients, store_id, operations, legacy_bytes) = match bytes {
        None if marker.is_some() => return Err(invalid()),
        None => (vec![], None, vec![], None),
        Some(bytes) => {
            // Inspect the shape before decoding; malformed v2 never falls back to an empty v1.
            let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
            if value.is_array() {
                let clients = serde_json::from_value(value).map_err(|_| invalid())?;
                (clients, marker.clone(), vec![], Some(bytes))
            } else {
                let record: Record = serde_json::from_value(value).map_err(|_| invalid())?;
                if record.version != 2 || marker.as_deref() != Some(record.store_id.as_str()) {
                    return Err(invalid());
                }
                (
                    record.clients,
                    Some(record.store_id),
                    record.operations,
                    None,
                )
            }
        }
    };
    if clients.len() > 128 || operations.len() > clients.len() * 2 {
        return Err(invalid());
    }
    let mut ids = std::collections::HashSet::new();
    for client in &clients {
        super::validate_client(
            &client.name,
            &client.agent,
            &client.models,
            client.concurrency,
        )
        .map_err(|_| invalid())?;
        if uuid::Uuid::parse_str(&client.id).is_err()
            || !ids.insert(&client.id)
            || client.key_hash.len() != 64
            || !client.key_hash.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(invalid());
        }
    }
    let mut operation_ids = std::collections::HashSet::new();
    let mut actions = std::collections::HashSet::new();
    for outcome in &operations {
        outcome.validate().map_err(|_| invalid())?;
        if Some(outcome.request.store_id.as_str()) != store_id.as_deref()
            || !operation_ids.insert(&outcome.request.operation_id)
            || !actions.insert((
                outcome.receipt.client_id.as_str(),
                outcome.receipt.action.as_str(),
            ))
        {
            return Err(invalid());
        }
        let client = clients
            .iter()
            .find(|c| c.id == outcome.receipt.client_id)
            .ok_or_else(invalid)?;
        match &outcome.request.action {
            super::operations::Action::Create { draft } if !draft.matches(client) => {
                return Err(invalid());
            }
            super::operations::Action::Revoke { .. } if client.enabled => return Err(invalid()),
            _ => (),
        }
    }
    Ok(Registry {
        entries: clients
            .into_iter()
            .map(|client| Entry {
                capacity: Arc::new(Semaphore::new(client.concurrency)),
                client,
            })
            .collect(),
        store_id,
        operations,
        legacy_bytes,
        readable: true,
        stamp: snapshot.stamp,
    })
}
pub(super) fn commit(
    directory: &Path,
    current: &mut Registry,
    mut next: Registry,
) -> Result<(), ConfigError> {
    commit_using(directory, current, &mut next, files::atomic_write)
}
fn commit_using(
    directory: &Path,
    current: &mut Registry,
    next: &mut Registry,
    write: impl FnOnce(&Path, &[u8], u32) -> Result<(), ConfigError>,
) -> Result<(), ConfigError> {
    current.healthy()?;
    let snapshot = files::read_protected(&directory.join("clients.json"));
    if !snapshot.is_ok_and(|snapshot| snapshot.stamp == current.stamp) {
        current.readable = false;
        return Err(invalid());
    }
    let clients: Vec<_> = next.entries.iter().map(|e| e.client.clone()).collect();
    let bytes = if let Some(store_id) = &next.store_id {
        serde_json::to_vec(&Record {
            version: 2,
            store_id: store_id.clone(),
            clients,
            operations: next.operations.clone(),
        })
    } else {
        serde_json::to_vec(&clients)
    }
    .map_err(|_| invalid())?;
    if bytes.len() as u64 > files::LIMIT {
        return Err(ConfigError::new(
            "capacity",
            "客户端授权与回执已达 1 MiB 上限，原记录未覆盖",
        ));
    }
    let prepared = (|| -> Result<(), ConfigError> {
        if next.store_id.is_some()
            && let Some(original) = &current.legacy_bytes
        {
            let backup = directory.join(format!(
                "clients.pre-operations-v1-{}.json",
                files::hash(original)
            ));
            match files::read_protected(&backup)?.bytes {
                Some(saved) if saved != *original => return Err(invalid()),
                Some(_) => (),
                None => files::atomic_write(&backup, original, 0o600)?,
            }
        }
        if let Some(id) = &next.store_id {
            current.ensure_store(id)?;
            match marker(directory)? {
                Some(stored) if stored != *id => return Err(invalid()),
                Some(_) => (),
                None => {
                    // An empty first registry must exist before the migration marker. A failed
                    // first publication can then be retried without inventing a new identity.
                    let path = directory.join("clients.json");
                    if files::read_protected(&path)?.bytes.is_none() {
                        if !current.entries.is_empty() || current.store_id.is_some() {
                            return Err(invalid());
                        }
                        files::atomic_write(&path, b"[]", 0o600)?;
                    }
                    let bytes = serde_json::to_vec(&Marker {
                        version: 1,
                        store_id: id.clone(),
                    })
                    .map_err(|_| invalid())?;
                    files::atomic_write(&directory.join("operations-v1.json"), &bytes, 0o600)?;
                }
            }
        }
        Ok(())
    })();
    let written = prepared.and_then(|()| write(&directory.join("clients.json"), &bytes, 0o600));
    let expected = files::hash(&bytes);
    // Re-read after every publication attempt, including marker / backup errors.
    // Keep live semaphore permits while adopting the verified on-disk authorization.
    match load(directory) {
        Ok(mut actual) => {
            for entry in &mut actual.entries {
                if let Some(old) = current.entries.iter().find(|e| {
                    e.client.id == entry.client.id
                        && e.client.concurrency == entry.client.concurrency
                }) {
                    entry.capacity = old.capacity.clone();
                }
            }
            *current = actual;
        }
        Err(_) => current.readable = false,
    }
    if written.is_err()
        || !current.readable
        || current.stamp.digest.as_deref() != Some(expected.as_str())
    {
        // A rename may have succeeded before directory fsync failed. Read back the actual
        // registry rather than rolling memory back to possibly revoked authorization.
        return Err(ConfigError::new(
            "gateway_result_unknown",
            "客户端写入结果尚未确认，请保留原操作编号核查；未重新签发密钥",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn fixture() -> (tempfile::TempDir, std::path::PathBuf, Registry) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let directory = root.join("gateway-lan");
        files::private_dir(&directory).unwrap();
        let registry = load(&directory).unwrap();
        (temp, directory, registry)
    }
    fn client() -> Entry {
        Entry {
            capacity: Arc::new(Semaphore::new(1)),
            client: Client {
                id: uuid::Uuid::new_v4().to_string(),
                name: "fixture".into(),
                agent: "claude".into(),
                provider_id: "anthropic".into(),
                models: vec!["fixture".into()],
                concurrency: 1,
                enabled: true,
                created_at: "2026-10-27T00:00:00Z".into(),
                key_hash: "a".repeat(64),
            },
        }
    }
    #[test]
    fn legacy_upgrade_preserves_exact_backup_and_refuses_old_decoder() {
        let (_temp, dir, mut current) = fixture();
        let mut legacy = current.clone();
        legacy.entries.push(client());
        commit(&dir, &mut current, legacy).unwrap();
        let before = std::fs::read(dir.join("clients.json")).unwrap();
        let mut next = current.clone();
        next.store_id = Some(uuid::Uuid::new_v4().to_string());
        commit(&dir, &mut current, next).unwrap();
        let backup = dir.join(format!(
            "clients.pre-operations-v1-{}.json",
            files::hash(&before)
        ));
        assert_eq!(std::fs::read(&backup).unwrap(), before);
        assert_eq!(
            std::fs::metadata(backup).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let after = std::fs::read(dir.join("clients.json")).unwrap();
        assert!(serde_json::from_slice::<Vec<Client>>(&after).is_err());
        assert_eq!(
            load(&dir).unwrap().entries[0].client.key_hash,
            "a".repeat(64)
        );
        assert!(
            current
                .ensure_store(&uuid::Uuid::new_v4().to_string())
                .is_err()
        );
    }
    #[test]
    fn first_publication_failure_retains_bound_empty_registry_for_retry() {
        let (_temp, dir, mut current) = fixture();
        let id = uuid::Uuid::new_v4().to_string();
        let mut next = current.clone();
        next.store_id = Some(id.clone());
        next.entries.push(client());
        let e = commit_using(&dir, &mut current, &mut next, |_, _, _| Err(invalid())).unwrap_err();
        assert_eq!(e.code, "gateway_result_unknown");
        let reopened = load(&dir).unwrap();
        assert!(reopened.entries.is_empty());
        assert_eq!(reopened.store_id.as_deref(), Some(id.as_str()));
        commit(&dir, &mut current, next).unwrap();
        assert_eq!(load(&dir).unwrap().entries.len(), 1);
    }
    #[test]
    fn rename_then_sync_error_reads_committed_revocation_and_preserves_held_slots() {
        let (_temp, dir, mut current) = fixture();
        let mut next = current.clone();
        next.entries.push(client());
        commit(&dir, &mut current, next).unwrap();
        let _held = current.entries[0]
            .capacity
            .clone()
            .try_acquire_owned()
            .unwrap();
        let mut next = current.clone();
        next.entries[0].client.enabled = false;
        let e = commit_using(&dir, &mut current, &mut next, |path, bytes, mode| {
            files::atomic_write(path, bytes, mode)?;
            Err(invalid())
        })
        .unwrap_err();
        assert_eq!(e.code, "gateway_result_unknown");
        assert!(!current.entries[0].client.enabled);
        assert_eq!(current.entries[0].capacity.available_permits(), 0);
        assert!(!load(&dir).unwrap().entries[0].client.enabled);
    }
    #[test]
    fn unreadable_after_write_failure_fails_closed_without_restoring_old_auth() {
        let (_temp, dir, mut current) = fixture();
        let mut next = current.clone();
        next.entries.push(client());
        commit(&dir, &mut current, next).unwrap();
        let mut next = current.clone();
        next.entries[0].client.enabled = false;
        assert!(
            commit_using(&dir, &mut current, &mut next, |path, _, _| {
                files::atomic_write(path, b"{", 0o600)?;
                Err(invalid())
            })
            .is_err()
        );
        assert!(current.healthy().is_err());
        assert!(load(&dir).is_err());
        assert_eq!(std::fs::read(dir.join("clients.json")).unwrap(), b"{");
    }
    #[test]
    fn migrated_registry_or_marker_loss_and_malformed_v2_never_recreate_identity() {
        let (_temp, dir, mut current) = fixture();
        let mut next = current.clone();
        next.store_id = Some(uuid::Uuid::new_v4().to_string());
        commit(&dir, &mut current, next).unwrap();
        let path = dir.join("clients.json");
        let marker_path = dir.join("operations-v1.json");
        let registry = std::fs::read(&path).unwrap();
        let marker = std::fs::read(&marker_path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(load(&dir).is_err());
        assert!(!path.exists());
        files::atomic_write(&path, &registry, 0o600).unwrap();
        std::fs::remove_file(&marker_path).unwrap();
        assert!(load(&dir).is_err());
        assert!(!marker_path.exists());
        files::atomic_write(&marker_path, &marker, 0o600).unwrap();
        let mut v: serde_json::Value = serde_json::from_slice(&registry).unwrap();
        v["version"] = 99.into();
        let bad = serde_json::to_vec(&v).unwrap();
        files::atomic_write(&path, &bad, 0o600).unwrap();
        assert!(load(&dir).is_err());
        assert_eq!(std::fs::read(path).unwrap(), bad);
    }

    #[test]
    fn persisted_operation_identity_and_draft_corruption_preserve_bytes() {
        let (_temp, dir, mut current) = fixture();
        let entry = client();
        let store_id = uuid::Uuid::new_v4().to_string();
        let outcome: Outcome = serde_json::from_value(serde_json::json!({
            "request":{"storeId":store_id,"operationId":entry.client.id,
                "action":{"kind":"create","draft":{"name":"fixture","agent":"claude","providerId":"anthropic","models":["fixture"],"concurrency":1}}},
            "receipt":{"apiVersion":1,"storeId":store_id,"operationId":entry.client.id,"action":"create","clientId":entry.client.id,
                "listen":"127.0.0.1:43210","completedAt":"2026-10-27T00:00:00Z"}
        })).unwrap();
        let mut next = current.clone();
        next.entries.push(entry);
        next.operations.push(outcome);
        next.store_id = Some(store_id);
        commit(&dir, &mut current, next).unwrap();
        let path = dir.join("clients.json");
        let original: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        for property in ["operation", "draft", "listen", "duplicate", "unknown"] {
            let mut record = original.clone();
            match property {
                "operation" => {
                    record["operations"][0]["receipt"]["operationId"] =
                        uuid::Uuid::new_v4().to_string().into()
                }
                "draft" => {
                    record["operations"][0]["request"]["action"]["draft"]["name"] = "changed".into()
                }
                "listen" => record["operations"][0]["receipt"]["listen"] = "8.8.8.8:443".into(),
                "duplicate" => {
                    let entry = record["operations"][0].clone();
                    record["operations"].as_array_mut().unwrap().push(entry);
                }
                _ => record["operations"][0]["request"]["secret"] = "unexpected".into(),
            }
            let bytes = serde_json::to_vec(&record).unwrap();
            files::atomic_write(&path, &bytes, 0o600).unwrap();
            assert!(load(&dir).is_err(), "{property}");
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn externally_removed_or_changed_registry_is_not_recreated_from_memory() {
        for removed in [true, false] {
            let (_temp, dir, mut current) = fixture();
            let mut next = current.clone();
            next.entries.push(client());
            commit(&dir, &mut current, next).unwrap();
            let next = current.clone();
            let path = dir.join("clients.json");
            if removed {
                std::fs::remove_file(&path).unwrap();
            } else {
                files::atomic_write(&path, b"{", 0o600).unwrap();
            }
            assert_eq!(
                commit(&dir, &mut current, next).unwrap_err().code,
                "invalid_record"
            );
            assert!(current.healthy().is_err());
            if removed {
                assert!(!path.exists());
            } else {
                assert_eq!(std::fs::read(path).unwrap(), b"{");
            }
        }
    }
}
