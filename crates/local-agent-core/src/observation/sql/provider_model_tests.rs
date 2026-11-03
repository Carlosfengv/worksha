use super::*;

#[test]
fn provider_model_groups_match_full_canonical_gateway_and_native_totals_across_pages() {
    let providers: Vec<crate::Provider> = ["p-a", "p-b"].into_iter().map(|id| {
        serde_json::from_value(json!({"id":id,"name":"Same","endpoint":"https://example.com","protocol":"messages","model":"same","configured":true})).unwrap()
    }).collect();
    let mut rows: Vec<_> = (0..89).map(row).collect();
    for item in &mut rows {
        item.provider_id = Some("p-a".into());
        item.model = Some("same".into());
    }
    rows[85].provider_id = Some("p-b".into());
    rows[85].usage.total_tokens = Some(0);
    rows[86].provider_id = None;
    rows[87].model = None;
    rows[88].model = Some("quoted \" / 中文".into());
    let records = provider_usage::gateway_records(rows.clone(), &[]);
    for source in ["gateway", "native"] {
        let dir = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(dir.path()).unwrap();
        store.save_batch(&rows).unwrap();
        let mut history = History::open(dir.path()).unwrap();
        history.refresh(&[]).unwrap();
        if source == "native" {
            history.set_native_usage(records.clone(), vec![]).unwrap();
        }
        let query = provider_usage::Query {
            source: source.into(),
            provider_id: Some("p-a".into()),
            model: Some("same".into()),
            ..Default::default()
        };
        let first = history
            .provider_usage(
                query.clone(),
                &providers,
                "asof".into(),
                "ready".into(),
                now(),
            )
            .unwrap();
        let mut expected = value(
            &provider_usage::aggregate(
                query.clone(),
                &providers,
                records.clone(),
                "asof".into(),
                "ready".into(),
                now(),
            )
            .unwrap(),
        )
        .unwrap();
        let mut got = first.clone();
        without_cursor(&mut got);
        without_cursor(&mut expected);
        assert_eq!(got, expected, "{source}");
        assert_eq!(first["calls"].as_array().unwrap().len(), 40);
        assert_eq!(first["providerModels"].as_array().unwrap().len(), 4);
        let mut next_query = query;
        next_query.cursor = first["nextCursor"].as_str().map(str::to_owned);
        let next = history
            .provider_usage(
                next_query,
                &providers,
                "later".into(),
                "ready".into(),
                now(),
            )
            .unwrap();
        assert_eq!(
            first["providerModels"], next["providerModels"],
            "cached page {source}"
        );
        let all = history
            .provider_usage(
                provider_usage::Query {
                    source: source.into(),
                    ..Default::default()
                },
                &providers,
                "asof".into(),
                "ready".into(),
                now(),
            )
            .unwrap();
        assert_eq!(
            first["providerModels"], all["providerModels"],
            "global scope {source}"
        );
    }
}
