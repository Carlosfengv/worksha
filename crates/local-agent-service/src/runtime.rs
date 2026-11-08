use crate::{AppState, api};
use axum::{
    Json, Router,
    extract::State,
    middleware,
    routing::{get, post},
};
use local_agent_core::runtime::{IAM_PROTOCOL, IAM_PROTOCOL_HEADER, Identity};
use tokio::sync::watch;

#[derive(Clone)]
pub(crate) struct Runtime {
    identity: Identity,
    shutdown: watch::Sender<bool>,
}

impl Runtime {
    pub(crate) fn new(identity: Identity) -> Self {
        Self {
            identity,
            shutdown: watch::channel(false).0,
        }
    }

    pub(crate) async fn stopped(&self) {
        let mut receiver = self.shutdown.subscribe();
        let _ = receiver.wait_for(|stopped| *stopped).await;
    }

    pub(crate) fn stop(&self) {
        self.shutdown.send_replace(true);
    }

    pub(crate) fn routes(&self, state: AppState) -> Router {
        Router::new()
            .route("/api/runtime", get(identity))
            .route("/api/runtime/shutdown", post(shutdown))
            .with_state(self.clone())
            .layer(middleware::from_fn_with_state(state, api::authorize))
    }
}

async fn identity(
    State(runtime): State<Runtime>,
) -> ([(&'static str, &'static str); 1], Json<Identity>) {
    (
        [(IAM_PROTOCOL_HEADER, IAM_PROTOCOL)],
        Json(runtime.identity),
    )
}

async fn shutdown(State(runtime): State<Runtime>) -> Json<serde_json::Value> {
    // Both listeners drain existing requests before the process exits.
    runtime.stop();
    Json(serde_json::json!({"stopping": true}))
}
