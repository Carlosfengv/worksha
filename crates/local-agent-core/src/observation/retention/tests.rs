use super::*;
use crate::observation::{GatewayClient, ModelObservation};
use serde_json::Value;
use std::collections::HashMap;

const NAMESPACE: &str = "00000000-0000-4000-8000-000000000001";
fn now() -> DateTime<Utc> {
    "2026-10-27T08:00:00Z".parse().unwrap()
}
fn request(rows: Vec<ModelObservation>) -> Request {
    crate::requests::logical_requests(rows, &HashMap::new()).remove(0)
}
fn fact(request: &Request) -> GatewayFact {
    GatewayFact::from_request(request, NAMESPACE, crate::agents::builtin_directory()).unwrap()
}
fn value(fact: &GatewayFact) -> Value {
    serde_json::to_value(fact).unwrap()
}

#[test]
fn privacy_projection_never_copies_identifiers_labels_endpoints_or_arbitrary_notes() {
    let mut row = super::super::tests::request(now() - Duration::days(10));
    row.id = "private-attempt-id".into();
    row.logical_id = Some("private-logical-id".into());
    row.session_id = Some("private-session-id".into());
    row.provider = "private-provider-label".into();
    row.provider_id = Some("private-provider-id".into());
    row.endpoint = "https://private-host.test/private/path?key=private-query-secret".into();
    row.upstream_request_id = Some("private-upstream-id".into());
    row.response_id = Some("private-response-id".into());
    row.stop_reason = Some("private-stop-note".into());
    row.timing_issue = Some("private-timing-note".into());
    row.client = Some(GatewayClient {
        id: "private-client-id".into(),
        name: "private-client-name".into(),
        peer: "private-peer-address".into(),
    });
    let mut request = request(vec![row]);
    request.session_title = Some("private-session-title".into());
    request.project_id = Some("private-project-id".into());
    request.project_name = Some("private-project-path".into());
    let projected = value(&fact(&request));
    let text = projected.to_string();
    assert!(!text.contains("private-"), "{text}");
    assert_eq!(
        projected["stats"],
        serde_json::to_value(Summary::of([&request])).unwrap()
    );
    assert_eq!(projected["model"], "model");
    assert_eq!(projected.as_object().unwrap().keys().count(), 12);
}

#[test]
fn keys_are_stable_per_namespace_and_do_not_claim_to_be_source_ids() {
    let request = request(vec![super::super::tests::request(now())]);
    let a = fact(&request);
    let b = fact(&request);
    assert_eq!(value(&a), value(&b));
    assert_eq!(value(&a)["key"].as_str().unwrap().len(), 64);
    let other = GatewayFact::from_request(
        &request,
        "00000000-0000-4000-8000-000000000002",
        crate::agents::builtin_directory(),
    )
    .unwrap();
    assert_ne!(value(&a)["key"], value(&other)["key"]);
    assert_ne!(value(&a)["provider"], value(&other)["provider"]);
    assert_eq!(value(&a)["stats"], value(&other)["stats"]);
    for namespace in ["", "not-a-uuid", "00000000-0000-1000-8000-000000000001"] {
        assert_eq!(
            GatewayFact::from_request(&request, namespace, crate::agents::builtin_directory())
                .err()
                .unwrap(),
            FactError::InvalidNamespace
        );
    }
}

#[test]
fn every_retention_window_preserves_the_exact_nanosecond_boundary() {
    for days in [7, 30, 90] {
        let cutoff = now() - Duration::days(days);
        for (offset, expected) in [
            (-1, ExpirationCandidate::Expired),
            (0, ExpirationCandidate::WithinWindow),
            (1, ExpirationCandidate::WithinWindow),
        ] {
            let row = super::super::tests::request(cutoff + Duration::nanoseconds(offset));
            let f = fact(&request(vec![row]));
            assert_eq!(
                f.expiration_candidate(days as u16, now()).unwrap(),
                expected
            );
        }
    }
    let f = fact(&request(vec![super::super::tests::request(now())]));
    for days in [0, 1, 29, 31, 365, u16::MAX] {
        assert_eq!(
            f.expiration_candidate(days, now()),
            Err(FactError::InvalidRetentionWindow)
        );
    }
    assert_eq!(
        f.expiration_candidate(90, DateTime::<Utc>::MIN_UTC),
        Err(FactError::InvalidRetentionWindow)
    );
}

#[test]
fn all_attempts_must_be_finished_before_the_cutoff_even_if_the_last_request_end_is_old() {
    let mut a = super::super::tests::request(now() - Duration::days(10));
    a.logical_id = Some("retry".into());
    a.status = "error".into();
    a.ended_at = Some(now().to_rfc3339());
    let mut b = a.clone();
    b.id = "r2".into();
    b.status = "success".into();
    b.started_at = (now() - Duration::days(9)).to_rfc3339();
    b.ended_at = Some((now() - Duration::days(9) + Duration::seconds(2)).to_rfc3339());
    let request = request(vec![a, b]);
    assert_eq!(request.status, "recovered");
    assert_eq!(
        fact(&request).expiration_candidate(7, now()).unwrap(),
        ExpirationCandidate::WithinWindow
    );
}

#[test]
fn active_unknown_or_invalid_time_is_protected_without_losing_usage_facts() {
    for change in [
        "active",
        "unknown",
        "missing-end",
        "invalid-start",
        "backwards",
    ] {
        let mut row = super::super::tests::request(now() - Duration::days(100));
        let reason = match change {
            "active" => {
                row.status = "in_progress".into();
                Protection::Active
            }
            "unknown" => {
                row.status = "future-state".into();
                Protection::UnknownOutcome
            }
            "missing-end" => {
                row.ended_at = None;
                Protection::UnknownTime
            }
            "invalid-start" => {
                row.started_at = "private-invalid-time".into();
                Protection::UnknownTime
            }
            "backwards" => {
                row.ended_at = Some((now() - Duration::days(101)).to_rfc3339());
                Protection::UnknownTime
            }
            _ => unreachable!(),
        };
        let request = request(vec![row]);
        let f = fact(&request);
        assert_eq!(
            f.expiration_candidate(90, now()).unwrap(),
            ExpirationCandidate::Protected(reason)
        );
        assert_eq!(f.statistics().usage.total_tokens, Some(20));
        assert!(!value(&f).to_string().contains("private-invalid-time"));
        assert!(value(&f)["completedAt"].is_null());
    }
}

#[test]
fn cache_denominators_preserve_big_integers_and_exclude_invalid_pairs() {
    let mut a = super::super::tests::request(now());
    a.logical_id = Some("retry".into());
    a.usage.input_tokens = Some(u64::MAX);
    a.usage.cached_input_tokens = Some(u64::MAX - 1);
    let mut b = a.clone();
    b.id = "r2".into();
    let mut c = a.clone();
    c.id = "r3".into();
    c.usage.cached_input_tokens = None;
    let mut d = a.clone();
    d.id = "r4".into();
    d.usage.input_tokens = Some(1);
    d.usage.cached_input_tokens = Some(2);
    let f = fact(&request(vec![a, b, c, d]));
    assert_eq!(f.cache_counters().0, (u128::from(u64::MAX) * 2).to_string());
    assert_eq!(
        f.cache_counters().1,
        ((u128::from(u64::MAX) - 1) * 2).to_string()
    );
    assert_eq!(f.statistics().cache_samples, 2);
}

#[test]
fn known_zero_and_missing_usage_are_different_and_pseudonyms_do_not_change_counts() {
    let mut a = super::super::tests::request(now());
    a.usage.total_tokens = Some(0);
    let zero = fact(&request(vec![a.clone()]));
    a.usage.total_tokens = None;
    let missing = fact(&request(vec![a]));
    assert_eq!(zero.statistics().usage.total_tokens, Some(0));
    assert_eq!(zero.statistics().usage_missing, 0);
    assert_eq!(missing.statistics().usage.total_tokens, None);
    assert_eq!(missing.statistics().usage_missing, 1);
    assert_eq!(value(&zero)["key"], value(&missing)["key"]);
}

#[test]
fn normalization_does_not_change_nanoseconds_and_invalid_dimensions_do_not_copy_controls() {
    let mut row = super::super::tests::request(now() - Duration::days(10));
    row.started_at = "2026-10-17T08:00:00.123456789+08:00".into();
    row.ended_at = Some("2026-10-17T08:00:01.123456790+08:00".into());
    row.model = Some("private-model\ncontrol".into());
    let f = fact(&request(vec![row]));
    assert_eq!(value(&f)["startedAt"], "2026-10-17T00:00:00.123456789Z");
    assert_eq!(value(&f)["completedAt"], "2026-10-17T00:00:01.123456790Z");
    assert!(value(&f)["model"].is_null());
}

#[test]
fn native_and_empty_requests_cannot_be_misclassified_as_owned_gateway_facts() {
    let mut request = request(vec![super::super::tests::request(now())]);
    request.source = "native".into();
    assert_eq!(
        GatewayFact::from_request(&request, NAMESPACE, crate::agents::builtin_directory())
            .err()
            .unwrap(),
        FactError::NotGatewayRequest
    );
    request.source = "gateway".into();
    request.attempts.clear();
    assert_eq!(
        GatewayFact::from_request(&request, NAMESPACE, crate::agents::builtin_directory())
            .err()
            .unwrap(),
        FactError::InvalidRequest
    );
}
