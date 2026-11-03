use super::{Action, Checked, Manager, Query, Request, Result, fail};
use crate::plugin_host::external::diagnostics::Operation;
use crate::plugin_host::external::run_sync;
use crate::{ApiResult, AppState};
use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use local_agent_core::Provider;
use serde::Deserialize;
use serde_json::{Value, json};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/plugin-extensions/status", post(status))
        .route("/api/plugin-extensions/resource", post(resource))
        .route("/api/plugin-extensions/action", post(action))
        .route("/api/plugin-extensions/export", post(export))
        .layer(axum::extract::DefaultBodyLimit::max(1048576))
}
fn response(r: Result<Value>) -> ApiResult {
    r.map(Json).map_err(|error| {
        let status = match error.code {
            "permission_denied" | "grant_revoked" => StatusCode::FORBIDDEN,
            "not_found" => StatusCode::NOT_FOUND,
            "stale_snapshot" | "result_pending" => StatusCode::CONFLICT,
            "invalid_record" | "busy" => StatusCode::SERVICE_UNAVAILABLE,
            _ => StatusCode::BAD_REQUEST,
        };
        let message = match error.code {
            "stale_snapshot" => "配置、连接或授权已变化，请重新选择文件并预览。",
            "result_pending" => "原操作结果待核查；保留备份，未创建重复操作。",
            "unverified_connector" => "DSH 连接器尚未通过指定实例验证，当前不会连接。",
            _ => crate::plugin_host::external::message(error.code),
        };
        (status, Json(json!({"code":error.code,"message":message})))
    })
}
async fn synchronous(work: impl FnOnce() -> Result<Value> + Send + 'static) -> ApiResult {
    response(run_sync(work).await)
}
async fn status(State(s): State<AppState>, Json(q): Json<Query>) -> ApiResult {
    synchronous(move || {
        if q.plugin_id.is_empty() && q.digest.is_empty() {
            s.plugins.extension_history()
        } else {
            s.plugins.extension_status(&q)
        }
    })
    .await
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Resource {
    plugin_id: String,
    digest: String,
    path: String,
}
async fn resource(State(s): State<AppState>, Json(r): Json<Resource>) -> ApiResult {
    synchronous(move || {
        s.plugins.grant_extension_file(
            &Query {
                plugin_id: r.plugin_id,
                digest: r.digest,
            },
            std::path::Path::new(&r.path),
        )
    })
    .await
}
async fn export(State(s): State<AppState>, Json(q): Json<Query>) -> ApiResult {
    synchronous(move || s.plugins.export_connector(&q)).await
}
fn provider(s: &AppState, id: &str) -> Result<Provider> {
    s.store
        .lock()
        .checked("unavailable")?
        .providers()
        .checked("unavailable")?
        .into_iter()
        .find(|p| p.id == id)
        .ok_or(fail("not_found"))
}
async fn action(State(s): State<AppState>, Json(r): Json<Request>) -> ApiResult {
    let operation = match &r.action {
        Action::BindProtocol { .. } => Operation::BindProtocol,
        Action::UnbindProtocol => Operation::UnbindProtocol,
        Action::PrepareConfig { .. } => Operation::PrepareConfig,
        Action::ApplyConfig { .. } => Operation::ApplyConfig,
        Action::RestoreConfig { .. } => Operation::RestoreConfig,
        Action::DiscardConfig { .. } => Operation::DiscardConfig,
        Action::StartConnector => Operation::StartConnector,
        Action::StopConnector => Operation::StopConnector,
        Action::RefreshConnector => Operation::RefreshConnector,
    };
    let mut observed = s.plugins.observe(&r.plugin_id, &r.digest, operation);
    let q = Query {
        plugin_id: r.plugin_id,
        digest: r.digest,
    };
    let result = match r.action {
        Action::PrepareConfig { provider_id } => {
            let state = s.clone();
            let selected = run_sync(move || provider(&state, &provider_id)).await;
            match selected {
                Ok(selected) => s.plugins.prepare_config(&q, &selected).await,
                Err(error) => Err(error),
            }
        }
        Action::StartConnector => {
            let state = s.clone();
            let query = q.clone();
            let selected = run_sync(move || {
                let g = state.plugins.extension_grant(&query)?;
                let c: super::Connection =
                    serde_json::from_slice(&super::checked_bytes(&g)?).checked("invalid_input")?;
                if let super::Connection::Http {
                    provider_id: Some(id),
                    ..
                } = c
                {
                    provider(&state, &id).map(Some)
                } else {
                    Ok(None)
                }
            })
            .await;
            match selected {
                Ok(selected) => {
                    s.plugins
                        .start_connector(&q, selected, s.store.clone())
                        .await
                }
                Err(error) => Err(error),
            }
        }
        action => {
            run_sync(move || match action {
                Action::BindProtocol {
                    provider_id,
                    allow_private_network,
                } => {
                    s.plugins
                        .bind_protocol(&q, &provider(&s, &provider_id)?, allow_private_network)
                }
                Action::UnbindProtocol => s.plugins.unbind_protocol(&q),
                Action::ApplyConfig { plan_id, revision } => {
                    let id = s.plugins.config_provider_id(&q, &plan_id)?;
                    s.plugins
                        .apply_config(&q, &plan_id, &revision, &provider(&s, &id)?)
                }
                Action::RestoreConfig { operation_id } => {
                    s.plugins.restore_config(&q, &operation_id)
                }
                Action::DiscardConfig { plan_id } => s.plugins.discard_config(&q, &plan_id),
                Action::StopConnector => s.plugins.stop_connector(&q),
                Action::RefreshConnector => s.plugins.extension_status(&q),
                Action::PrepareConfig { .. } | Action::StartConnector => unreachable!(),
            })
            .await
        }
    };
    observed.finish(&result);
    response(result)
}
impl Manager {
    pub(crate) fn extension_history(&self) -> Result<Value> {
        let store = self.advanced.lock().checked("unavailable")?;
        store.ready()?;
        let mut stmt = store
            .db()?
            .prepare("SELECT record FROM plans ORDER BY rowid DESC LIMIT 64")
            .checked("unavailable")?;
        let plans = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .checked("unavailable")?
            .collect::<std::result::Result<Vec<_>, _>>()
            .checked("unavailable")?
            .iter()
            .map(|r| {
                serde_json::from_str::<super::config::Plan>(r)
                    .checked("invalid_record")
                    .map(|p| p.public())
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(
            json!({"plans":plans,"planCapacity":super::config::capacity(store.db()?)?,"dsh":"unverified","cleanup":"unsupported","redemption":"unsupported"}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreadable_records_and_admission_failures_have_stable_non_sensitive_errors() {
        for code in ["invalid_record", "busy"] {
            let (status, Json(body)) = response(Err(fail(code))).unwrap_err();
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(body["code"], code);
            assert_eq!(body["message"], crate::plugin_host::external::message(code));
        }
        let (_, Json(body)) = response(Err(fail("invalid_record"))).unwrap_err();
        assert!(
            body["message"]
                .as_str()
                .unwrap()
                .contains("保留原文件与备份")
        );
        assert_eq!(body.as_object().unwrap().len(), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn synchronous_adapter_yields_and_does_not_cancel_an_admitted_worker() {
        let entered = std::sync::Arc::new(tokio::sync::Notify::new());
        let waiting = entered.notified();
        let (release, gate) = std::sync::mpsc::channel();
        let (finished, done) = tokio::sync::oneshot::channel();
        let task = tokio::spawn({
            let entered = entered.clone();
            async move {
                synchronous(move || {
                    entered.notify_one();
                    gate.recv_timeout(std::time::Duration::from_secs(3))
                        .unwrap();
                    let _ = finished.send(());
                    Ok(json!({"plans":[]}))
                })
                .await
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), waiting)
            .await
            .unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            tokio::time::sleep(std::time::Duration::from_millis(10)),
        )
        .await
        .unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        release.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), done)
            .await
            .unwrap()
            .unwrap();
    }
}
