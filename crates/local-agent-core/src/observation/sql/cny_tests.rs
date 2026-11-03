use super::*;

#[test]
fn provider_cny_totals_and_unknown_reasons_survive_pages_and_cold_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let prototype: ModelObservation = serde_json::from_value(json!({
        "id":"cny", "agentId":"opencode", "provider":"saved", "providerId":"saved",
        "endpoint":"https://api.deepseek.com", "bindingRevision":1,
        "startedAt":"2026-10-21T02:00:00Z", "endedAt":"2026-10-21T02:00:01Z",
        "status":"success", "model":"deepseek-flash", "protocol":"chat-completions",
        "revision":1, "usage":{"inputTokens":1_000_000,"outputTokens":100_000,"cachedInputTokens":800_000,"totalTokens":1_100_000}
    })).unwrap();
    let mut rows: Vec<_> = (0..85)
        .map(|i| {
            let mut r = prototype.clone();
            r.id = format!("known-{i:03}");
            r
        })
        .collect();
    for (reason, status, http, count) in [
        ("上游拒绝跨协议请求", "error", 400, 13),
        (
            "上游返回当前不可转换的思考或拒绝内容，请使用同协议连接",
            "error",
            200,
            2,
        ),
        ("客户端结束读取，未收到完整结束事件", "cancelled", 200, 3),
    ] {
        for i in 0..count {
            let mut r = prototype.clone();
            r.id = format!("unknown-{status}-{http}-{i}");
            r.status = status.into();
            r.http_status = Some(http);
            r.timing_issue = Some(reason.into());
            r.usage = Usage::default();
            rows.push(r);
        }
    }
    store.save_batch(&rows).unwrap();
    let originals: Vec<String> = {
        let mut q = store
            .0
            .prepare("SELECT value FROM model_observations ORDER BY id")
            .unwrap();
        q.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    let provider:crate::Provider=serde_json::from_value(json!({"id":"saved","name":"renamed","endpoint":"https://reseller.invalid","protocol":"responses","configured":true,"model":"changed"})).unwrap();
    let at = chrono::DateTime::parse_from_rfc3339("2026-10-25T05:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    for _ in 0..2 {
        let mut h = History::open(dir.path()).unwrap();
        h.refresh(&[]).unwrap();
        let mut q = provider_usage::Query {
            source: "gateway".into(),
            provider_id: Some("saved".into()),
            days: Some(7),
            ..Default::default()
        };
        let first = h
            .provider_usage(
                q.clone(),
                std::slice::from_ref(&provider),
                at.to_rfc3339(),
                "ready".into(),
                at,
            )
            .unwrap();
        assert_eq!(first["totalCalls"], 103);
        assert_eq!(first["selected"]["cost"]["estimated"][0]["currency"], "CNY");
        assert!(
            (first["selected"]["cost"]["estimated"][0]["amount"]
                .as_f64()
                .unwrap()
                - 85.0 * 1.232)
                .abs()
                < 1e-10
        );
        assert_eq!(first["selected"]["cost"]["unknownCalls"], 18);
        let reasons = &first["selected"]["unknownCostReasons"];
        assert_eq!(reasons["missing_usage_upstream_rejected"], 13);
        assert_eq!(reasons["missing_usage_unsupported_content"], 2);
        assert_eq!(reasons["missing_usage_client_cancelled"], 3);
        let mut ids = HashSet::new();
        let mut page = first.clone();
        loop {
            for row in page["calls"].as_array().unwrap() {
                assert!(ids.insert(row["id"].as_str().unwrap().to_string()));
            }
            assert_eq!(page["selected"], first["selected"]);
            let Some(cursor) = page["nextCursor"].as_str() else {
                break;
            };
            q.cursor = Some(cursor.into());
            page = h
                .provider_usage(
                    q.clone(),
                    std::slice::from_ref(&provider),
                    at.to_rfc3339(),
                    "ready".into(),
                    at,
                )
                .unwrap();
        }
        assert_eq!(ids.len(), 103);
    }
    let after: Vec<String> = {
        let mut q = store
            .0
            .prepare("SELECT value FROM model_observations ORDER BY id")
            .unwrap();
        q.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    assert_eq!(
        after, originals,
        "rebuilding currency estimates never rewrites source observations"
    );
}
