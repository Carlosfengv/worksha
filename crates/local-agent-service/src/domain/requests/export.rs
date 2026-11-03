//! Revalidates captured authority outside the SQL lock before releasing bytes.
use super::{capture, q};
use crate::{ApiResult, AppState, blocking, observation::request_export::export_error};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use local_agent_core::{domain::requests::export as x, requests::export::ExportError};
use serde_json::Value;
type Failure = (StatusCode, Json<Value>);

#[cfg(test)]
mod tests;

enum Action {
    Begin(x::Begin),
    Read(x::Read),
    Finish(x::Finish),
}
fn sql<T>(
    state: &AppState,
    work: impl FnOnce(&mut local_agent_core::observation::sql::History) -> Result<T, ExportError>,
) -> Result<T, Failure> {
    state
        .observation
        .cached_sql_query(|h| Ok(work(h)))
        .map_err(|_| export_error(ExportError::Unavailable))?
        .map_err(export_error)
}
fn query(state: &AppState, id: &str, action: &Action) -> Result<q::UnifiedRequestQuery, Failure> {
    sql(state, |h| match action {
        Action::Begin(input) => {
            input.validate()?;
            h.unified_csv_export_query(id, chrono::Utc::now())
                .or_else(|_| {
                    h.object_request_query(&input.snapshot_token)
                        .map_err(|_| ExportError::Stale)
                })
        }
        _ => h.unified_csv_export_query(id, chrono::Utc::now()),
    })
}
fn signature(state: &AppState, input: &q::UnifiedRequestQuery) -> Result<String, Failure> {
    capture(state, input)
        .map(|c| c.source_signature)
        .map_err(|e| crate::plugin_host::external::api::response(Err(e)).unwrap_err())
}
fn run(
    state: &AppState,
    id: &str,
    action: Action,
    before_finish: &dyn Fn(),
) -> Result<Value, Failure> {
    if uuid::Uuid::parse_str(id).is_err() {
        return Err(export_error(ExportError::Invalid));
    }
    let begin = matches!(action, Action::Begin(_));
    let mut owned = !begin;
    let result = (|| {
        let input = query(state, id, &action)?;
        let before = signature(state, &input)?;
        let value = match &action {
            Action::Begin(input) => {
                let info = sql(state, |h| {
                    h.begin_unified_csv_export(input, &before, &state.sessions, chrono::Utc::now())
                })?;
                owned = true;
                Some(
                    serde_json::to_value(info)
                        .map_err(|_| export_error(ExportError::Unavailable))?,
                )
            }
            Action::Read(input) => Some(
                serde_json::to_value(sql(state, |h| {
                    h.unified_csv_export_chunk(
                        id,
                        input.offset.value(),
                        &before,
                        &state.sessions,
                        chrono::Utc::now(),
                    )
                })?)
                .map_err(|_| export_error(ExportError::Unavailable))?,
            ),
            Action::Finish(_) => {
                sql(state, |h| {
                    h.check_unified_csv_export(id, &before, &state.sessions, chrono::Utc::now())
                })?;
                None
            }
        };
        before_finish();
        let after = signature(state, &input)?;
        match action {
            Action::Finish(input) => serde_json::to_value(sql(state, |h| {
                h.finish_unified_csv_export(id, &input, &after, &state.sessions, chrono::Utc::now())
            })?)
            .map_err(|_| export_error(ExportError::Unavailable)),
            _ => {
                sql(state, |h| {
                    h.check_unified_csv_export(id, &after, &state.sessions, chrono::Utc::now())
                })?;
                value.ok_or_else(|| export_error(ExportError::Unavailable))
            }
        }
    })();
    if owned && result.is_err() {
        let _ = sql(state, |h| h.cancel_unified_csv_export(id));
    }
    result
}
async fn work(state: AppState, id: String, action: Action) -> ApiResult {
    blocking::run(move || run(&state, &id, action, &|| {}))
        .await
        .map_err(|e| {
            export_error(match e {
                blocking::Error::Busy => ExportError::Busy,
                blocking::Error::Unavailable => ExportError::Unavailable,
            })
        })?
        .map(Json)
}
pub(crate) async fn begin(
    State(state): State<AppState>,
    input: Result<Json<x::Begin>, axum::extract::rejection::JsonRejection>,
) -> ApiResult {
    let Json(input) = input.map_err(|_| export_error(ExportError::Invalid))?;
    work(state, input.operation_id.clone(), Action::Begin(input)).await
}
pub(crate) async fn chunk(
    State(state): State<AppState>,
    Path(id): Path<String>,
    input: Result<Query<x::Read>, axum::extract::rejection::QueryRejection>,
) -> ApiResult {
    let Query(input) = input.map_err(|_| export_error(ExportError::Invalid))?;
    work(state, id, Action::Read(input)).await
}
pub(crate) async fn finish(
    State(state): State<AppState>,
    Path(id): Path<String>,
    input: Result<Json<x::Finish>, axum::extract::rejection::JsonRejection>,
) -> ApiResult {
    let Json(input) = input.map_err(|_| export_error(ExportError::Invalid))?;
    work(state, id, Action::Finish(input)).await
}
pub(crate) async fn cancel(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    if uuid::Uuid::parse_str(&id).is_err() {
        return Err(export_error(ExportError::Invalid));
    }
    blocking::run(move || sql(&state, |h| h.cancel_unified_csv_export(&id)))
        .await
        .map_err(|_| export_error(ExportError::Unavailable))??;
    Ok(Json(serde_json::json!({"status":"cancelled"})))
}
