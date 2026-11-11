use super::super::{Checked, Manager, Result, fail, package as legacy, run_sync};
use super::{
    runtime::{self, Context},
    store::{Grant, Installed, Lease},
};
use local_agent_core::{
    domain::{Decimal, activity::query::ReportedActivityMeasurements},
    plugin::full as f,
};
use rusqlite::{OptionalExtension, params};
use std::{collections::BTreeMap, sync::Arc};

pub(super) const PLUGIN: &str = "net.worksha.dsh-desktop";
const PUBLISHER: &str = "net.worksha";

pub(super) fn is_source(manifest: &f::Manifest) -> bool {
    manifest.id == PLUGIN
        && manifest.publisher == PUBLISHER
        && manifest
            .source
            .as_ref()
            .is_some_and(|source| source.id == "desktop" && source.max_age_seconds.is_some())
}

impl Manager {
    /// A new publisher approval must never revive a grant from before revocation.
    pub(super) fn dsh_publisher_ticket(
        &self,
        package: &Installed,
        create: bool,
    ) -> Result<Option<String>> {
        let db = self.db.lock()?;
        if create {
            db.execute(
                "INSERT OR IGNORE INTO publisher_source_tickets SELECT id,?3 FROM publishers WHERE id=?1 AND public_key=?2",
                params![PUBLISHER, package.signing_key, uuid::Uuid::new_v4().to_string()],
            )
            .checked("unavailable")?;
        }
        db.query_row(
            "SELECT ticket FROM publisher_source_tickets JOIN publishers ON publisher=id WHERE id=?1 AND public_key=?2",
            params![PUBLISHER, package.signing_key],
            |row| row.get(0),
        )
        .optional()
        .checked("unavailable")
    }

    pub(crate) async fn restore_dsh_source(self: &Arc<Self>) -> Result<()> {
        let gate = self.gate(PLUGIN)?.lock_owned().await;
        let manager = self.clone();
        let query = run_sync(move || manager.restore_saved_dsh_source()).await?;
        drop(gate);
        if let Some(query) = query {
            self.watch_source(query.clone())?;
            // The watcher retries if the exporter is midway through a write.
            let _ = self.full_refresh(query).await;
        }
        Ok(())
    }

    fn restore_saved_dsh_source(&self) -> Result<Option<f::PackageQuery>> {
        if !self
            .root
            .with_file_name("plugin-sources")
            .join("selected")
            .join(PLUGIN)
            .is_file()
        {
            return Ok(None);
        }
        let (selected, saved) = {
            let host = self.full.lock().checked("unavailable")?;
            let Some(saved) = host.saved_dsh_source(PLUGIN)? else {
                return Ok(None);
            };
            (host.selection(PLUGIN)?, saved)
        };
        if !selected.enabled
            || selected.digest != saved.digest
            || selected.epoch != saved.epoch
            || selected.revision != saved.revision
        {
            return Ok(None);
        }
        let query = f::PackageQuery {
            plugin_id: PLUGIN.into(),
            digest: selected.digest.clone(),
        };
        let (package, path) = self.full_verified(&query)?;
        if !is_source(&package.manifest)
            || self.dsh_publisher_ticket(&package, false)?.as_ref() != Some(&saved.publisher_ticket)
        {
            return Err(fail("grant_revoked"));
        }
        let lease = Lease {
            id: query.plugin_id.clone(),
            digest: query.digest.clone(),
            publisher: package.manifest.publisher.clone(),
            epoch: selected.epoch,
            revision: selected.revision,
            publisher_revision: self.full_publisher_revision(PUBLISHER)?,
        };
        let grant = Grant {
            lease: lease.clone(),
            path: saved.path.clone(),
            device: saved.device,
            inode: saved.inode,
            unified: true,
            connection: None,
        };
        let metadata = std::fs::symlink_metadata(&grant.path).checked("permission_denied")?;
        if metadata.created().ok() != Some(saved.created)
            || grant.path.canonicalize().ok().as_ref() != Some(&grant.path)
        {
            return Err(fail("grant_revoked"));
        }
        runtime::stamp(&grant)?;
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
        if host.selection(PLUGIN)? != context.before
            || !legacy::read_file(&host.root.join("selected").join(PLUGIN), 64)
                .is_ok_and(|bytes| bytes == query.digest.as_bytes())
        {
            return Err(fail("grant_revoked"));
        }
        host.active.insert(PLUGIN.into(), lease);
        host.grants.insert(PLUGIN.into(), grant);
        Ok(Some(query))
    }
}

pub(crate) struct Task {
    pub(crate) native_id: String,
    pub(crate) title: String,
    pub(crate) started_at: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) measurements: Option<ReportedActivityMeasurements>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BridgeTiming {
    message_native_id: String,
    message_started_at_ms: Option<u64>,
    model: Option<String>,
    first_token_ms: Option<u64>,
    output_tokens: Option<Decimal>,
    completed_tokens: Option<Decimal>,
    completed_decode_ms: Option<u64>,
    running_tokens: Option<Decimal>,
    running_decode_ms: Option<u64>,
}

impl BridgeTiming {
    fn measurements(&self) -> Option<ReportedActivityMeasurements> {
        const MAX_DURATION_MS: u64 = 86_400_000;
        const MAX_EXACT_TOKENS: u64 = (1 << 53) - 1;
        let rate = |tokens: Option<&Decimal>, elapsed: Option<u64>| {
            let count = tokens?.value();
            let ms = elapsed?;
            (count <= MAX_EXACT_TOKENS && (1..=MAX_DURATION_MS).contains(&ms))
                .then(|| count as f64 * 1000.0 / ms as f64)
        };
        let reported = ReportedActivityMeasurements {
            first_token_ms: self.first_token_ms.filter(|v| *v <= MAX_DURATION_MS),
            running_tokens_per_second: rate(self.running_tokens.as_ref(), self.running_decode_ms),
            completed_tokens_per_second: rate(
                self.completed_tokens.as_ref(),
                self.completed_decode_ms,
            ),
            output_tokens: self
                .output_tokens
                .clone()
                .filter(|v| v.value() <= MAX_EXACT_TOKENS),
        };
        (reported.first_token_ms.is_some()
            || reported.running_tokens_per_second.is_some()
            || reported.completed_tokens_per_second.is_some()
            || reported.output_tokens.is_some())
        .then_some(reported)
    }
}

/// The bundled bridge supplies chronological conversation events alongside each
/// session fact. Only authored messages from the authorized frame name tasks.
pub(super) fn latest_tasks(frame: &str) -> BTreeMap<String, Task> {
    frame
        .lines()
        .filter_map(|line| {
            let mut row: serde_json::Value = serde_json::from_str(line).ok()?;
            let fact = &row["fact"];
            if fact["kind"] != "session" {
                return None;
            }
            let session = fact["value"]["nativeId"].as_str()?;
            let event = row["conversation"]
                .as_array()?
                .iter()
                .rev()
                .find(|event| event["kind"] == "message" && event["role"] == "user")?;
            let native_id = event["nativeId"].as_str()?;
            native_id.parse::<u64>().ok()?;
            let title = event["text"]
                .as_str()?
                .trim()
                .chars()
                .take(240)
                .collect::<String>();
            if title.is_empty() {
                return None;
            }
            let session = session.to_owned();
            let native_id = native_id.to_owned();
            let timing = serde_json::from_value::<BridgeTiming>(row["activityTiming"].take())
                .ok()
                .filter(|timing| timing.message_native_id == native_id);
            let measurements = timing.as_ref().and_then(BridgeTiming::measurements);
            let started_at = timing.as_ref().and_then(|timing| {
                let milliseconds = i64::try_from(timing.message_started_at_ms?).ok()?;
                Some(
                    chrono::DateTime::from_timestamp_millis(milliseconds)?
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                )
            });
            let model = timing.and_then(|timing| timing.model).filter(|model| {
                !model.trim().is_empty()
                    && model.len() <= 200
                    && !model.chars().any(char::is_control)
            });
            Some((
                session,
                Task {
                    native_id,
                    title,
                    started_at,
                    model,
                    measurements,
                },
            ))
        })
        .collect()
}
