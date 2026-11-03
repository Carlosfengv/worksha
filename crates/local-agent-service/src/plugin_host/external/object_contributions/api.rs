use super::super::{Checked, Result};
use crate::{ApiResult, AppState};
use axum::{Extension, Json, Router, extract::State, routing::post};
use local_agent_core::plugin::object_contributions::{
    ObjectContributionAction as A, ObjectContributionReply as R,
};
use std::sync::Arc;
use std::time::Duration;

#[cfg(test)]
mod tests;

#[derive(Clone)]
struct ReadBudget {
    running: Arc<tokio::sync::Semaphore>,
    admitted: Arc<tokio::sync::Semaphore>,
}
impl ReadBudget {
    fn new() -> Self {
        Self {
            running: Arc::new(tokio::sync::Semaphore::new(2)),
            admitted: Arc::new(tokio::sync::Semaphore::new(32)),
        }
    }

    async fn run(&self, work: impl FnOnce() -> ApiResult + Send + 'static) -> ApiResult {
        // An object switch releases old references while mounting new slots.
        // Absorb that short burst without enlarging the blocking worker pool or
        // creating an unbounded queue. A queued caller never starts after timeout.
        let admitted = self
            .admitted
            .clone()
            .try_acquire_owned()
            .map_err(|_| read_error("busy"))?;
        let running = tokio::time::timeout(
            Duration::from_millis(250),
            self.running.clone().acquire_owned(),
        )
        .await
        .map_err(|_| read_error("busy"))?
        .map_err(|_| read_error("unavailable"))?;
        tokio::task::spawn_blocking(move || {
            // Dropping the HTTP waiter cannot free a still-running worker.
            let _permits = (admitted, running);
            work()
        })
        .await
        .map_err(|_| read_error("unavailable"))?
    }
}

fn read_error(code: &str) -> (axum::http::StatusCode, Json<serde_json::Value>) {
    let message = if code == "busy" {
        "数据读取繁忙，请稍后重试"
    } else {
        "数据读取暂不可用，请稍后重试"
    };
    (
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({"code":code,"message":message})),
    )
}

pub(in crate::plugin_host::external) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/plugins/object-contributions", post(perform))
        .layer(Extension(ReadBudget::new()))
}

async fn perform(
    State(state): State<AppState>,
    Extension(budget): Extension<ReadBudget>,
    Json(input): Json<A>,
) -> ApiResult {
    // Optional detail slots cannot consume the base page's query permits.
    budget
        .run(move || {
            super::super::api::response(
                dispatch(&state, input)
                    .and_then(|reply| serde_json::to_value(reply).checked("invalid_record")),
            )
        })
        .await
}
fn dispatch(state: &AppState, input: A) -> Result<R> {
    let manager = &state.plugins;
    let reader = crate::domain::object_summary::Source(state);
    match input {
        A::Catalog {} => Ok(R::Catalog {
            catalog: manager.object_contribution_catalog()?,
        }),
        A::Grant { request } => Ok(R::Status {
            status: manager.object_contribution_grant(request, true)?,
        }),
        A::Revoke { request } => Ok(R::Status {
            status: manager.object_contribution_grant(request, false)?,
        }),
        A::Issue { request } => Ok(R::Reference {
            reference: manager.issue_object_reference(request, &reader)?,
        }),
        A::Read { query } => manager.read_object_reference(query, &reader),
        A::Release { query } => manager.release_object_reference(query),
    }
}
