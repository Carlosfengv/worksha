use super::provider_cost::*;
use crate::{observation::ModelObservation, provider_usage, sessions::Usage};
use serde_json::json;

fn usage() -> Usage {
    Usage {
        input_tokens: Some(1_000_000),
        output_tokens: Some(100_000),
        cached_input_tokens: Some(800_000),
        reasoning_output_tokens: Some(50_000),
        total_tokens: Some(1_100_000),
        ..Default::default()
    }
}
fn estimate(model: &str, time: &str, u: &Usage) -> Cost {
    calculate(
        Some("https://api.deepseek.com"),
        Some(model),
        Some(time),
        "success",
        u,
        None,
        Some(time),
    )
}

#[test]
fn cache_is_subtracted_once_and_reasoning_is_already_in_output() {
    let alias = calculate(
        Some("https://api.deepseek.com/anthropic"),
        Some("deepseek-v4-pro[1M]"),
        Some("2026-10-19T02:00:00Z"),
        "success",
        &usage(),
        None,
        Some("2026-10-19T02:00:00Z"),
    );
    assert_eq!(alias.kind, "estimated");
    assert_eq!(alias.price.unwrap().model, "deepseek-v4-pro");
    let c = estimate("deepseek-flash", "2026-10-19T02:00:00Z", &usage());
    assert_eq!(c.kind, "estimated");
    assert!((c.amount.unwrap() - 0.1848).abs() < 1e-12);
    assert_eq!(c.components[0].tokens, 200_000);
    assert_eq!(c.components[1].amount, 0.0048);
    assert_eq!(c.price.unwrap().period, "peak");
    let c = estimate("deepseek-v4-pro", "2026-10-19T10:00:00Z", &usage());
    assert!((c.amount.unwrap() - 0.3476).abs() < 1e-12);
    assert_eq!(c.price.unwrap().period, "off_peak");
    assert_eq!(
        estimate("deepseek-flash", "2026-10-18T02:00:00Z", &usage()).amount,
        Some(0.0924)
    );
    // Offset timestamps use UTC and all published aliases use the Flash tariff.
    assert_eq!(
        estimate("deepseek-v4-flash", "2026-10-19T10:00:00+08:00", &usage()).amount,
        Some(0.1848)
    );
}

#[test]
fn missing_prices_usage_bad_subsets_and_time_boundaries_never_become_zero() {
    for (model, time, reason) in [
        (
            "deepseek-v4-pro[2M]",
            "2026-10-19T02:00:00Z",
            "missing_price",
        ),
        ("other", "2026-10-19T02:00:00Z", "missing_price"),
        ("deepseek-flash", "invalid", "unknown_time"),
        (
            "deepseek-flash",
            "2026-10-16T23:59:59Z",
            "price_outside_period",
        ),
        (
            "deepseek-flash",
            "2026-11-18T00:00:00Z",
            "price_outside_period",
        ),
    ] {
        let c = estimate(model, time, &usage());
        assert_eq!(c.reason, Some(reason));
        assert!(c.amount.is_none());
    }
    for (field, reason) in [
        ("input", "missing_usage"),
        ("output", "missing_usage"),
        ("cache", "missing_cache_usage"),
        ("large-cache", "inconsistent_usage"),
        ("write", "inconsistent_usage"),
        ("reasoning", "inconsistent_usage"),
        ("total", "inconsistent_usage"),
        ("overflow", "inconsistent_usage"),
    ] {
        let mut u = usage();
        match field {
            "input" => u.input_tokens = None,
            "output" => u.output_tokens = None,
            "cache" => u.cached_input_tokens = None,
            "large-cache" => u.cached_input_tokens = Some(1_000_001),
            "write" => u.cache_write_input_tokens = Some(10),
            "reasoning" => u.reasoning_output_tokens = Some(100_001),
            "total" => u.total_tokens = Some(1),
            _ => {
                u.input_tokens = Some(u64::MAX);
                u.total_tokens = None;
            }
        }
        assert_eq!(
            estimate("deepseek-flash", "2026-10-19T02:00:00Z", &u).reason,
            Some(reason)
        );
    }
    let at = "2026-10-19T03:59:59Z";
    assert_eq!(
        calculate(
            Some("https://api.deepseek.com"),
            Some("deepseek-flash"),
            Some(at),
            "success",
            &usage(),
            None,
            Some("2026-10-19T04:00:01Z")
        )
        .reason,
        Some("rate_boundary")
    );
    for endpoint in [
        "https://reseller.example/v1",
        "http://api.deepseek.com",
        "https://api.deepseek.com.evil.test",
        "https://api.deepseek.com:444",
        "https://api.deepseek.com/custom",
    ] {
        assert_eq!(
            calculate(
                Some(endpoint),
                Some("deepseek-flash"),
                Some(at),
                "success",
                &usage(),
                None,
                Some(at)
            )
            .reason,
            Some("missing_price")
        );
    }
}

#[test]
fn real_zero_actual_charges_and_unknown_are_distinct_and_exclusive() {
    let report = |amount| ReportedCost {
        amount,
        currency: "USD".into(),
        source: "openrouter.usage.cost".into(),
    };
    let actual = |r: &ReportedCost| {
        calculate(
            Some("https://openrouter.ai/api/v1"),
            None,
            None,
            "failed",
            &Usage::default(),
            Some(r),
            None,
        )
    };
    assert_eq!(actual(&report(0.0)).kind, "actual");
    assert_eq!(actual(&report(0.123)).amount, Some(0.123));
    assert_eq!(actual(&report(-1.0)).kind, "unknown");
    assert_eq!(actual(&report(f64::NAN)).kind, "unknown");
    assert_eq!(actual(&report(f64::INFINITY)).kind, "unknown");
    assert_eq!(
        calculate(
            Some("https://other.test"),
            None,
            None,
            "success",
            &usage(),
            Some(&report(1.0)),
            None
        )
        .kind,
        "unknown"
    );
    assert_eq!(
        calculate(
            Some("https://openrouter.ai/api/v1"),
            None,
            None,
            "in_progress",
            &usage(),
            Some(&report(0.0)),
            None
        )
        .kind,
        "unknown"
    );
    let mut totals = Totals::default();
    totals.add(&actual(&report(0.0)));
    totals.add(&actual(&report(0.123)));
    totals.add(&estimate(
        "deepseek-flash",
        "2026-10-19T02:00:00Z",
        &usage(),
    ));
    totals.add(&Cost::unknown("missing_usage"));
    assert_eq!(totals.actual[0].calls, 2);
    assert_eq!(totals.actual[0].amount, 0.123);
    assert_eq!(totals.estimated[0].calls, 1);
    assert_eq!(totals.unknown_calls, 1);
    let zero = Usage {
        input_tokens: Some(0),
        output_tokens: Some(0),
        ..Default::default()
    };
    assert_eq!(
        estimate("deepseek-flash", "2026-10-19T02:00:00Z", &zero).amount,
        Some(0.0)
    );
}

#[test]
fn old_observations_remain_readable_and_costs_are_not_limited_to_a_page() {
    let make = |n| {
        serde_json::from_value::<ModelObservation>(json!({"id":format!("r{n}"),"providerId":"p","agentId":"codex","provider":"renamed","endpoint":"https://api.deepseek.com","bindingRevision":1,"startedAt":"2026-10-19T02:00:00Z","endedAt":"2026-10-19T02:00:01Z","status":"success","protocol":"chat-completions","revision":1,"model":"deepseek-flash","usage":usage()})).unwrap()
    };
    let mut rows = (0..85).map(make).collect::<Vec<_>>();
    rows.push(rows[0].clone());
    let provider = serde_json::from_value(json!({"id":"p","name":"Renamed","endpoint":"https://reseller.example","protocol":"responses","model":"changed","configured":true,"revision":10,"category":"custom"})).unwrap();
    let snapshot = provider_usage::aggregate(
        provider_usage::Query {
            source: "gateway".into(),
            provider_id: Some("p".into()),
            ..Default::default()
        },
        &[provider],
        provider_usage::gateway_records(rows, &[]),
        "now".into(),
        "ready".into(),
        chrono::DateTime::parse_from_rfc3339("2026-10-19T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
    )
    .unwrap();
    assert_eq!(snapshot.calls.len(), 40);
    assert_eq!(snapshot.models[0].totals.cost.estimated[0].calls, 85);
    let selected = snapshot.selected.unwrap();
    assert_eq!(selected.cost.estimated[0].currency, "CNY");
    assert!((selected.cost.estimated[0].amount - 85.0 * 1.232).abs() < 1e-10);
    assert_eq!(snapshot.summary.cost.unknown_calls, 0);
    // Editing today's Provider does not reprice old requests using that endpoint.
    assert!(snapshot.calls.iter().all(|r| r.cost.kind == "estimated"));
}

fn provider_row() -> ModelObservation {
    serde_json::from_value(json!({
        "id":"cny-call", "providerId":"p", "agentId":"opencode",
        "provider":"saved", "endpoint":"https://api.deepseek.com", "bindingRevision":1,
        "startedAt":"2026-10-19T02:00:00Z", "endedAt":"2026-10-19T02:00:01Z",
        "status":"success", "protocol":"chat-completions", "revision":1,
        "model":"deepseek-flash", "usage":usage()
    }))
    .unwrap()
}

#[test]
fn provider_cny_uses_independent_official_rates_and_keeps_other_consumers_usd() {
    for (model, at, expected, rates) in [
        (
            "deepseek-flash",
            "2026-10-19T02:00:00Z",
            1.232,
            [2.0, 0.04, 8.0],
        ),
        (
            "deepseek-flash",
            "2026-10-18T02:00:00Z",
            0.616,
            [1.0, 0.02, 4.0],
        ),
        (
            "deepseek-v4-pro",
            "2026-10-19T02:00:00Z",
            4.74,
            [9.0, 0.3, 27.0],
        ),
        (
            "deepseek-v4-pro",
            "2026-10-19T10:00:00Z",
            2.37,
            [4.5, 0.15, 13.5],
        ),
    ] {
        let mut row = provider_row();
        row.model = Some(model.into());
        row.started_at = at.into();
        row.ended_at = Some(at.into());
        let before = serde_json::to_value(&row).unwrap();
        let c = for_provider(&row);
        assert_eq!(c.kind, "estimated");
        assert_eq!(c.currency, Some("CNY"));
        assert!((c.amount.unwrap() - expected).abs() < 1e-12);
        assert!((c.components.iter().map(|p| p.amount).sum::<f64>() - expected).abs() < 1e-12);
        assert_eq!(
            c.components
                .iter()
                .map(|p| p.per_million)
                .collect::<Vec<_>>(),
            rates
        );
        let p = c.price.unwrap();
        assert_eq!(p.version, "deepseek-cny-2026-10-25");
        assert_eq!(p.checked_at, "2026-10-25");
        assert_eq!(
            [
                p.input_per_million,
                p.cached_input_per_million,
                p.output_per_million
            ],
            rates
        );
        assert_eq!(
            c.source,
            Some("https://api-docs.deepseek.com/zh-cn/quick_start/pricing/")
        );
        assert_eq!(serde_json::to_value(&row).unwrap(), before);
        assert_eq!(estimate(model, at, &row.usage).currency, Some("USD"));
    }
    let mut row = provider_row();
    row.endpoint = "https://api.deepseek.com/anthropic".into();
    row.model = Some("deepseek-v4-pro[1M]".into());
    assert_eq!(for_provider(&row).price.unwrap().model, "deepseek-v4-pro");
    for endpoint in [
        "https://api.deepseek.com.evil.test",
        "https://reseller.invalid/v1",
    ] {
        row.endpoint = endpoint.into();
        assert_eq!(for_provider(&row).reason, Some("missing_price"));
    }
    row.endpoint = "https://openrouter.ai/api/v1".into();
    row.reported_cost = Some(ReportedCost {
        amount: 0.123,
        currency: "USD".into(),
        source: "openrouter.usage.cost".into(),
    });
    let cost = for_provider(&row);
    assert_eq!(cost.kind, "actual");
    assert_eq!(cost.currency, Some("USD"));
    assert_eq!(cost.amount, Some(0.123));
}

#[test]
fn provider_cny_preserves_missing_usage_with_recorded_outcome_reasons() {
    let mut rows = vec![];
    for (status, http, issue, reason, count) in [
        (
            "error",
            400,
            "上游拒绝跨协议请求",
            "missing_usage_upstream_rejected",
            7,
        ),
        (
            "error",
            422,
            "上游拒绝跨协议请求",
            "missing_usage_upstream_rejected",
            6,
        ),
        (
            "error",
            200,
            "上游返回当前不可转换的思考或拒绝内容，请使用同协议连接",
            "missing_usage_unsupported_content",
            2,
        ),
        (
            "cancelled",
            200,
            "客户端结束读取，未收到完整结束事件",
            "missing_usage_client_cancelled",
            3,
        ),
    ] {
        let mut row = provider_row();
        row.usage = Usage::default();
        row.status = status.into();
        row.http_status = Some(http);
        row.timing_issue = Some(issue.into());
        let c = for_provider(&row);
        assert_eq!(c.kind, "unknown");
        assert_eq!(c.reason, Some(reason));
        assert!(c.amount.is_none() && c.currency.is_none() && c.price.is_none());
        for i in 0..count {
            let mut r = row.clone();
            r.id = format!("{http}-{status}-{i}");
            rows.push(r);
        }
        row.timing_issue = Some("unrecognized upstream text".into());
        assert_eq!(for_provider(&row).reason, Some("missing_usage"));
    }
    let records = provider_usage::gateway_records(rows, &[]);
    let mut totals = provider_usage::Totals::default();
    for r in &records {
        totals.add(r);
    }
    assert_eq!(totals.cost.unknown_calls, 18);
    assert_eq!(
        totals
            .unknown_cost_reasons
            .get("missing_usage_upstream_rejected"),
        Some(&13)
    );
    assert_eq!(
        totals
            .unknown_cost_reasons
            .get("missing_usage_unsupported_content"),
        Some(&2)
    );
    assert_eq!(
        totals
            .unknown_cost_reasons
            .get("missing_usage_client_cancelled"),
        Some(&3)
    );
}

#[test]
fn provider_cny_does_not_fill_missing_fields_or_cross_tariff_boundaries() {
    let mut row = provider_row();
    row.usage.cached_input_tokens = None;
    assert_eq!(for_provider(&row).reason, Some("missing_cache_usage"));
    row = provider_row();
    row.started_at = "2026-10-16T23:59:59Z".into();
    assert_eq!(for_provider(&row).reason, Some("price_outside_period"));
    row = provider_row();
    row.started_at = "2026-10-19T03:59:59Z".into();
    row.ended_at = Some("2026-10-19T04:00:01Z".into());
    assert_eq!(for_provider(&row).reason, Some("rate_boundary"));
    row = provider_row();
    row.usage.input_tokens = Some(0);
    row.usage.cached_input_tokens = Some(0);
    row.usage.output_tokens = Some(0);
    row.usage.reasoning_output_tokens = Some(0);
    row.usage.total_tokens = Some(0);
    assert_eq!(for_provider(&row).amount, Some(0.0));
}
