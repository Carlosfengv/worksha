use crate::AppState;
use axum::{
    Json,
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use local_agent_core::sessions::NativeError;

pub(crate) async fn authorize(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let headers = request.headers();
    let expected = format!("Bearer {}", state.token);
    if headers.get("host").and_then(|h| h.to_str().ok()) != Some(state.host.as_str())
        || headers.contains_key("origin")
        || headers.get("authorization").and_then(|h| h.to_str().ok()) != Some(expected.as_str())
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"message":"未授权访问"})),
        )
            .into_response();
    }
    next.run(request).await
}

pub(crate) type ApiResult = Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)>;

pub(crate) fn error(message: String) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({"message": message})),
    )
}

pub(crate) fn native_error(error: NativeError) -> (StatusCode, Json<serde_json::Value>) {
    let status = match error.code {
        "not_found" => StatusCode::NOT_FOUND,
        "unauthorized" => StatusCode::FORBIDDEN,
        "stale_snapshot" | "project_hidden" => StatusCode::CONFLICT,
        "unavailable" => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::BAD_REQUEST,
    };
    (
        status,
        Json(serde_json::json!({"code":error.code,"message":error.message})),
    )
}
pub(crate) fn json_value(value: impl serde::Serialize) -> ApiResult {
    serde_json::to_value(value)
        .map(Json)
        .map_err(|_| error("读取失败".into()))
}
