//! Recovery remains available independently of guest activation and package health.
use super::super::{Checked, Result, fail, run_sync};
use crate::{ApiResult, AppState};
use axum::{Json, extract::State, http::StatusCode};
use local_agent_core::domain::routing_policy_recovery::{
    PolicyRecoveryAction as Action, PolicyRecoveryReply as Reply,
};

pub(super) async fn perform(State(state): State<AppState>, Json(input): Json<Action>) -> ApiResult {
    let result = run_sync(move || apply(&state, input)).await;
    match result {
        Ok(reply) => serde_json::to_value(reply)
            .map(Json)
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"code":"invalid_record","message":"恢复回复暂不可读取，请核查原范围。","outcomeUnknown":true})))),
        Err(e) => Err((
            if e.code == "stale_snapshot" { StatusCode::CONFLICT } else if e.code == "busy" { StatusCode::SERVICE_UNAVAILABLE } else { StatusCode::BAD_REQUEST },
            Json(serde_json::json!({"code":e.code,"message":if e.code == "scope_recovery_unknown" { "恢复结果待核查，请保留此核查编号并重新读取；没有自动重新授权或发布。" } else if e.code == "result_pending" { "仍有原发布或证据不完整，请先核查原编号；本次没有改写范围。" } else { super::super::message(e.code) },"outcomeUnknown":e.code == "scope_recovery_unknown"})),
        )),
    }
}
fn apply(state: &AppState, action: Action) -> Result<Reply> {
    let store = state.store.lock().checked("unavailable")?;
    let store_id = match &action {
        Action::List { store_id } => store_id,
        Action::Inspect { scope } => &scope.store_id,
        Action::Resolve { reference, .. } => &reference.scope.store_id,
    };
    store
        .require_model_service_store(store_id)
        .checked("stale_snapshot")?;
    let mut host = state.plugins.policy.lock().checked("unavailable")?;
    let reply = match action {
        Action::List { store_id } => {
            let (entries, limited) = host.recovery_list(&store, &store_id)?;
            Reply::Listed {
                store_id,
                entries,
                limited,
            }
        }
        Action::Inspect { scope } => Reply::Inspected {
            inspection: Box::new(host.recovery_inspect(&store, scope)?),
        },
        Action::Resolve { reference, mode } => Reply::Resolved {
            revision: host.recovery_resolve(&store, reference.clone(), mode.clone())?,
            reference,
            mode,
        },
    };
    if serde_json::to_vec(&reply).checked("invalid_record")?.len() > 1024 * 1024 {
        return Err(fail("resource_limit"));
    }
    Ok(reply)
}
