use std::collections::{BTreeMap, BTreeSet};

use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use local_agent_core::plugin::platform::*;
use rusqlite::{TransactionBehavior, params};
use serde::Deserialize;

use crate::{ApiResult, AppState};

use super::{
    Checked, Manager, Result,
    api::response,
    fail, package, platform_package,
    platform_store::{PlatformStore, valid_object_id, validate_values},
    run_sync,
    store::Selection,
};

mod directory;
mod entity;
pub(super) mod guest;
mod source_adapter;
pub(crate) use source_adapter::refresh_if_directory;
pub(crate) use source_adapter::{DirectoryView, run_sources, source_views};
pub(super) use source_adapter::{Snapshot as DirectorySnapshot, validate_mapper};
#[path = "sink.rs"]
mod sink;
pub(crate) use sink::run as run_sinks;

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/plugins/platform/contributions", get(list))
        .route("/api/plugins/platform/slot", post(query))
        .route("/api/plugins/platform/action", post(action))
        .route("/api/plugins/platform/directory", post(grant_directory))
        .route("/api/plugins/platform/manage", post(manage))
}

async fn list(State(state): State<AppState>) -> ApiResult {
    response(
        run_sync(move || {
            let status = state.plugins.list()?;
            let mut items = Vec::new();
            let mut store = None;
            for plugin in status
                .into_iter()
                .filter(|p| platform_package::opted_in(&p.manifest))
            {
                let selection = state.plugins.selection(&plugin.manifest.id)?;
                let mut item = PlatformContribution {
                    plugin_id: plugin.manifest.id,
                    digest: plugin.digest,
                    epoch: plugin.activation_epoch,
                    grant_revision: String::new(),
                    enabled: false,
                    definition: None,
                    entity_grants: vec![],
                    directory_grants: vec![],
                    used_rows: 0,
                    used_bytes: 0,
                    sink: None,
                    diagnostic: plugin.error_code,
                };
                let store = store.get_or_insert_with(|| open_store(&state));
                let result = store.as_ref().map_err(Clone::clone).and_then(|store| {
                    let definition = state
                        .plugins
                        .platform_definition(&item.plugin_id, &item.digest)?;
                    let db = &store.0;
                    let (rows, bytes) = PlatformStore::usage(db, &item.plugin_id)?;
                    item.used_rows = rows;
                    item.used_bytes = bytes;
                    item.grant_revision =
                        PlatformStore::revision(db, &item.plugin_id, selection.revision)?;
                    item.entity_grants = PlatformStore::entity_grants(
                        db,
                        &item.plugin_id,
                        &item.digest,
                        selection.revision,
                    )?;
                    item.directory_grants = directory::purposes(db, &item.plugin_id, &selection)?;
                    if selection.digest != item.digest {
                        item.directory_grants.clear();
                    } else {
                        item.sink = sink::status(db, &item.plugin_id, &selection)?;
                    }
                    item.enabled = plugin.enabled && plugin.runtime_status == "active";
                    item.definition = Some(definition);
                    Ok(())
                });
                if let Err(error) = result {
                    item.diagnostic = Some(error.code.into());
                }
                items.push(item);
            }
            serde_json::to_value(PlatformContributionList { items }).checked("invalid_record")
        })
        .await,
    )
}

async fn query(State(state): State<AppState>, Json(request): Json<PlatformSlotQuery>) -> ApiResult {
    response(query_slot(state, request).await)
}

async fn query_slot(state: AppState, request: PlatformSlotQuery) -> Result<serde_json::Value> {
    let plugin = request.plugin_id.clone();
    let selector = request.slot_or_view_id.clone();
    let snapshot = state.clone();
    let (selection, definition, mut view, summary_grants) = run_sync(move || {
        let state = snapshot;
        let (selection, definition) = state
            .plugins
            .active_platform(&request.plugin_id, &request.digest)?;
        let mut store = open_store(&state)?;
        let transaction = store.0.transaction().checked("unavailable")?;
        let revision =
            PlatformStore::revision(&transaction, &request.plugin_id, selection.revision)?;
        let grants = PlatformStore::entity_grants(
            &transaction,
            &request.plugin_id,
            &request.digest,
            selection.revision,
        )?;
        let sink_status = if definition.log_sink.is_some() {
            sink::status(&transaction, &request.plugin_id, &selection)?
        } else {
            None
        };
        let slot = definition
            .slots
            .iter()
            .find(|slot| slot.id == request.slot_or_view_id);
        let view_id = if let Some(slot) = slot {
            if slot.point.target() != request.context.as_ref().map(|context| context.target) {
                return Err(fail("object_context_required"));
            }
            &slot.view_id
        } else {
            &request.slot_or_view_id
        };
        let view = definition.views.get(view_id).ok_or(fail("not_found"))?;
        let (mut records, next_cursor) = if let Some(collection_id) = &view.collection {
            let collection = &definition.collections[collection_id];
            validate_values(collection, &request.filter, false)?;
            if request
                .cursor
                .as_ref()
                .is_some_and(|cursor| !package::local_id(cursor))
            {
                return Err(fail("invalid_input"));
            }
            let reference = match slot.and_then(|slot| slot.filter_by_context.as_deref()) {
                Some(field) => {
                    let context = request
                        .context
                        .as_ref()
                        .filter(|context| valid_object_id(&context.object_id))
                        .ok_or(fail("object_context_required"))?;
                    Some((field, context.target, context.object_id.as_str()))
                }
                None => None,
            };
            PlatformStore::records(
                &transaction,
                &request.plugin_id,
                collection_id,
                request.cursor.as_deref(),
                &request.filter,
                reference,
            )?
        } else {
            if request.cursor.is_some() || !request.filter.is_empty() {
                return Err(fail("invalid_input"));
            }
            (vec![], None)
        };
        transaction.commit().checked("unavailable")?;
        let summary_grants = if let Some(collection) = view
            .collection
            .as_ref()
            .and_then(|key| definition.collections.get(key))
        {
            entity::hydrate(&state, collection, &grants, &mut records)?
        } else {
            entity::SummaryGrants::default()
        };
        state
            .plugins
            .ensure_platform_current(&request.plugin_id, &selection)?;
        if PlatformStore::revision(&store.0, &request.plugin_id, selection.revision)? != revision {
            return Err(fail("grant_revoked"));
        }
        let summaries = || records.iter().flat_map(|record| record.hydrated.values());
        let authorization = if summaries()
            .any(|entity| matches!(entity.authorization, Authorization::Unauthorized))
        {
            Authorization::Unauthorized
        } else if summaries().any(|entity| matches!(entity.authorization, Authorization::Unknown)) {
            Authorization::Unknown
        } else {
            Authorization::Authorized
        };
        let coverage = if next_cursor.is_some() {
            Coverage::Partial
        } else {
            Coverage::Complete
        };
        let reply = PlatformSlotView {
            action_store_id: Some(PlatformStore::identity(&store.0)?),
            blocks: vec![view.clone()],
            records,
            next_cursor,
            coverage,
            freshness: Freshness::Current,
            authorization,
            epoch: selection.epoch.to_string(),
            grant_revision: revision,
            sink: sink_status,
        };
        Ok((
            selection,
            definition,
            reply,
            std::sync::Arc::new(summary_grants),
        ))
    })
    .await?;
    if definition.wasm.as_ref().is_some_and(|wasm| wasm.render) {
        guest::render(
            &state,
            &plugin,
            selection.clone(),
            definition,
            &selector,
            &mut view,
            summary_grants.clone(),
        )
        .await?;
    }
    let revision = view.grant_revision.clone();
    let reply = serde_json::to_value(view).checked("invalid_record")?;
    run_sync(move || {
        let store = open_store(&state)?;
        let _commit = state
            .plugins
            .platform_commit
            .lock()
            .checked("unavailable")?;
        state.plugins.ensure_platform_current(&plugin, &selection)?;
        if PlatformStore::revision(&store.0, &plugin, selection.revision)? != revision {
            return Err(fail("grant_revoked"));
        }
        summary_grants.ensure(&state, &store.0)?;
        Ok(reply)
    })
    .await
}

async fn action(
    State(state): State<AppState>,
    Json(request): Json<PlatformActionRequest>,
) -> ApiResult {
    response(apply_action(state, request).await)
}

async fn apply_action(
    state: AppState,
    mut request: PlatformActionRequest,
) -> Result<serde_json::Value> {
    let receipt_only = request.receipt_only.take().unwrap_or(false);
    if receipt_only && request.operation_id.is_none() {
        return Err(fail("invalid_input"));
    }
    let intent = match (&request.operation_id, &request.store_id) {
        (None, None) => None,
        (Some(id), Some(_))
            if uuid::Uuid::parse_str(id)
                .is_ok_and(|parsed| parsed.get_version_num() == 4 && parsed.to_string() == *id) =>
        {
            Some(package::hash(
                &serde_json::to_vec(&request).checked("invalid_input")?,
            ))
        }
        _ => return Err(fail("invalid_input")),
    };
    let _operation = state.plugins.gate(&request.plugin_id)?.lock_owned().await;
    let original_intent = intent.clone();
    let verification = state.clone();
    let input = request.clone();
    let (selection, definition, receipt) = run_sync(move || {
        let (selection, definition) = verification
            .plugins
            .active_platform(&input.plugin_id, &input.digest)?;
        let store = open_store(&verification)?;
        let revision = PlatformStore::revision(&store.0, &input.plugin_id, selection.revision)?;
        authorize_action(&input, &selection, &definition, &revision)?;
        let receipt =
            PlatformStore::receipt(&store.0, &input, original_intent.as_deref(), &revision)?;
        Ok((selection, definition, receipt))
    })
    .await?;
    if let Some(result) = receipt {
        return Ok(result);
    }
    if receipt_only {
        return Ok(serde_json::json!({"pending":true}));
    }
    let writes = if definition.wasm.as_ref().is_some_and(|wasm| wasm.action) {
        guest::action(&state, selection.clone(), definition.clone(), &mut request).await?
    } else {
        vec![]
    };
    run_sync(move || {
        let plugin = &request.plugin_id;
        let mut store = open_store(&state)?;
        let tx = store
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .checked("unavailable")?;
        let revision = PlatformStore::revision(&tx, plugin, selection.revision)?;
        authorize_action(&request, &selection, &definition, &revision)?;
        if let Some(result) = PlatformStore::receipt(&tx, &request, intent.as_deref(), &revision)? {
            return Ok(result);
        }
        let record_id = if request.action_id == PlatformAction::SetValue {
            PlatformStore::set_value(&tx, plugin, &request)?;
            request.record_id.clone()
        } else {
            let collection = request.collection.as_deref().ok_or(fail("invalid_input"))?;
            let schema = definition
                .collections
                .get(collection)
                .ok_or(fail("not_found"))?;
            PlatformStore::change_record(&tx, &request, collection, schema)?
        };
        for write in writes {
            write.apply(&tx, &request, &definition)?;
        }
        let (rows, bytes) = PlatformStore::usage(&tx, plugin)?;
        if rows > MAX_ROWS || bytes > MAX_BYTES {
            return Err(fail("resource_limit"));
        }
        let _commit = state
            .plugins
            .platform_commit
            .lock()
            .checked("unavailable")?;
        state.plugins.ensure_platform_current(plugin, &selection)?;
        if let (Some(id), Some(hash)) = (&request.operation_id, &intent) {
            let count: i64 = tx
                .query_row(
                    "SELECT count(*) FROM ext_action_receipts WHERE plugin_id=?1",
                    [plugin],
                    |r| r.get(0),
                )
                .checked("unavailable")?;
            if count >= 100_000 {
                return Err(fail("resource_limit"));
            }
            tx.execute(
                "INSERT INTO ext_action_receipts VALUES(?1,?2,?3,?4)",
                params![plugin, id, hash, record_id],
            )
            .checked("unavailable")?;
        }
        tx.commit().checked("unavailable")?;
        Ok(serde_json::json!({"recordId": record_id, "grantRevision": revision}))
    })
    .await
}

fn authorize_action(
    request: &PlatformActionRequest,
    selection: &Selection,
    definition: &PlatformDefinition,
    revision: &str,
) -> Result<()> {
    if request.expected_epoch != selection.epoch.to_string()
        || request.expected_grant_revision != revision
    {
        return Err(fail("grant_revoked"));
    }
    if !definition.views.values().any(|view| {
        view.collection == request.collection && view.actions.contains(&request.action_id)
    }) {
        return Err(fail("permission_denied"));
    }
    if let Some(expected) = &request.expected_values
        && (!matches!(
            request.action_id,
            PlatformAction::Update | PlatformAction::Move
        ) || expected.is_empty()
            || !expected.keys().eq(request.values.keys()))
    {
        return Err(fail("invalid_input"));
    }
    Ok(())
}

async fn manage(
    State(state): State<AppState>,
    Json(request): Json<PlatformManageRequest>,
) -> ApiResult {
    if matches!(request.operation, PlatformManagement::FlushSink) {
        return response(
            sink::flush(
                state,
                request.plugin_id,
                request.digest,
                Some(request.expected_grant_revision),
            )
            .await
            .and_then(|status| serde_json::to_value(status).checked("invalid_record")),
        );
    }
    response(run_sync(move || {
        let plugin = &request.plugin_id;
        let (selection, definition) = state.plugins.active_platform(plugin, &request.digest)?;
        let mut store = open_store(&state)?;
        let tx = store.0.transaction_with_behavior(TransactionBehavior::Immediate).checked("unavailable")?;
        if request.expected_grant_revision != PlatformStore::revision(&tx, plugin, selection.revision)? {
            return Err(fail("grant_revoked"));
        }
        match request.operation {
            PlatformManagement::GrantEntity { target } => {
                if !definition.collections.values().flat_map(|c| c.fields.values()).any(|field| field.kind.reference() == Some(target)) {
                    return Err(fail("permission_denied"));
                }
                tx.execute(
                    "INSERT INTO ext_entity_grants VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ','now'), ?4)
                    ON CONFLICT (plugin_id, target_kind) DO UPDATE SET digest = excluded.digest,
                    granted_at = excluded.granted_at, host_revision = excluded.host_revision",
                    params![plugin, request.digest, target.as_str(), selection.revision.to_string()],
                ).checked("unavailable")?;
            }
            PlatformManagement::RevokeEntity { target } => {
                tx.execute("DELETE FROM ext_entity_grants WHERE plugin_id = ?1 AND digest = ?2 AND target_kind = ?3",
                    params![plugin, request.digest, target.as_str()],
                ).checked("unavailable")?;
            }
            PlatformManagement::ClearData => {
                tx.execute("DELETE FROM ext_data_records WHERE plugin_id = ?1", [plugin]).checked("unavailable")?;
                tx.execute("DELETE FROM ext_data_kv WHERE plugin_id = ?1", [plugin]).checked("unavailable")?;
            }
            PlatformManagement::RevokeDirectory { purpose } => directory::revoke(&tx, plugin, purpose)?,
            PlatformManagement::ConfigureSink { filter, rotate_policy } =>
                sink::configure(&tx, plugin, &selection, filter, rotate_policy)?,
            PlatformManagement::FlushSink => return Err(fail("invalid_input")),
        }
        PlatformStore::bump_revision(&tx, plugin)?;
        let revision = PlatformStore::revision(&tx, plugin, selection.revision)?;
        let _commit = state.plugins.platform_commit.lock().checked("unavailable")?;
        state.plugins.ensure_platform_current(plugin, &selection)?;
        tx.commit().checked("unavailable")?;
        Ok(serde_json::json!({"grantRevision": revision}))
    }).await)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DirectorySelection {
    request: DirectoryGrantRequest,
    path: std::path::PathBuf,
    expected_grant_revision: String,
}

async fn grant_directory(
    State(state): State<AppState>,
    Json(selection): Json<DirectorySelection>,
) -> ApiResult {
    response(
        run_sync(move || {
            let request = selection.request.clone();
            let mut result = directory::grant(&state, selection)?;
            if matches!(request.purpose, DirectoryPurpose::SourceRead) {
                result["read"] =
                    match source_adapter::refresh(&state, &request.plugin_id, &request.digest) {
                        Ok(None) => serde_json::json!({"status":"complete"}),
                        Ok(Some(code)) => serde_json::json!({"status":"partial","code":code}),
                        Err(error) => {
                            serde_json::json!({"status":"pending_or_failed","code":error.code})
                        }
                    };
            }
            Ok(result)
        })
        .await,
    )
}

fn open_store(state: &AppState) -> Result<PlatformStore> {
    let id = state.store_readers.read(
        || fail("busy"),
        |store| store.database_id().checked("unavailable"),
    )?;
    PlatformStore::open(&state.plugins.root, &id)
}

impl Manager {
    fn platform_definition(&self, id: &str, digest: &str) -> Result<PlatformDefinition> {
        let (_, bundle) = self.verified_package(id, digest)?;
        platform_package::parse(&bundle.manifest, &bundle.files)?.ok_or(fail("not_found"))
    }

    fn active_platform(&self, id: &str, digest: &str) -> Result<(Selection, PlatformDefinition)> {
        self.ensure_running()?;
        let selected = self.selection(id)?;
        if selected.digest != digest || !selected.enabled || selected.status != "active" {
            return Err(fail("plugin_disabled"));
        }
        let definition = self.platform_definition(id, digest)?;
        self.ensure_platform_current(id, &selected)?;
        Ok((selected, definition))
    }

    fn ensure_platform_current(&self, id: &str, expected: &Selection) -> Result<()> {
        self.ensure_running()?;
        let selected = self.selection(id)?;
        if selected.digest != expected.digest
            || !selected.enabled
            || selected.status != "active"
            || selected.epoch != expected.epoch
            || selected.revision != expected.revision
        {
            return Err(fail("grant_revoked"));
        }
        Ok(())
    }
}
