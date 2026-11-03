use crate::{ApiResult, AppState, json_value};
use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
};
use local_agent_core::observation::retention::impact::{self, Request};
use serde::Serialize;

fn failure(
    code: &'static str,
    message: &'static str,
    status: StatusCode,
) -> (StatusCode, Json<serde_json::Value>) {
    (
        status,
        Json(serde_json::json!({"code":code,"message":message})),
    )
}
fn unavailable() -> (StatusCode, Json<serde_json::Value>) {
    failure(
        "retention_unavailable",
        "留存预览暂不可用，原始记录保持不变",
        StatusCode::SERVICE_UNAVAILABLE,
    )
}

fn scope(state: &AppState, request: &Request) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    let store = state.store.lock().map_err(|_| unavailable())?;
    if store.database_id().map_err(|_| unavailable())? != request.store_id {
        return Err(failure(
            "retention_store_changed",
            "后台数据库已变化，请刷新设置后重新预览",
            StatusCode::CONFLICT,
        ));
    }
    if store.settings_state().map_err(|_| unavailable())?.revision
        != request.expected_settings_revision
    {
        return Err(failure(
            "retention_settings_changed",
            "设置已有新版本，请刷新后重新预览",
            StatusCode::CONFLICT,
        ));
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Preview {
    version: u32,
    store_id: String,
    settings_revision: u64,
    days: u16,
    captured_at: String,
    cutoff: String,
    valid_until: String,
    cleanup_enabled: bool,
    extra_archive_enabled: bool,
    confirmation_available: bool,
    scope: &'static str,
    other_sources: &'static str,
    aggregate_retention: &'static str,
    gateway: impact::GatewayImpact,
    pending_writes: usize,
    writer_issue: bool,
}

pub(crate) async fn preview(
    State(state): State<AppState>,
    Query(request): Query<Request>,
) -> ApiResult {
    if !request.valid() {
        return Err(failure(
            "invalid_input",
            "请选择有效的保留天数并刷新本机设置后重试",
            StatusCode::BAD_REQUEST,
        ));
    }
    crate::blocking::query(state.query_capacity.clone(), move || {
        scope(&state, &request)?;
        let now = chrono::Utc::now();
        let gateway = state
            .observation
            .retention_impact(request.days, now)
            .map_err(|error| {
                use impact::Error;
                let (code, message, status) = match error {
                    Error::InvalidInput => {
                        ("invalid_input", "保留天数无效", StatusCode::BAD_REQUEST)
                    }
                    Error::SourceAbsent => (
                        "retention_source_absent",
                        "观测来源文件不存在，尚不能确认历史为空",
                        StatusCode::SERVICE_UNAVAILABLE,
                    ),
                    Error::SourceChanged => (
                        "retention_source_changed",
                        "观测来源已变化，请重新预览",
                        StatusCode::CONFLICT,
                    ),
                    Error::UnsupportedSchema => (
                        "retention_source_unsupported",
                        "观测来源结构尚不受支持，原记录已保留",
                        StatusCode::SERVICE_UNAVAILABLE,
                    ),
                    Error::Limit => (
                        "retention_preview_limit",
                        "历史超过本次预览的容量或时间限制，未返回部分数量",
                        StatusCode::SERVICE_UNAVAILABLE,
                    ),
                    Error::Unavailable => (
                        "retention_unavailable",
                        "留存预览读取未完成，原记录已保留",
                        StatusCode::SERVICE_UNAVAILABLE,
                    ),
                };
                failure(code, message, status)
            })?;
        scope(&state, &request)?;
        let health = state.observation.retention_writer_health();
        let cutoff = now - chrono::Duration::days(i64::from(request.days));
        json_value(Preview {
            version: 1,
            store_id: request.store_id,
            settings_revision: request.expected_settings_revision,
            days: request.days,
            captured_at: now.to_rfc3339(),
            cutoff: cutoff.to_rfc3339(),
            valid_until: (now + chrono::Duration::minutes(5)).to_rfc3339(),
            cleanup_enabled: false,
            extra_archive_enabled: false,
            confirmation_available: false,
            scope: "committed_gateway_only",
            other_sources: "not_checked",
            aggregate_retention: "continuous",
            gateway,
            pending_writes: health.pending,
            writer_issue: health.error.is_some() || health.dropped_updates > 0,
        })
    })
    .await
}

#[derive(Serialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
enum SourceReport<T> {
    Checked {
        read_started_at: String,
        read_finished_at: String,
        value: T,
    },
    Unverified {
        reason: &'static str,
    },
}
fn source_report<T>(read: impl FnOnce() -> Result<T, impact::Error>) -> SourceReport<T> {
    let begin = chrono::Utc::now();
    match read() {
        Ok(value) => SourceReport::Checked {
            read_started_at: begin.to_rfc3339(),
            read_finished_at: chrono::Utc::now().to_rfc3339(),
            value,
        },
        Err(error) => SourceReport::Unverified {
            reason: match error {
                impact::Error::InvalidInput => "invalid_input",
                impact::Error::SourceAbsent => "source_absent",
                impact::Error::SourceChanged => "source_changed",
                impact::Error::UnsupportedSchema => "unsupported_schema",
                impact::Error::Unavailable => "unavailable",
                impact::Error::Limit => "limit",
            },
        },
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Inventory {
    version: u32,
    store_id: String,
    settings_revision: u64,
    days: u16,
    captured_at: String,
    cutoff: String,
    valid_until: String,
    cleanup_enabled: bool,
    extra_archive_enabled: bool,
    confirmation_available: bool,
    scope: &'static str,
    remaining_sources: &'static str,
    aggregate_retention: &'static str,
    gateway: SourceReport<impact::GatewayImpact>,
    tools: SourceReport<impact::ToolImpact>,
    tool_history: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    activity_caches: Option<ActivityCaches>,
    pending_writes: usize,
    writer_issue: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ActivityCaches {
    gateway: SourceReport<impact::ActivityImpact>,
    native: SourceReport<impact::ActivityImpact>,
}

pub(crate) async fn inventory(
    State(state): State<AppState>,
    Query(request): Query<impact::InventoryRequest>,
) -> ApiResult {
    if !request.valid() {
        return Err(failure(
            "invalid_input",
            "请选择有效的保留天数并刷新本机设置后重试",
            StatusCode::BAD_REQUEST,
        ));
    }
    let version = request.inventory_version;
    let request = request.scope();
    crate::blocking::query(state.query_capacity.clone(), move || {
        scope(&state, &request)?;
        let now = chrono::Utc::now();
        // Each source owns its snapshot. These two WAL databases do not form
        // an atomic inventory or deletion authorization.
        let gateway = source_report(|| state.observation.retention_impact(request.days, now));
        let tools = source_report(|| state.plugins.retention_tool_impact(request.days, now));
        let activity_caches = (version == 2).then(|| ActivityCaches {
            gateway: source_report(|| {
                state.observation.retention_activity_impact(
                    impact::ActivitySource::Gateway,
                    request.days,
                    now,
                )
            }),
            native: source_report(|| {
                state.observation.retention_activity_impact(
                    impact::ActivitySource::Native,
                    request.days,
                    now,
                )
            }),
        });
        scope(&state, &request)?;
        let health = state.observation.retention_writer_health();
        json_value(Inventory {
            version: u32::from(version),
            store_id: request.store_id,
            settings_revision: request.expected_settings_revision,
            days: request.days,
            captured_at: now.to_rfc3339(),
            cutoff: (now - chrono::Duration::days(i64::from(request.days))).to_rfc3339(),
            valid_until: (now + chrono::Duration::minutes(5)).to_rfc3339(),
            cleanup_enabled: false,
            extra_archive_enabled: false,
            confirmation_available: false,
            scope: if version == 2 {
                "product_owned_saved_sources"
            } else {
                "product_owned_saved_metadata"
            },
            remaining_sources: "not_checked",
            aggregate_retention: "continuous",
            gateway,
            tools,
            activity_caches,
            tool_history: "saved_only_prior_history_unknown",
            pending_writes: health.pending,
            writer_issue: health.error.is_some() || health.dropped_updates > 0,
        })
    })
    .await
}
