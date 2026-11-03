//! Loopback HTTP fixtures: no real account, native configuration or model request.
use super::*;
use axum::{
    body::Bytes,
    response::{IntoResponse, Response},
};
use local_agent_core::Provider;
use std::time::Duration;

pub fn upstream(body: &Value) -> Option<Response> {
    let mode = body["max_tokens"]
        .as_u64()
        .or_else(|| body["max_output_tokens"].as_u64())?;
    if !(790..=796).contains(&mode) {
        return None;
    }
    if mode == 795 {
        return Some((axum::http::StatusCode::UNPROCESSABLE_ENTITY,axum::Json(json!({"error":{"message":"messages content must be string; upstream-secret-fixture"}}))).into_response());
    }
    let usage = json!({"prompt_tokens":100,"completion_tokens":7,"prompt_cache_hit_tokens":80});
    let message = match mode {
        790 => {
            json!({"role":"assistant","content":null,"reasoning_content":"reasoning-private-sentinel","refusal":"refusal-private-sentinel"})
        }
        791 => json!({"role":"assistant","content":null,"reasoning_content":42}),
        796 => {
            json!({"role":"assistant","content":null,"tool_calls":[{"id":"bad","function":{"name":"read","arguments":"{"}}]})
        }
        _ => json!({"role":"assistant","content":"response-private-sentinel"}),
    };
    if body["stream"] != true {
        return Some(axum::Json(json!({"id":"chat-json","model":body["model"],"choices":[{"message":message,"finish_reason":if mode==796 {"tool_calls"}else{"stop"}}],"usage":usage})).into_response());
    }
    let mut delta = message;
    delta.as_object_mut().unwrap().remove("role");
    let initial =
        event(json!({"id":"chat-stream","model":body["model"],"choices":[{"delta":delta}]}));
    let end = event(
        json!({"id":"chat-stream","choices":[{"delta":{},"finish_reason":"stop"}],"usage":usage}),
    ) + "data: [DONE]\n\n";
    let stream = async_stream::stream! {
        yield Ok::<_,std::io::Error>(Bytes::from(initial));
        tokio::time::sleep(Duration::from_millis(if mode==794 {5000}else{80})).await;
        if mode!=793 && mode!=794 { yield Ok(Bytes::from(end)); }
    };
    Some(
        (
            [("content-type", "text/event-stream")],
            axum::body::Body::from_stream(stream),
        )
            .into_response(),
    )
}

pub async fn verify(state: &crate::AppState, client: &reqwest::Client, original: &Provider) {
    let before_all = state.observation.hub_rows_for_test().len();
    for agent in ["claude", "codex"] {
        let mut p = original.clone();
        p.vendor = "deepseek".into();
        p.protocol = "chat-completions".into();
        let (rendered, local) = {
            let manager = state.providers.lock().unwrap();
            let (rendered, _) = state.gateway.prepare(&manager, agent, &p).unwrap();
            let secret = String::from_utf8(
                manager
                    .secret(rendered.credential_ref.as_ref().unwrap())
                    .unwrap()
                    .to_vec(),
            )
            .unwrap();
            (rendered, secret)
        };
        let url = format!(
            "{}{}",
            rendered.endpoint,
            if agent == "codex" {
                "/responses"
            } else {
                "/v1/messages"
            }
        );
        for (mode, streamed) in [
            (790, false),
            (790, true),
            (791, false),
            (791, true),
            (792, true),
            (793, true),
            (794, true),
            (795, false),
            (796, false),
        ] {
            let before: std::collections::HashSet<_> = state
                .observation
                .hub_rows_for_test()
                .into_iter()
                .map(|r| r.id)
                .collect();
            let body = if agent == "codex" {
                json!({"model":"chosen-model","input":"request-private-sentinel","stream":streamed,"max_output_tokens":mode})
            } else {
                json!({"model":"chosen-model","messages":[{"role":"user","content":"request-private-sentinel"}],"stream":streamed,"max_tokens":mode})
            };
            let mut response = client
                .post(&url)
                .bearer_auth(&local)
                .json(&body)
                .send()
                .await
                .unwrap();
            if (792..=794).contains(&mode) {
                let mut received = String::new();
                while !received.contains("response-private-sentinel") {
                    let chunk = response.chunk().await.unwrap().unwrap();
                    received.push_str(&String::from_utf8_lossy(&chunk));
                }
                drop(response);
            } else {
                assert_eq!(
                    response.status().as_u16(),
                    if mode == 795 {
                        422
                    } else if !streamed && mode != 790 {
                        502
                    } else {
                        200
                    }
                );
                let text = response.text().await.unwrap();
                if mode == 790 {
                    assert!(text.contains("refusal-private-sentinel"));
                    assert!(text.contains(if agent == "claude" {
                        "\"stop_reason\":\"refusal\""
                    } else {
                        "\"type\":\"refusal\""
                    }));
                } else if mode == 795 {
                    assert!(text.contains("对话角色或内容格式"));
                    assert!(!text.contains("upstream-secret-fixture"));
                }
            }
            tokio::time::timeout(Duration::from_secs(4), async {
                loop {
                    let available = state.observation.capacity.available_permits();
                    let new: Vec<_> = state
                        .observation
                        .hub_rows_for_test()
                        .into_iter()
                        .filter(|r| !before.contains(&r.id))
                        .collect();
                    if available == 64 && new.len() == 1 && new[0].status != "in_progress" {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            let row = state
                .observation
                .hub_rows_for_test()
                .into_iter()
                .find(|r| !before.contains(&r.id))
                .unwrap();
            // Reprice only this fixture clone using a trusted endpoint and fixed
            // tariff date, never by changing production observations.
            let mut billable = row.clone();
            billable.endpoint = "https://api.deepseek.com".into();
            billable.model = Some("deepseek-flash".into());
            billable.started_at = "2026-10-25T00:00:00Z".into();
            billable.ended_at = Some("2026-10-25T00:00:10Z".into());
            let cost = local_agent_core::provider_cost::for_provider(&billable);
            assert_eq!(
                row.status,
                match mode {
                    790 => "success",
                    792..=794 => "cancelled",
                    _ => "error",
                },
                "{agent} {mode}"
            );
            if [793, 794, 795].contains(&mode) {
                assert!(row.usage.output_tokens.is_none(), "{agent} {mode}");
                assert_eq!(cost.kind, "unknown");
                assert!(cost.amount.is_none());
            } else {
                assert_eq!(row.usage.input_tokens, Some(100), "{agent} {mode}");
                assert_eq!(row.usage.output_tokens, Some(7), "{agent} {mode}");
                assert_eq!(row.usage.cached_input_tokens, Some(80), "{agent} {mode}");
                assert_eq!(cost.kind, "estimated", "{agent} {mode}");
                assert_eq!(cost.currency, Some("CNY"));
                assert!((cost.amount.unwrap() - 0.0000496).abs() < 1e-12);
            }
            if (792..=794).contains(&mode) {
                assert_eq!(row.downstream_status.as_deref(), Some("cancelled"));
            }
        }
    }
    assert_eq!(state.observation.hub_rows_for_test().len() - before_all, 18);
}
