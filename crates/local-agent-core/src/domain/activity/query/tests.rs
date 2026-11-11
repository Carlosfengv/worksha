use super::*;
fn row(id: &str, kind: ActivityKind, at: Option<String>) -> ActivityRecord {
    ActivityRecord {
        id: id.into(),
        kind,
        source_id: "source".into(),
        provider_key: "fixture".into(),
        source_revision: "1".into(),
        observed_at: "2026-10-31T10:00:00Z".into(),
        occurred_at: at,
        session_id: None,
        message_id: None,
        message_started_at: None,
        product_ids: vec![],
        unknown_client: true,
        title: "metadata".into(),
        model: None,
        phase: "unknown".into(),
        state: ActivityState::Current,
        tool_names: vec![],
        logical_request_id: None,
        historical_duration_ms: None,
        measurements: None,
        reported_measurements: None,
    }
}
fn status(kind: ActivityKind, coverage: Coverage) -> ActivitySourceState {
    ActivitySourceState {
        kind,
        state: SourceState {
            source_id: "source".into(),
            authorization: Authorization::Authorized,
            freshness: Freshness::Current,
            coverage,
            problem: None,
        },
    }
}
#[test]
fn activity_window_is_bounded_and_keeps_full_counts_and_unknown_time_separate() {
    let mut window = ActivityWindow::default();
    for n in 0..300 {
        window.observe(row(
            &format!("known-{n}"),
            ActivityKind::NativeMessage,
            Some(
                chrono::DateTime::from_timestamp_millis(1793440800000 + n)
                    .unwrap()
                    .to_rfc3339(),
            ),
        ));
    }
    for n in 0..20 {
        let mut value = row(&format!("unknown-{n}"), ActivityKind::NativeMessage, None);
        value.state = ActivityState::Historical;
        window.observe(value);
    }
    let (items, summary, limited) =
        window.finish(&[status(ActivityKind::NativeMessage, Coverage::Partial)]);
    assert!(limited);
    assert_eq!(items.len(), 256);
    assert_eq!(items[0].id, "known-299");
    assert_eq!(items[255].id, "known-44");
    assert!(items.iter().all(|r| r.occurred_at.is_some()));
    assert_eq!(summary[0].observed_records.as_ref().unwrap().value(), 320);
    assert_eq!(summary[0].current.as_ref().unwrap().value(), 300);
    assert_eq!(summary[0].historical.as_ref().unwrap().value(), 20);
    assert_eq!(summary[0].unknown_time.as_ref().unwrap().value(), 20);
}
#[test]
fn attempts_messages_and_source_events_have_independent_units_and_unknown_is_not_zero() {
    let mut window = ActivityWindow::default();
    for id in ["one", "two"] {
        let mut r = row(id, ActivityKind::GatewayAttempt, None);
        r.logical_request_id = Some("same-logical-request".into());
        window.observe(r);
    }
    window.observe(row("one", ActivityKind::NativeMessage, None));
    let (items, summary, limited) = window.finish(&[
        status(ActivityKind::GatewayAttempt, Coverage::Partial),
        status(ActivityKind::NativeMessage, Coverage::Partial),
        status(ActivityKind::SourceEvent, Coverage::Unavailable),
    ]);
    assert!(!limited);
    assert_eq!(items.len(), 3);
    assert_eq!(summary[0].observed_records.as_ref().unwrap().value(), 2);
    assert_eq!(summary[0].logical_requests.as_ref().unwrap().value(), 1);
    assert!(summary[1].logical_requests.is_none());
    assert!(summary[2].observed_records.is_none());
    let (_, summary, _) =
        ActivityWindow::default().finish(&[status(ActivityKind::NativeMessage, Coverage::Partial)]);
    assert_eq!(summary[0].observed_records.as_ref().unwrap().value(), 0);
}
#[test]
fn activity_query_rejects_arbitrary_methods_unbounded_ranges_and_invalid_scopes() {
    let good = UnifiedActivityQuery {
        scope: QueryScope {
            kind: ScopeKind::Builtin,
            source_ids: vec!["claude".into()],
        },
        range_seconds: 10800,
        product_id: None,
        session_id: None,
        include_gateway: true,
        include_historical: true,
    };
    good.validate().unwrap();
    for mutation in 0..5 {
        let mut q = good.clone();
        match mutation {
            0 => q.range_seconds = 0,
            1 => q.range_seconds = 86401,
            2 => q.scope.source_ids.clear(),
            3 => q.scope.source_ids.push("claude".into()),
            _ => q.scope.kind = ScopeKind::External,
        };
        assert!(q.validate().is_err());
    }
    let mut encoded = serde_json::to_value(&good).unwrap();
    encoded["method"] = serde_json::json!("scan");
    assert!(serde_json::from_value::<UnifiedActivityQuery>(encoded).is_err());
    let now = 1793440800000;
    let unknown = row("unknown", ActivityKind::SourceEvent, None);
    assert!(good.matches(&unknown, now));
    let future = row(
        "future",
        ActivityKind::SourceEvent,
        Some(
            chrono::DateTime::from_timestamp_millis(now + 1001)
                .unwrap()
                .to_rfc3339(),
        ),
    );
    assert!(!good.matches(&future, now));
}
