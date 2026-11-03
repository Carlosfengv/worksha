use super::*;
use crate::observation::retention::test_support::{STORE, directory, retry};

#[test]
fn full_projection_keeps_separate_attempts_and_request_parts_without_private_identity() {
    let (request, rows) = retry();
    let fact =
        GatewayFact::from_request_with_evidence(&request, &rows, STORE, directory(), &[]).unwrap();
    let value = serde_json::to_value(&fact).unwrap();
    assert_eq!(value["version"], 2);
    assert!(!value.to_string().contains("private-"));
    let attribution = fact.attribution.as_ref().unwrap();
    assert!(attribution.valid(4));
    assert_eq!(attribution.attempts.len(), 4);
    assert_eq!(attribution.session.as_ref().unwrap().len(), 64);
    assert_eq!(attribution.project.as_ref().unwrap().len(), 64);
    assert_ne!(attribution.session, attribution.project);
    assert_eq!(attribution.parts.len(), 4);
    assert_eq!(fact.statistics().requests, 1);
    assert_eq!(fact.statistics().attempts, 4);
    for (attempt, row) in attribution.attempts.iter().zip(rows) {
        assert_eq!(
            attempt.provider_group,
            key(STORE, "provider", row.provider_id.as_ref().unwrap())
        );
        assert!(attempt.attributed_provider.is_some());
        let recorded = provider_usage::gateway_records(vec![row.clone()], &[]).remove(0);
        let mut expected = provider_usage::Totals::default();
        expected.add(&recorded);
        expected.finish();
        assert_eq!(
            serde_json::to_value(&attempt.provider_stats).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
        let cost = provider_cost::calculate(
            Some(&row.endpoint),
            row.model.as_deref(),
            Some(&row.started_at),
            &row.status,
            &row.usage,
            row.reported_cost.as_ref(),
            row.ended_at.as_deref(),
        );
        let mut model = crate::model_usage::UsageTotals::default();
        model.add(&row.usage, &cost);
        assert_eq!(
            serde_json::to_value(&attempt.model_stats).unwrap(),
            serde_json::to_value(model).unwrap()
        );
    }
}

#[test]
fn model_history_and_provider_channels_keep_their_own_cost_currencies() {
    let (request, rows) = retry();
    let fact =
        GatewayFact::from_request_with_evidence(&request, &rows, STORE, directory(), &[]).unwrap();
    let attempt = &fact.attribution.as_ref().unwrap().attempts[0];
    assert_eq!(attempt.request_cost.estimated[0].currency, "USD");
    assert_eq!(attempt.provider_stats.cost.estimated[0].currency, "CNY");
    assert_eq!(
        serde_json::to_value(&attempt.model_stats).unwrap()["cost"]["estimated"][0]["currency"],
        "USD"
    );
}

#[test]
fn context_links_are_scoped_pseudonyms_and_do_not_depend_on_display_names() {
    let (mut request, rows) = retry();
    let a =
        GatewayFact::from_request_with_evidence(&request, &rows, STORE, directory(), &[]).unwrap();
    request.project_name = Some("another-private-name".into());
    request.session_title = Some("another-private-title".into());
    let b =
        GatewayFact::from_request_with_evidence(&request, &rows, STORE, directory(), &[]).unwrap();
    assert_eq!(
        a.attribution.as_ref().unwrap().project,
        b.attribution.as_ref().unwrap().project
    );
    assert_eq!(
        a.attribution.as_ref().unwrap().session,
        b.attribution.as_ref().unwrap().session
    );
    let other = GatewayFact::from_request_with_evidence(
        &request,
        &rows,
        "00000000-0000-4000-8000-000000000002",
        directory(),
        &[],
    )
    .unwrap();
    assert_ne!(
        a.attribution.unwrap().project,
        other.attribution.unwrap().project
    );
}

#[test]
fn unreadable_model_labels_are_not_copied_or_merged_into_unknown_models() {
    let (_, mut rows) = retry();
    rows[0].routed_model = Some("private-model\ncontrol".into());
    rows[1].routed_model = None;
    rows[1].model = None;
    let request = crate::observation::retention::test_support::canonical(rows.clone());
    let f =
        GatewayFact::from_request_with_evidence(&request, &rows, STORE, directory(), &[]).unwrap();
    let a = f.attribution.unwrap();
    assert!(a.attempts[0].routed_model.label.is_none());
    assert_ne!(
        a.attempts[0].routed_model.key,
        a.attempts[1].routed_model.key
    );
    assert!(
        !serde_json::to_value(&a)
            .unwrap()
            .to_string()
            .contains("private-model")
    );
}

#[test]
fn duplicate_attempt_identity_is_rejected_instead_of_double_counted() {
    let (mut request, rows) = retry();
    request.attempts.push(request.attempts[0].clone());
    assert_eq!(
        GatewayFact::from_request_with_evidence(&request, &rows, STORE, directory(), &[])
            .err()
            .unwrap(),
        FactError::InvalidRequest
    );
}

#[test]
fn recorded_model_provider_identity_preserves_the_existing_format() {
    for (name, endpoint, revision, expected) in [
        (None, None, None, "fe44a8cccd89edae70b54c5e58399979"),
        (
            Some("name"),
            Some("https://example.test/v1"),
            Some(7),
            "6c98e52137b6d58ad8a24a7cb74e6f48",
        ),
        (
            Some("产品A"),
            Some("https://host.test:443/v1"),
            Some(0),
            "8f5a7626a69642acd7d2b4dbd2380cd8",
        ),
    ] {
        assert_eq!(
            crate::model_usage::provider_identity(name, endpoint, revision),
            expected
        );
    }
}
