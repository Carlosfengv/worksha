use super::*;
use local_agent_core::observation::{AnalysisCost, analyze};
use local_agent_core::observation::{AnalysisRow, timing_stats};
use serde_json::json;

#[test]
fn sql_overview_preserves_canonical_buckets_cost_missing_and_retry_timing() {
    let now = chrono::DateTime::parse_from_rfc3339("2026-10-20T03:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let dir = tempfile::tempdir().unwrap();
    let store = local_agent_core::observation::ObservationStore::open(dir.path()).unwrap();
    let mut rows: Vec<_> = (0..120)
        .map(|i| {
            let mut r = row(
                &format!("row-{i:02}"),
                "https://api.deepseek.com",
                "deepseek-flash",
            );
            r.duration_ms = Some(1000.0);
            r.ttft_ms = Some(i as f64);
            if i >= 15 {
                r.started_at = (now - chrono::Duration::minutes(i * 40)).to_rfc3339();
                r.model = (i % 11 != 0).then(|| format!("model-{}", i % 37));
                if i % 9 == 0 {
                    r.usage = Default::default();
                }
                if i % 7 == 0 {
                    r.status = "interrupted".into();
                }
            }
            r
        })
        .collect();
    rows[0].logical_id = Some("retry".into());
    rows[1].logical_id = Some("retry".into());
    rows[1].model = Some("different".into());
    rows[2].started_at = "invalid".into();
    rows[3].usage = Default::default();
    rows[4].status = "interrupted".into();
    rows[5].agent_product_id = None;
    store.save_batch(&rows).unwrap();
    let mut sql = local_agent_core::observation::sql::History::open(dir.path()).unwrap();
    sql.refresh(&[]).unwrap();
    rows.sort_by(|a, b| b.started_at.cmp(&a.started_at).then(b.id.cmp(&a.id)));
    sql.set_native_analysis(1, &gateway_rows(&rows)).unwrap();
    for range in [6, 12, 24, 168] {
        for source in ["gateway", "native"] {
            for agent in [None, Some("opencode"), Some("unknown")] {
                for dimension in ["agent", "model"] {
                    for metric in ["calls", "tokens", "cost"] {
                        let q = OverviewQuery {
                            agent: agent.map(str::to_owned),
                            range,
                            metric: metric.into(),
                            dimension: dimension.into(),
                            source: source.into(),
                        };
                        let expected = serde_json::to_value(
                            analyze(q.clone(), &gateway_rows(&rows), now).unwrap(),
                        )
                        .unwrap();
                        let got = sql.analysis(&q, now).unwrap();
                        fn check(a: &serde_json::Value, b: &serde_json::Value) {
                            match (a, b) {
                                (serde_json::Value::Number(a), serde_json::Value::Number(b)) => {
                                    assert!(
                                        (a.as_f64().unwrap() - b.as_f64().unwrap()).abs() < 1e-8
                                    )
                                }
                                (serde_json::Value::Array(a), serde_json::Value::Array(b)) => {
                                    assert_eq!(a.len(), b.len());
                                    for (a, b) in a.iter().zip(b) {
                                        check(a, b)
                                    }
                                }
                                (serde_json::Value::Object(a), serde_json::Value::Object(b)) => {
                                    assert_eq!(a.len(), b.len());
                                    for (k, v) in a {
                                        check(v, &b[k])
                                    }
                                }
                                _ => assert_eq!(a, b),
                            }
                        }
                        check(&got, &expected);
                        if source == "gateway" {
                            assert_eq!(
                                sql.timing_stats(&q, now).unwrap(),
                                serde_json::to_value(timing_stats(&rows, &q, now)).unwrap()
                            );
                        }
                    }
                }
            }
        }
    }
    sql.set_native_analysis(2, &[]).unwrap();
    let q = OverviewQuery {
        agent: None,
        range: 168,
        metric: "tokens".into(),
        dimension: "model".into(),
        source: "native".into(),
    };
    assert_eq!(
        sql.analysis(&q, now).unwrap(),
        serde_json::to_value(analyze(q, &[], now).unwrap()).unwrap()
    );
}

fn row(id: &str, endpoint: &str, model: &str) -> ModelObservation {
    serde_json::from_value(json!({
        "id":id,"agentId":"opencode","agentProductId":"opencode",
        "provider":"saved","providerId":"saved","endpoint":endpoint,
        "model":model,"bindingRevision":1,"startedAt":"2026-10-20T02:00:00Z",
        "endedAt":"2026-10-20T02:00:01Z","status":"success","revision":1,
        "protocol":"chat-completions","usage":{"inputTokens":100,"outputTokens":10,
            "totalTokens":110,"cachedInputTokens":20}
    }))
    .unwrap()
}

#[test]
fn overview_keeps_usd_while_provider_uses_cny_and_both_preserve_retry_unknowns() {
    let estimate = row("estimate", "https://api.deepseek.com", "deepseek-flash");
    let mut actual = row("actual", "https://openrouter.ai/api/v1", "vendor/model");
    actual.reported_cost = Some(local_agent_core::provider_cost::ReportedCost {
        amount: 0.123456789,
        currency: "USD".into(),
        source: "openrouter.usage.cost".into(),
    });
    actual.logical_id = Some("retry".into());
    let mut unknown = actual.clone();
    unknown.id = "unknown".into();
    unknown.reported_cost = None;
    unknown.status = "interrupted".into();
    let mut zero = actual.clone();
    zero.id = "zero".into();
    zero.logical_id = None;
    zero.reported_cost.as_mut().unwrap().amount = 0.0;
    let mut tiny = zero.clone();
    tiny.id = "tiny".into();
    tiny.reported_cost.as_mut().unwrap().amount = 0.000000001;
    let rows = vec![
        estimate.clone(),
        actual.clone(),
        unknown,
        zero,
        tiny,
        estimate,
    ];
    let grouped = gateway_rows(&rows);
    assert_eq!(grouped.len(), 4, "retry once, stable ID once");
    let query = OverviewQuery {
        agent: Some("opencode".into()),
        range: 24,
        metric: "cost".into(),
        dimension: "model".into(),
        source: "gateway".into(),
    };
    let now = chrono::DateTime::parse_from_rfc3339("2026-10-20T03:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let result = analyze(query.clone(), &grouped, now).unwrap();
    let provider_rows =
        local_agent_core::provider_usage::gateway_records(rows.into_iter().take(5).collect(), &[]);
    let cny: f64 = provider_rows
        .iter()
        .filter(|r| r.cost.currency == Some("CNY"))
        .filter_map(|r| r.cost.amount)
        .sum();
    assert!((cny - 0.0002408).abs() < 1e-12);
    let usd: f64 = provider_rows
        .iter()
        .filter(|r| r.cost.currency == Some("USD"))
        .filter_map(|r| r.cost.amount)
        .sum();
    assert!((usd - 0.123456790).abs() < 1e-12);
    // Independently published USD input/cache/output rates, never CNY conversion.
    let expected = usd + (80.0 * 0.30 + 20.0 * 0.006 + 10.0 * 1.20) / 1e6;
    assert!((result.total.unwrap() / 1e6 - expected).abs() < 1e-12);
    assert_eq!(result.cost.unknown_attempts, 1);
    assert_eq!(result.missing, 1);
    assert!((result.cost.actual_usd.unwrap() - 0.123456790).abs() < 1e-12);
    assert!((result.cost.estimated_usd.unwrap() - 0.00003612).abs() < 1e-12);
    assert_eq!(
        result
            .models
            .iter()
            .map(|r| r.value.unwrap_or(0.0))
            .sum::<f64>(),
        result.total.unwrap()
    );
    assert_eq!(
        result
            .buckets
            .iter()
            .map(|r| r.value.unwrap_or(0.0))
            .sum::<f64>(),
        result.total.unwrap()
    );
    let calls = analyze(
        OverviewQuery {
            metric: "calls".into(),
            ..query.clone()
        },
        &grouped,
        now,
    )
    .unwrap();
    assert_eq!(calls.total, Some(4.0));
    let filtered = analyze(
        OverviewQuery {
            agent: Some("claude-code-cli".into()),
            ..query.clone()
        },
        &grouped,
        now,
    )
    .unwrap();
    assert_eq!(filtered.total, Some(0.0));
    assert_eq!(filtered.cost.actual_usd, None);
    let unknown = AnalysisRow {
        cost: AnalysisCost {
            unknown_attempts: 1,
            ..Default::default()
        },
        ..grouped[0].clone()
    };
    assert!(analyze(query, &[unknown], now).unwrap().total.is_none());
}

#[test]
fn reused_retry_keys_do_not_merge_different_providers_or_sessions() {
    let mut a = row("a", "https://api.deepseek.com", "deepseek-flash");
    a.logical_id = Some("key".into());
    a.session_id = Some("opencode:one".into());
    let mut b = a.clone();
    b.id = "b".into();
    b.session_id = Some("opencode:two".into());
    let mut c = a.clone();
    c.id = "c".into();
    c.provider_id = Some("another".into());
    let mut d = a.clone();
    d.id = "d".into();
    let input = vec![a, b, c, d];
    let rows = gateway_rows(&input);
    assert_eq!(rows.len(), 3);
    assert_eq!(
        local_agent_core::requests::logical_requests(input.clone(), &HashMap::new()).len(),
        3
    );
    let q = OverviewQuery {
        agent: Some("opencode".into()),
        range: 24,
        metric: "calls".into(),
        dimension: "agent".into(),
        source: "gateway".into(),
    };
    let now = chrono::DateTime::parse_from_rfc3339("2026-10-20T03:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    assert_eq!(timing_stats(&input, &q, now).retry_groups_unmeasurable, 1);
    assert_eq!(timing_stats(&input, &q, now).success, 2);
}
