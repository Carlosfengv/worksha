use crate::{
    model_services::{Capabilities, ServiceSelection},
    observation::{ModelObservation, RouteEvidence},
    requests::Request,
};
use std::collections::HashMap;
pub(super) const STORE: &str = "00000000-0000-4000-8000-000000000001";
pub(super) fn directory() -> &'static crate::agents::AgentDirectory {
    crate::agents::builtin_directory()
}
pub(super) fn canonical(rows: Vec<ModelObservation>) -> Request {
    crate::requests::logical_requests(rows, &HashMap::new()).remove(0)
}
pub(super) fn retry() -> (Request, Vec<ModelObservation>) {
    let mut rows = vec![];
    for index in 0..4 {
        let at = "2026-10-27T00:00:01Z"
            .parse::<chrono::DateTime<chrono::Utc>>()
            .unwrap()
            + chrono::Duration::seconds(index);
        let mut row = crate::observation::tests::request(at);
        row.id = format!("private-attempt-{index}");
        row.logical_id = Some("model-service-private-logical".into());
        row.session_id = Some("private-session".into());
        row.provider_id = Some(
            if index % 2 == 0 {
                "provider-a"
            } else {
                "provider-b"
            }
            .into(),
        );
        row.provider = format!("private-provider-label-{}", index % 2);
        row.endpoint = if index % 2 == 0 {
            "https://api.deepseek.com"
        } else {
            "https://openrouter.ai/api/v1"
        }
        .into();
        row.model = Some(
            if index % 2 == 0 {
                "deepseek-v4-flash"
            } else {
                "fixture-model"
            }
            .into(),
        );
        row.routed_model = Some(
            if index % 2 == 0 {
                "routed-a"
            } else {
                "routed-b"
            }
            .into(),
        );
        row.status = if index == 3 { "success" } else { "error" }.into();
        row.clock_id = Some("private-clock".into());
        row.start_tick_ms = Some(index as f64 * 2000.0);
        row.binding_revision = 1 + index as u64 % 2;
        row.usage.cached_input_tokens = Some(index as u64);
        if index == 1 {
            row.reported_cost = Some(crate::provider_cost::ReportedCost {
                amount: 0.0,
                currency: "USD".into(),
                source: "openrouter.usage.cost".into(),
            });
        }
        if index == 2 {
            row.usage.total_tokens = None;
        }
        row.routing = Some(RouteEvidence {
            trace: None,
            binding_id: "private-binding".into(),
            route_version_id: format!("private-route-{index}"),
            provider_revision: 1,
            boundary: "verified".into(),
            service: Some(ServiceSelection {
                service_id: "private-service-id".into(),
                service_version_id: "private-version".into(),
                service_name: "private-service-name".into(),
                model_id: "private-model-id".into(),
                model_name: "private-model-name".into(),
                requested_alias: "private-alias".into(),
                preferred_model_id: None,
                preferred_model_name: None,
                attempt: index as u8,
                fallback_reason: None,
                upstream_account: None,
                upstream_account_label: None,
                account_selection_reason: None,
                routing_rule: None,
                routing_intent: None,
                pending_child: None,
                nested_route: vec![],
                nested_scope: None,
                budget: None,
                mode: "auto".into(),
                capabilities: Capabilities::default(),
            }),
        });
        rows.push(row);
    }
    let mut request = canonical(rows.clone());
    assert_eq!(request.attempts.len(), 4);
    assert_eq!(request.status, "recovered");
    request.project_id = Some("private-project-id".into());
    request.project_name = Some("private-project-path".into());
    request.session_title = Some("private-session-title".into());
    (request, rows)
}
