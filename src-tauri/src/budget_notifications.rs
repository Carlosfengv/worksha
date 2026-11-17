use crate::backend::Backend;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;
use tauri::Manager;
use tauri_plugin_notification::{NotificationExt, PermissionState};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pending {
    store_id: String,
    warnings: Vec<Warning>,
}
#[derive(Deserialize)]
struct Warning {
    id: String,
}

pub(crate) fn start(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut timer = tokio::time::interval(Duration::from_secs(5));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            timer.tick().await;
            let Ok(context) = app.state::<Backend>().context() else {
                continue;
            };
            let Ok(raw) = context
                .request(
                    reqwest::Method::GET,
                    "/api/agents/budget-warnings?pending=true",
                    None,
                )
                .await
            else {
                continue;
            };
            let Ok(pending) = serde_json::from_value::<Pending>(raw) else {
                continue;
            };
            if pending.warnings.is_empty()
                || context.ensure_current().is_err()
                || !matches!(
                    app.notification().permission_state(),
                    Ok(PermissionState::Granted)
                )
            {
                continue;
            }
            let count = pending.warnings.len();
            let body = if count == 1 {
                "一个本机绑定的预算窗口已达到预警阈值。".to_owned()
            } else {
                format!("{count} 个本机绑定预算窗口已达到预警阈值。")
            };
            if app
                .notification()
                .builder()
                .title("Worksha · 本机预算预警")
                .body(body)
                .show()
                .is_ok()
            {
                let ids = pending
                    .warnings
                    .into_iter()
                    .map(|item| item.id)
                    .collect::<Vec<_>>();
                let _ = context
                    .request(
                        reqwest::Method::POST,
                        "/api/agents/budget-warnings",
                        Some(json!({"storeId":pending.store_id,"ids":ids})),
                    )
                    .await;
            }
        }
    });
}
