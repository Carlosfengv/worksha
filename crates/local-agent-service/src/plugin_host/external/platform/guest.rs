use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use local_agent_core::plugin::platform::*;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::AppState;

use super::{
    Checked, PlatformStore, Result, Selection, entity::SummaryGrants, fail, open_store, package,
    run_sync, validate_values,
};

#[derive(Clone)]
pub(in crate::plugin_host::external) struct Call {
    state: AppState,
    plugin: String,
    selection: Selection,
    grant_revision: String,
    definition: PlatformDefinition,
    writable: bool,
    handles: BTreeMap<String, (String, String, String)>,
    summary_grants: Arc<SummaryGrants>,
    pub(in crate::plugin_host::external) writes: Vec<Write>,
}

impl Call {
    fn new(
        state: &AppState,
        plugin: &str,
        selection: Selection,
        grant_revision: String,
        definition: PlatformDefinition,
        writable: bool,
    ) -> Self {
        Self {
            state: state.clone(),
            plugin: plugin.into(),
            selection,
            grant_revision,
            definition,
            writable,
            handles: BTreeMap::new(),
            summary_grants: Arc::default(),
            writes: vec![],
        }
    }

    fn ensure(&self, store: &Connection) -> Result<()> {
        self.state
            .plugins
            .ensure_platform_current(&self.plugin, &self.selection)?;
        if PlatformStore::revision(store, &self.plugin, self.selection.revision)?
            != self.grant_revision
        {
            return Err(fail("grant_revoked"));
        }
        self.summary_grants.ensure(&self.state, store)
    }

    pub(in crate::plugin_host::external) async fn invoke(
        &mut self,
        method: &str,
        payload: &str,
        storage: bool,
    ) -> Result<Value> {
        let method = method.to_owned();
        let request: Value = serde_json::from_str(payload).checked("invalid_input")?;
        let source = self.clone();
        let (reply, staged) = run_sync(move || {
            let store = open_store(&source.state)?;
            source.ensure(&store.0)?;
            if method == "domain.read" {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Handle {
                    handle: String,
                }
                let request: Handle = serde_json::from_value(request).checked("invalid_input")?;
                let (collection, field, id) = source
                    .handles
                    .get(&request.handle)
                    .ok_or(fail("permission_denied"))?;
                let schema = source
                    .definition
                    .collections
                    .get(collection)
                    .ok_or(fail("invalid_record"))?;
                let grants = PlatformStore::entity_grants(
                    &store.0,
                    &source.plugin,
                    &source.selection.digest,
                    source.selection.revision,
                )?;
                let mut record = PlatformRecord {
                    record_id: String::new(),
                    values: BTreeMap::from([(field.clone(), json!(id))]),
                    hydrated: Default::default(),
                };
                let grants = super::entity::hydrate(
                    &source.state,
                    schema,
                    &grants,
                    std::slice::from_mut(&mut record),
                )?;
                let summary = record
                    .hydrated
                    .get(field)
                    .filter(|s| matches!(s.authorization, Authorization::Authorized))
                    .ok_or(fail("permission_denied"))?;
                let reply = serde_json::to_value(summary).checked("invalid_record")?;
                let _commit = source
                    .state
                    .plugins
                    .platform_commit
                    .lock()
                    .checked("unavailable")?;
                source.ensure(&store.0)?;
                grants.ensure(&source.state, &store.0)?;
                return Ok((reply, None));
            }
            if matches!(method.as_str(), "source.list_files" | "source.read") {
                let Some(AdapterDef::AgentAdapter { source_mapper }) = &source.definition.adapter
                else {
                    return Err(fail("permission_denied"));
                };
                let value = super::source_adapter::guest_read(
                    &store.0,
                    &source.plugin,
                    &source.selection,
                    source_mapper,
                    &method,
                    request,
                )?;
                source.ensure(&store.0)?;
                return Ok((value, None));
            }
            if !storage {
                return Err(fail("permission_denied"));
            }
            let command: StorageRequest =
                serde_json::from_value(request).checked("invalid_input")?;
            let (reply, staged) = source.storage(&store.0, &method, command)?;
            source.ensure(&store.0)?;
            Ok((reply, staged))
        })
        .await?;
        if let Some(staged) = staged {
            if self.writes.len() >= 32 {
                return Err(fail("resource_limit"));
            }
            self.writes.push(staged);
        }
        Ok(reply)
    }

    fn storage(
        &self,
        db: &Connection,
        method: &str,
        request: StorageRequest,
    ) -> Result<(Value, Option<Write>)> {
        let no_write = |value: Value| Ok((value, None));
        let key = || {
            request
                .key
                .as_deref()
                .filter(|key| package::local_id(key))
                .ok_or(fail("invalid_input"))
        };
        let collection = || {
            let id = request.collection.as_deref().ok_or(fail("invalid_input"))?;
            let definition = self
                .definition
                .collections
                .get(id)
                .ok_or(fail("not_found"))?;
            Ok((id, definition))
        };
        let record = || {
            request
                .record_id
                .as_deref()
                .filter(|key| package::local_id(key))
                .ok_or(fail("invalid_input"))
        };
        match method {
            "storage.kv.get" => {
                let key = key()?;
                if let Some(value) = self.writes.iter().rev().find_map(|write| match write {
                    Write::PutValue(name, value) if name == key => Some(json!({"value":value})),
                    Write::DeleteValue(name) if name == key => Some(json!({"value":null})),
                    _ => None,
                }) {
                    return no_write(value);
                }
                let json: Option<String> = db
                    .query_row(
                        "SELECT value_json FROM ext_data_kv WHERE plugin_id=?1 AND key=?2",
                        params![self.plugin, key],
                        |row| row.get(0),
                    )
                    .optional()
                    .checked("invalid_record")?;
                let value: Option<Value> = json
                    .map(|json| serde_json::from_str(&json))
                    .transpose()
                    .checked("invalid_record")?;
                no_write(json!({"value":value}))
            }
            "storage.kv.list" => {
                let after = request.cursor.as_deref().unwrap_or_default();
                if !after.is_empty() && !package::local_id(after) {
                    return Err(fail("invalid_input"));
                }
                let mut statement = db.prepare("SELECT key FROM ext_data_kv WHERE plugin_id=?1 AND key>?2 ORDER BY key LIMIT 65").checked("invalid_record")?;
                let mut keys = statement
                    .query_map(params![self.plugin, after], |row| row.get::<_, String>(0))
                    .checked("invalid_record")?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .checked("invalid_record")?;
                let next = if keys.len() > 64 {
                    keys.pop();
                    keys.last().cloned()
                } else {
                    None
                };
                no_write(json!({"keys":keys,"nextCursor":next}))
            }
            "storage.records.get" => {
                let (collection, _) = collection()?;
                let json: Option<String> = db.query_row("SELECT record_json FROM ext_data_records WHERE plugin_id=?1 AND collection=?2 AND record_id=?3",
                    params![self.plugin, collection, record()?], |row| row.get(0)).optional().checked("invalid_record")?;
                let value: Option<Value> = json
                    .map(|json| serde_json::from_str(&json))
                    .transpose()
                    .checked("invalid_record")?;
                no_write(json!({"values":value}))
            }
            "storage.records.query" => {
                let (collection, definition) = collection()?;
                validate_values(definition, &request.filter, false)?;
                let (records, cursor) = PlatformStore::records(
                    db,
                    &self.plugin,
                    collection,
                    request.cursor.as_deref(),
                    &request.filter,
                    None,
                )?;
                no_write(json!({"records":records,"nextCursor":cursor}))
            }
            "storage.kv.put" if self.writable => Ok((
                json!({"staged":true}),
                Some(Write::PutValue(key()?.into(), request.value)),
            )),
            "storage.kv.delete" if self.writable => Ok((
                json!({"staged":true}),
                Some(Write::DeleteValue(key()?.into())),
            )),
            "storage.records.put" if self.writable => {
                let (collection, _) = collection()?;
                let id = if request.record_id.is_some() {
                    Some(record()?.into())
                } else {
                    None
                };
                Ok((
                    json!({"staged":true}),
                    Some(Write::PutRecord(collection.into(), id, request.values)),
                ))
            }
            "storage.records.delete" if self.writable => {
                let (collection, _) = collection()?;
                Ok((
                    json!({"staged":true}),
                    Some(Write::DeleteRecord(collection.into(), record()?.into())),
                ))
            }
            _ => Err(fail("permission_denied")),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StorageRequest {
    key: Option<String>,
    collection: Option<String>,
    record_id: Option<String>,
    cursor: Option<String>,
    #[serde(default)]
    filter: Fields,
    #[serde(default)]
    values: Fields,
    #[serde(default)]
    value: Value,
}

#[derive(Clone)]
pub(in crate::plugin_host::external) enum Write {
    PutValue(String, Value),
    DeleteValue(String),
    PutRecord(String, Option<String>, Fields),
    DeleteRecord(String, String),
}

impl Write {
    pub(super) fn apply(
        self,
        db: &Connection,
        body: &PlatformActionRequest,
        definition: &PlatformDefinition,
    ) -> Result<()> {
        let mut request = body.clone();
        request.expected_values = None;
        match self {
            Self::PutValue(key, value) => {
                request.record_id = Some(key);
                request.collection = None;
                request.values = BTreeMap::from([("value".into(), value)]);
                PlatformStore::set_value(db, &request.plugin_id, &request)?;
            }
            Self::DeleteValue(key) => {
                db.execute(
                    "DELETE FROM ext_data_kv WHERE plugin_id=?1 AND key=?2",
                    params![request.plugin_id, key],
                )
                .checked("unavailable")?;
            }
            Self::PutRecord(collection, id, values) => {
                request.action_id = if id.is_some() {
                    PlatformAction::Update
                } else {
                    PlatformAction::Create
                };
                request.record_id = id;
                request.values = values;
                PlatformStore::change_record(
                    db,
                    &request,
                    &collection,
                    &definition.collections[&collection],
                )?;
            }
            Self::DeleteRecord(collection, id) => {
                request.action_id = PlatformAction::Delete;
                request.record_id = Some(id);
                request.values.clear();
                PlatformStore::change_record(
                    db,
                    &request,
                    &collection,
                    &definition.collections[&collection],
                )?;
            }
        }
        Ok(())
    }
}

pub(super) async fn render(
    state: &AppState,
    plugin: &str,
    selected: Selection,
    definition: PlatformDefinition,
    selector: &str,
    view: &mut PlatformSlotView,
    summary_grants: Arc<SummaryGrants>,
) -> Result<()> {
    let mut scope = Call::new(
        state,
        plugin,
        selected.clone(),
        view.grant_revision.clone(),
        definition,
        false,
    );
    scope.summary_grants = summary_grants;
    let rows = view
        .records
        .iter()
        .map(|record| {
            let handles: BTreeMap<_, _> = record
                .hydrated
                .iter()
                .filter_map(|(field, summary)| {
                    if !matches!(summary.authorization, Authorization::Authorized) {
                        return None;
                    }
                    let handle = uuid::Uuid::new_v4().to_string();
                    let collection = view.blocks.first()?.collection.as_ref()?;
                    let id = record.values.get(field)?.as_str()?;
                    scope.handles.insert(
                        handle.clone(),
                        (collection.clone(), field.clone(), id.into()),
                    );
                    Some((field, handle))
                })
                .collect();
            json!({"recordId":record.record_id,"values":record.values,"objectHandles":handles})
        })
        .collect::<Vec<_>>();
    let (reply, _) = state
        .plugins
        .invoke_platform_guest(
            plugin,
            selected,
            scope,
            "platform.render",
            json!({"slotOrViewId":selector,"records":rows}),
        )
        .await?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Render {
        record_ids: Option<Vec<String>>,
    }
    let reply: Render = serde_json::from_value(reply).checked("invalid_record")?;
    if let Some(ids) = reply.record_ids {
        let selected: BTreeSet<_> = ids.iter().collect();
        if selected.len() != ids.len() || ids.len() > view.records.len() {
            return Err(fail("invalid_record"));
        }
        let mut records: BTreeMap<_, _> = std::mem::take(&mut view.records)
            .into_iter()
            .map(|record| (record.record_id.clone(), record))
            .collect();
        view.records = ids
            .into_iter()
            .map(|id| records.remove(&id).ok_or(fail("invalid_record")))
            .collect::<Result<_>>()?;
    }
    Ok(())
}

pub(super) async fn action(
    state: &AppState,
    selected: Selection,
    definition: PlatformDefinition,
    request: &mut PlatformActionRequest,
) -> Result<Vec<Write>> {
    let scope = Call::new(
        state,
        &request.plugin_id,
        selected.clone(),
        request.expected_grant_revision.clone(),
        definition,
        true,
    );
    let (reply, writes) = state.plugins.invoke_platform_guest(&request.plugin_id, selected, scope, "platform.action",
        json!({"actionId":request.action_id,"collection":request.collection,"recordId":request.record_id,"values":request.values})).await?;
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Action {
        accept: bool,
        values: Option<Fields>,
    }
    let reply: Action = serde_json::from_value(reply).checked("invalid_record")?;
    if !reply.accept {
        return Err(fail("permission_denied"));
    }
    if let Some(values) = reply.values {
        request.values = values;
    }
    Ok(writes)
}
