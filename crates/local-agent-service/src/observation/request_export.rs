use super::request_query::{check_consent, stale};
use crate::{ApiResult, AppState, blocking};
use axum::{
    Json,
    body::Body,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::Response,
};
use local_agent_core::requests::export::{
    BeginExport, ExportChunk, ExportError, ExportInfo, FinishExport,
};
use serde::Deserialize;
use serde_json::{Value, json};

pub(crate) fn export_error(error: ExportError) -> (StatusCode, Json<Value>) {
    let (status, code) = match error {
        ExportError::Invalid => (StatusCode::BAD_REQUEST, "invalid_input"),
        ExportError::Busy => (StatusCode::SERVICE_UNAVAILABLE, "busy"),
        ExportError::Stale => (StatusCode::CONFLICT, "stale_snapshot"),
        ExportError::RecordTooLarge => (StatusCode::PAYLOAD_TOO_LARGE, "resource_limit"),
        ExportError::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
    };
    (
        status,
        Json(json!({"code":code,"message":error.to_string()})),
    )
}

async fn work<T: Send + 'static>(
    state: AppState,
    operation: impl FnOnce(
        &mut local_agent_core::observation::sql::History,
        &AppState,
    ) -> Result<T, (StatusCode, Json<Value>)>
    + Send
    + 'static,
) -> Result<T, (StatusCode, Json<Value>)> {
    blocking::run(move || {
        state
            .observation
            .hub
            .cached_query(|history| Ok(operation(history, &state)))
            .map_err(|_| export_error(ExportError::Unavailable))?
    })
    .await
    .map_err(|error| match error {
        blocking::Error::Busy => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"code":"busy", "message":"导出读取繁忙，请稍后重试"})),
        ),
        blocking::Error::Unavailable => export_error(ExportError::Unavailable),
    })?
}

async fn begin_job(
    state: AppState,
    input: BeginExport,
) -> Result<ExportInfo, (StatusCode, Json<Value>)> {
    if uuid::Uuid::parse_str(&input.operation_id).is_err()
        || input.snapshot.is_empty()
        || input.snapshot.len() > 100
    {
        return Err(export_error(ExportError::Invalid));
    }
    work(state, move |history, state| {
        let info = history
            .begin_csv_export(&input.operation_id, &input.snapshot, chrono::Utc::now())
            .map_err(export_error)?;
        let report = history
            .csv_export_report(&input.operation_id, chrono::Utc::now())
            .map_err(export_error)?;
        if let Err(error) = check_consent(state, &report) {
            let _ = history.cancel_csv_export(&input.operation_id);
            return Err(stale(error));
        }
        Ok(info)
    })
    .await
}
pub(crate) async fn begin(
    State(state): State<AppState>,
    input: Result<Json<BeginExport>, axum::extract::rejection::JsonRejection>,
) -> ApiResult {
    let Json(input) = input.map_err(|_| export_error(ExportError::Invalid))?;
    begin_job(state, input).await.map(|info| Json(json!(info)))
}

#[derive(Deserialize)]
pub(crate) struct ChunkQuery {
    after: i64,
}
async fn chunk_job(
    state: AppState,
    id: String,
    after: i64,
) -> Result<ExportChunk, (StatusCode, Json<Value>)> {
    work(state, move |history, state| {
        let report = history
            .csv_export_report(&id, chrono::Utc::now())
            .map_err(export_error)?;
        let read = (|| {
            check_consent(state, &report).map_err(stale)?;
            let chunk = history
                .csv_export_chunk(&id, after, chrono::Utc::now())
                .map_err(export_error)?;
            check_consent(state, &report).map_err(stale)?;
            Ok(chunk)
        })();
        if read.is_err() {
            let _ = history.cancel_csv_export(&id);
        }
        read
    })
    .await
}
pub(crate) async fn chunk(
    State(state): State<AppState>,
    Path(id): Path<String>,
    input: Result<Query<ChunkQuery>, axum::extract::rejection::QueryRejection>,
) -> ApiResult {
    let Query(input) = input.map_err(|_| export_error(ExportError::Invalid))?;
    chunk_job(state, id, input.after)
        .await
        .map(|part| Json(json!(part)))
}
async fn finish_job(
    state: AppState,
    id: String,
    input: FinishExport,
) -> Result<ExportInfo, (StatusCode, Json<Value>)> {
    work(state, move |history, state| {
        let report = history
            .csv_export_report(&id, chrono::Utc::now())
            .map_err(export_error)?;
        if let Err(error) = check_consent(state, &report) {
            let _ = history.cancel_csv_export(&id);
            return Err(stale(error));
        }
        history
            .finish_csv_export(&id, input.after, input.rows, chrono::Utc::now())
            .map_err(export_error)
    })
    .await
}
pub(crate) async fn finish(
    State(state): State<AppState>,
    Path(id): Path<String>,
    input: Result<Json<FinishExport>, axum::extract::rejection::JsonRejection>,
) -> ApiResult {
    let Json(input) = input.map_err(|_| export_error(ExportError::Invalid))?;
    finish_job(state, id, input)
        .await
        .map(|info| Json(json!(info)))
}
pub(crate) async fn cancel(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    work(state, move |history, _| {
        history.cancel_csv_export(&id).map_err(export_error)
    })
    .await?;
    Ok(Json(json!({"status":"cancelled"})))
}

// The legacy response shape remains compatible, but its CSV string is escaped
// incrementally. A slow/closed receiver never retains the query lock or queues
// the whole export in memory. Idle leases also expire if cleanup cannot run.
struct Cleanup {
    state: AppState,
    id: String,
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        let state = self.state.clone();
        let id = self.id.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = work(state, move |history, _| {
                    history.cancel_csv_export(&id).map_err(export_error)
                })
                .await;
            });
        }
    }
}
#[derive(Deserialize)]
pub(crate) struct LegacyQuery {
    snapshot: String,
}
pub(crate) async fn legacy(
    State(state): State<AppState>,
    Query(input): Query<LegacyQuery>,
) -> Result<Response, (StatusCode, Json<Value>)> {
    let id = uuid::Uuid::new_v4().to_string();
    let info = begin_job(
        state.clone(),
        BeginExport {
            operation_id: id.clone(),
            snapshot: input.snapshot,
        },
    )
    .await?;
    let cleanup = Cleanup {
        state: state.clone(),
        id: id.clone(),
    };
    let stream = async_stream::try_stream! {
        let _cleanup = cleanup;
        yield axum::body::Bytes::from_static(b"{\"csv\":\"");
        let mut after = -1;
        let mut rows = 0u64;
        loop {
            let part = chunk_job(state.clone(), id.clone(), after).await.map_err(|_| std::io::Error::other("CSV export interrupted"))?;
            rows += part.rows;
            // JSON escaping remains bounded by one source batch.
            let encoded = serde_json::to_string(&part.csv).map_err(std::io::Error::other)?;
            yield axum::body::Bytes::copy_from_slice(&encoded.as_bytes()[1..encoded.len()-1]);
            after += part.rows as i64;
            if part.next_after.is_none() { break; }
        }
        finish_job(state, id, FinishExport { after, rows }).await.map_err(|_| std::io::Error::other("CSV export authorization changed"))?;
        let as_of = serde_json::to_string(&info.as_of).map_err(std::io::Error::other)?;
        let snapshot = serde_json::to_string(&info.snapshot).map_err(std::io::Error::other)?;
        let tail = format!("\",\"count\":{},\"asOf\":{as_of},\"snapshot\":{snapshot}}}", info.count);
        yield axum::body::Bytes::from(tail);
    };
    let stream: std::pin::Pin<
        Box<dyn futures_util::Stream<Item = Result<axum::body::Bytes, std::io::Error>> + Send>,
    > = Box::pin(stream);
    let mut response = Response::new(Body::from_stream(stream));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/json"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    Ok(response)
}
