use super::{
    Checked, FileGrant, Lease, Manager, Query, Result, checked_bytes, fail, package,
    runtime::Worker,
};
use crate::plugin_host::external::run_sync;
use d::ConfigPlanner;
use local_agent_core::{
    Provider,
    domain::config as d,
    plugin::advanced::{BindingValue, ConfigCandidate, JsonEdit, MAX_DOCUMENT},
    providers::{files, fingerprint, vault},
};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

const MAX_PLANS: u32 = 64;

pub(super) fn capacity(db: &rusqlite::Connection) -> Result<Value> {
    let used: u32 = db
        .query_row("SELECT count(*) FROM plans", [], |r| r.get(0))
        .checked("unavailable")?;
    Ok(json!({"used":used,"limit":MAX_PLANS,"remaining":MAX_PLANS.saturating_sub(used)}))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Plan {
    id: String,
    plugin_id: String,
    digest: String,
    lease: Lease,
    grant: FileGrant,
    provider: Provider,
    edits: Vec<JsonEdit>,
    credential_pointers: Vec<String>,
    stamp: files::Stamp,
    state: String,
    created_at: String,
    after_hash: Option<String>,
    backup: Option<Vec<u8>>,
    backup_ref: Option<String>,
    after_stamp: Option<files::Stamp>,
}
impl Plan {
    fn can_discard(&self) -> bool {
        self.state == "prepared"
            && self.backup.is_none()
            && self.backup_ref.is_none()
            && self.after_hash.is_none()
            && self.after_stamp.is_none()
    }
    fn reusable_for(&self, next: &Self) -> bool {
        self.can_discard()
            && self.plugin_id == next.plugin_id
            && self.digest == next.digest
            && self.lease == next.lease
            && self.grant == next.grant
            && self.stamp == next.stamp
            && fingerprint(&self.provider) == fingerprint(&next.provider)
            && self.edits == next.edits
            && self.credential_pointers == next.credential_pointers
            && chrono::DateTime::parse_from_rfc3339(&self.created_at)
                .is_ok_and(|created| created >= chrono::Utc::now() - chrono::Duration::hours(24))
    }
    fn applied_mode(&self) -> u32 {
        if self.credential_pointers.is_empty() {
            self.stamp.mode
        } else {
            0o600
        }
    }
    pub(super) fn public(&self) -> Value {
        json!({"planId":self.id,"pluginId":self.plugin_id,"digest":self.digest,"path":self.grant.path,"revision":self.grant.hash,"providerId":self.provider.id,"providerName":self.provider.name,"state":self.state,"createdAt":self.created_at,"edits":self.edits,"backupAvailable":self.backup.is_some(),"canDiscard":self.can_discard(),"credentialPlaceholder":"${HOST_CREDENTIAL}"})
    }
}
impl Manager {
    pub(super) async fn prepare_config(
        self: &Arc<Self>,
        q: &Query,
        provider: &Provider,
    ) -> Result<Value> {
        let manager = self.clone();
        let query = q.clone();
        let provider = provider.clone();
        let (plan, document) =
            run_sync(move || manager.config_preview_snapshot(&query, &provider)).await?;
        let request = d::ConfigRequest {
            target_revision: plan.grant.hash.clone(),
            intent: d::ConfigIntent::JsonFields {
                edits: common_edits(&plan.edits)?,
            },
        };
        let planner = d::FiniteJsonPlanner;
        let d::ConfigWork::JsonProposal { edits } =
            planner.propose(&request).map_err(planning_error)?
        else {
            return Err(fail("permission_denied"));
        };
        let mut worker = Worker::open(self.clone(), q).await?;
        let candidate: ConfigCandidate = serde_json::from_value(
            worker
                .call(
                    "config.propose",
                    json!({"edits":edits,"credentialPlaceholder":d::CREDENTIAL_PLACEHOLDER}),
                )
                .await?,
        )
        .checked("invalid_input")?;
        let candidate = planner
            .inspect_result(
                &request,
                d::ConfigProposal {
                    request_identity: request.identity(),
                    output: d::ConfigOutput::JsonFields(common_edits(&candidate.edits)?),
                },
            )
            .map_err(planning_error)?;
        let d::ConfigOutput::JsonFields(edits) = candidate.output else {
            return Err(fail("permission_denied"));
        };
        let mut preview = document;
        for edit in edits {
            set(&mut preview, &edit.pointer, Value::String(edit.value))?;
        }
        if serde_json::to_vec(&preview).checked("invalid_input")?.len() > MAX_DOCUMENT as usize {
            return Err(fail("resource_limit"));
        }
        worker.close().await?;
        let manager = self.clone();
        run_sync(move || manager.commit_config_preview(plan)).await
    }
    fn config_preview_snapshot(&self, q: &Query, provider: &Provider) -> Result<(Plan, Value)> {
        let (_, policy, lease) = self.extension(q)?;
        let config = policy.config.ok_or(fail("unsupported"))?;
        let grant = self.extension_grant(q)?;
        let bytes = checked_bytes(&grant)?;
        let snapshot = files::read(&grant.path).checked("permission_denied")?;
        if snapshot.bytes.as_deref() != Some(bytes.as_slice()) {
            return Err(fail("stale_snapshot"));
        }
        let document: Value = serde_json::from_slice(&bytes).checked("invalid_input")?;
        if !document.is_object() {
            return Err(fail("invalid_input"));
        }
        let edits: Vec<JsonEdit> = config
            .fields
            .iter()
            .map(|f| JsonEdit {
                pointer: f.pointer.clone(),
                value: Value::String(match f.value {
                    BindingValue::Endpoint => provider.endpoint.clone(),
                    BindingValue::Model => provider.model.clone(),
                    BindingValue::Credential => "${HOST_CREDENTIAL}".into(),
                }),
            })
            .collect();
        let p = Plan {
            id: uuid::Uuid::new_v4().to_string(),
            plugin_id: q.plugin_id.clone(),
            digest: q.digest.clone(),
            lease,
            grant,
            provider: provider.clone(),
            edits,
            credential_pointers: config
                .fields
                .iter()
                .filter(|f| f.value == BindingValue::Credential)
                .map(|f| f.pointer.clone())
                .collect(),
            stamp: snapshot.stamp,
            state: "prepared".into(),
            created_at: chrono::Utc::now().to_rfc3339(),
            after_hash: None,
            backup: None,
            backup_ref: None,
            after_stamp: None,
        };
        Ok((p, document))
    }
    fn commit_config_preview(&self, p: Plan) -> Result<Value> {
        let gate = self.gate(&p.plugin_id)?;
        let _guard = gate.try_lock().checked("resource_limit")?;
        let current = files::read(&p.grant.path).checked("permission_denied")?;
        if current.stamp != p.stamp {
            return Err(fail("stale_snapshot"));
        }
        p.lease.check(self)?;
        let store = self.advanced.lock().checked("unavailable")?;
        let db = store.db()?;
        let mut stmt = db
            .prepare("SELECT id, record FROM plans WHERE plugin_id=?1 ORDER BY rowid DESC LIMIT 64")
            .checked("unavailable")?;
        let mut rows = stmt.query([&p.plugin_id]).checked("unavailable")?;
        let mut reused = None;
        while let Some(row) = rows.next().checked("unavailable")? {
            let id: String = row.get(0).checked("unavailable")?;
            let text: String = row.get(1).checked("unavailable")?;
            let old: Plan = serde_json::from_str(&text).checked("invalid_record")?;
            if old.id != id || old.plugin_id != p.plugin_id {
                return Err(fail("invalid_record"));
            }
            if reused.is_none() && old.reusable_for(&p) {
                reused = Some(old.public());
            }
        }
        if let Some(public) = reused {
            return Ok(public);
        }
        let count: u32 = store
            .db()?
            .query_row("SELECT count(*) FROM plans", [], |r| r.get(0))
            .checked("unavailable")?;
        if count >= MAX_PLANS {
            return Err(fail("plan_capacity_full"));
        }
        store
            .db()?
            .execute(
                "INSERT INTO plans VALUES(?1,?2,?3)",
                params![
                    p.id,
                    p.plugin_id,
                    serde_json::to_string(&p).checked("invalid_input")?
                ],
            )
            .checked("unavailable")?;
        Ok(p.public())
    }
    fn config_plan(&self, q: &Query, id: &str) -> Result<Plan> {
        self.advanced.lock().checked("unavailable")?.ready()?;
        let text: Option<String> = self
            .advanced
            .lock()
            .checked("unavailable")?
            .db()?
            .query_row("SELECT record FROM plans WHERE id=?1", [id], |r| r.get(0))
            .optional()
            .checked("unavailable")?;
        let p: Plan =
            serde_json::from_str(&text.ok_or(fail("not_found"))?).checked("invalid_record")?;
        if p.id != id {
            return Err(fail("invalid_record"));
        }
        if p.plugin_id != q.plugin_id || p.digest != q.digest {
            return Err(fail("permission_denied"));
        }
        Ok(p)
    }
    /// Host-only removal of an unused preview. Never touches a target file or the vault.
    pub(super) fn discard_config(&self, q: &Query, id: &str) -> Result<Value> {
        let gate = self.gate(&q.plugin_id)?;
        let _guard = gate.try_lock().checked("resource_limit")?;
        let store = self.advanced.lock().checked("unavailable")?;
        let text: Option<String> = store
            .db()?
            .query_row("SELECT record FROM plans WHERE id=?1", [id], |r| r.get(0))
            .optional()
            .checked("unavailable")?;
        let text = text.ok_or(fail("not_found"))?;
        let plan: Plan = serde_json::from_str(&text).checked("invalid_record")?;
        if plan.id != id {
            return Err(fail("invalid_record"));
        }
        if plan.plugin_id != q.plugin_id || plan.digest != q.digest {
            return Err(fail("permission_denied"));
        }
        if !plan.can_discard() {
            return Err(fail("result_pending"));
        }
        let changed = store
            .db()?
            .execute(
                "DELETE FROM plans WHERE id=?1 AND plugin_id=?2 AND record=?3",
                params![id, q.plugin_id, text],
            )
            .checked("unavailable")?;
        if changed != 1 {
            return Err(fail("not_found"));
        }
        Ok(json!({"planId":id,"state":"discarded"}))
    }
    pub(super) fn config_provider_id(&self, q: &Query, id: &str) -> Result<String> {
        Ok(self.config_plan(q, id)?.provider.id)
    }
    fn save_config_plan(&self, p: &Plan) -> Result<()> {
        self.advanced
            .lock()
            .checked("unavailable")?
            .db()?
            .execute(
                "UPDATE plans SET record=?2 WHERE id=?1",
                params![p.id, serde_json::to_string(p).checked("invalid_input")?],
            )
            .checked("unavailable")?;
        Ok(())
    }
    pub(super) fn apply_config(
        &self,
        q: &Query,
        id: &str,
        revision: &str,
        provider: &Provider,
    ) -> Result<Value> {
        let gate = self.gate(&q.plugin_id)?;
        let _guard = gate.try_lock().checked("resource_limit")?;
        let mut plan = self.config_plan(q, id)?;
        if ["applied", "restored"].contains(&plan.state.as_str()) {
            return Ok(plan.public());
        }
        plan.lease.check(self)?;
        if revision != plan.grant.hash || fingerprint(provider) != fingerprint(&plan.provider) {
            return Err(fail("stale_snapshot"));
        }
        if plan.state == "applying" {
            let snapshot = files::read(&plan.grant.path).checked("permission_denied")?;
            if snapshot.stamp.digest == plan.after_hash
                && snapshot.stamp.mode == plan.applied_mode()
            {
                plan.state = "applied".into();
                plan.after_stamp = Some(snapshot.stamp);
                self.save_config_plan(&plan)?;
                return Ok(plan.public());
            }
            if snapshot.stamp != plan.stamp {
                return Err(fail("result_pending"));
            }
            // A crash before replacement reuses the original prepared operation and backup.
        } else if plan.state != "prepared" {
            return Err(fail("result_pending"));
        }
        if chrono::DateTime::parse_from_rfc3339(&plan.created_at).checked("invalid_input")?
            < chrono::Utc::now() - chrono::Duration::hours(24)
        {
            return Err(fail("stale_snapshot"));
        }
        let bytes = checked_bytes(&plan.grant)?;
        if files::read(&plan.grant.path)
            .checked("permission_denied")?
            .stamp
            != plan.stamp
        {
            return Err(fail("stale_snapshot"));
        }
        let mut document: Value = serde_json::from_slice(&bytes).checked("invalid_input")?;
        for edit in &plan.edits {
            let value = if plan.credential_pointers.contains(&edit.pointer) {
                let reference = provider
                    .credential_ref
                    .as_ref()
                    .ok_or(fail("permission_denied"))?;
                let secret = self.vault.get(reference).checked("permission_denied")?;
                Value::String(
                    std::str::from_utf8(&secret)
                        .checked("permission_denied")?
                        .into(),
                )
            } else {
                edit.value.clone()
            };
            set(&mut document, &edit.pointer, value)?;
        }
        let mut after = serde_json::to_vec_pretty(&document).checked("invalid_input")?;
        after.push(b'\n');
        if after.len() > MAX_DOCUMENT as usize {
            return Err(fail("resource_limit"));
        }
        if plan.backup.is_none() {
            let key = vault::random_key();
            let reference = format!("plugin-config-{}", plan.id);
            self.vault
                .set(&reference, &key)
                .checked("permission_denied")?;
            plan.backup = Some(vault::seal(&key, &bytes, &plan.id).checked("unavailable")?);
            plan.backup_ref = Some(reference);
        }
        plan.after_hash = Some(package::hash(&after));
        plan.state = "applying".into();
        self.save_config_plan(&plan)?;
        plan.lease.check(self)?;
        files::replace(
            &plan.grant.path,
            &plan.stamp,
            Some(&after),
            plan.applied_mode(),
        )
        .checked("stale_snapshot")?;
        let snapshot = files::read(&plan.grant.path).checked("unavailable")?;
        if snapshot.bytes.as_deref() != Some(after.as_slice()) {
            return Err(fail("result_pending"));
        }
        plan.after_stamp = Some(snapshot.stamp);
        plan.state = "applied".into();
        self.save_config_plan(&plan)?;
        Ok(plan.public())
    }
    /// Recovery is owned by the host and remains usable after package removal or trust revocation.
    pub(super) fn restore_config(&self, q: &Query, id: &str) -> Result<Value> {
        let gate = self.gate(&q.plugin_id)?;
        let _guard = gate.try_lock().checked("resource_limit")?;
        let mut p = self.config_plan(q, id)?;
        if p.state == "restored" {
            return Ok(p.public());
        }
        if !["applied", "applying", "restoring"].contains(&p.state.as_str()) {
            return Err(fail("invalid_input"));
        }
        let current = files::read(&p.grant.path).checked("permission_denied")?;
        if ["applying", "restoring"].contains(&p.state.as_str())
            && current.stamp.digest == p.stamp.digest
        {
            // A crash before application or after restoration may leave the
            // original bytes in place. Permissions are part of that outcome.
            if current.stamp.mode != p.stamp.mode {
                return Err(fail("result_pending"));
            }
            p.state = "restored".into();
            self.save_config_plan(&p)?;
            return Ok(p.public());
        }
        if current.stamp.digest != p.after_hash
            || p.after_stamp.as_ref().is_some_and(|s| s != &current.stamp)
        {
            return Err(fail("stale_snapshot"));
        }
        let key = self
            .vault
            .get(p.backup_ref.as_ref().ok_or(fail("not_found"))?)
            .checked("permission_denied")?;
        let before = vault::open(&key, p.backup.as_ref().ok_or(fail("not_found"))?, &p.id)
            .checked("unavailable")?;
        if package::hash(&before) != p.grant.hash {
            return Err(fail("signature_invalid"));
        }
        p.state = "restoring".into();
        self.save_config_plan(&p)?;
        files::replace(&p.grant.path, &current.stamp, Some(&before), p.stamp.mode)
            .checked("stale_snapshot")?;
        p.state = "restored".into();
        self.save_config_plan(&p)?;
        Ok(p.public())
    }
}

fn common_edits(edits: &[JsonEdit]) -> Result<Vec<d::FieldEdit>> {
    edits
        .iter()
        .map(|edit| {
            Ok(d::FieldEdit {
                pointer: edit.pointer.clone(),
                value: edit.value.as_str().ok_or(fail("permission_denied"))?.into(),
            })
        })
        .collect()
}
fn planning_error(error: d::PlanError) -> super::super::Error {
    fail(error.code())
}

#[cfg(test)]
mod planner_bridge_tests {
    use super::*;
    #[test]
    fn common_json_job_retains_the_exact_extension_payload_and_rejects_extra_authority() {
        let edits = vec![
            JsonEdit {
                pointer: "/connection/model".into(),
                value: json!("owned-model"),
            },
            JsonEdit {
                pointer: "/connection/key".into(),
                value: json!(d::CREDENTIAL_PLACEHOLDER),
            },
        ];
        let request = d::ConfigRequest {
            target_revision: "owned-revision".into(),
            intent: d::ConfigIntent::JsonFields {
                edits: common_edits(&edits).unwrap(),
            },
        };
        let d::ConfigWork::JsonProposal { edits: proposed } =
            d::FiniteJsonPlanner.propose(&request).unwrap()
        else {
            panic!("expected JSON proposal");
        };
        assert_eq!(
            json!({"edits":proposed,"credentialPlaceholder":d::CREDENTIAL_PLACEHOLDER}),
            json!({"edits":edits,"credentialPlaceholder":"${HOST_CREDENTIAL}"})
        );
        let invalid = vec![JsonEdit {
            pointer: "/connection/key".into(),
            value: json!({"unexpected":"object"}),
        }];
        assert_eq!(
            common_edits(&invalid).err().unwrap().code,
            "permission_denied"
        );
        let result = d::ConfigProposal {
            request_identity: request.identity(),
            output: d::ConfigOutput::JsonFields(vec![d::FieldEdit {
                pointer: "/other/file".into(),
                value: "forbidden".into(),
            }]),
        };
        assert_eq!(
            planning_error(
                d::FiniteJsonPlanner
                    .inspect_result(&request, result)
                    .err()
                    .unwrap()
            )
            .code,
            "permission_denied"
        );
    }
}
fn set(root: &mut Value, pointer: &str, value: Value) -> Result<()> {
    if !super::safe_pointer(pointer) {
        return Err(fail("permission_denied"));
    }
    let mut keys = pointer[1..].split('/').peekable();
    let mut node = root;
    while let Some(key) = keys.next() {
        let map = node.as_object_mut().ok_or(fail("invalid_input"))?;
        if keys.peek().is_none() {
            map.insert(key.to_owned(), value);
            return Ok(());
        }
        node = map.entry(key).or_insert_with(|| json!({}));
    }
    Err(fail("invalid_input"))
}

#[cfg(test)]
mod lifecycle_tests;

#[cfg(test)]
mod isolation_tests {
    use super::*;
    use local_agent_core::plugin::external::{StateAction, StateRequest};
    use std::time::{Duration, Instant};

    async fn enabled(root: &std::path::Path) -> (Arc<Manager>, Query) {
        let (manager, query) = super::super::record_tests::host(root);
        let manager = Arc::new(manager);
        manager
            .change(StateRequest {
                plugin_id: query.plugin_id.clone(),
                digest: query.digest.clone(),
                action: StateAction::Enable,
            })
            .await
            .unwrap();
        (manager, query)
    }

    #[tokio::test]
    async fn preview_commit_rechecks_file_lease_and_shutdown_without_writing_stale_plans() {
        for change in ["unchanged", "file", "grant", "shutdown"] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            let (manager, query) = enabled(&root).await;
            let mut plan: Plan =
                serde_json::from_value(super::super::record_tests::valid_plan(&root, &query))
                    .unwrap();
            plan.lease = Lease::from(&manager.status(&query.plugin_id, &query.digest).unwrap());
            plan.grant.lease = plan.lease.clone();
            match change {
                "file" => std::fs::write(&plan.grant.path, b"{\"changed\":true}").unwrap(),
                "grant" => manager
                    .revoke(&query.plugin_id, &query.digest, false)
                    .unwrap(),
                "shutdown" => manager.stop(),
                _ => (),
            }
            let result = manager.commit_config_preview(plan);
            let count: i64 = manager
                .advanced
                .lock()
                .unwrap()
                .db()
                .unwrap()
                .query_row("SELECT count(*) FROM plans", [], |r| r.get(0))
                .unwrap();
            if change == "unchanged" {
                assert_eq!(result.unwrap()["planId"], "valid-plan");
                assert_eq!(count, 1);
            } else {
                assert_eq!(
                    result.unwrap_err().code,
                    match change {
                        "file" => "stale_snapshot",
                        "grant" => "grant_revoked",
                        _ => "plugin_disabled",
                    }
                );
                assert_eq!(count, 0);
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn waiting_for_config_catalog_does_not_block_the_async_runtime() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let (manager, query) = enabled(&root).await;
        let provider = local_agent_core::Store::open(&root)
            .unwrap()
            .providers()
            .unwrap()
            .remove(0);
        let (held, ready) = tokio::sync::oneshot::channel();
        let (release, gate) = std::sync::mpsc::channel();
        let holder = std::thread::spawn({
            let manager = manager.clone();
            move || {
                let _guard = manager.db.lock().unwrap();
                held.send(()).unwrap();
                let _ = gate.recv_timeout(Duration::from_secs(4));
            }
        });
        ready.await.unwrap();
        let start = Instant::now();
        let request = tokio::spawn(async move { manager.prepare_config(&query, &provider).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        let elapsed = start.elapsed();
        let was_waiting = !request.is_finished();
        let _ = release.send(());
        holder.join().unwrap();
        assert_eq!(request.await.unwrap().unwrap_err().code, "unsupported");
        assert!(was_waiting);
        assert!(
            elapsed < Duration::from_secs(1),
            "runtime stalled for {elapsed:?}"
        );
    }
}
