use super::*;
use crate::observation::retention::GatewayFact;
use crate::provider_cost::Cost;

#[test]
fn whitelisted_retention_facts_reproduce_full_canonical_sql_statistics_without_details() {
    let temp = tempfile::tempdir().unwrap();
    let history = History::open(temp.path()).unwrap();
    history.db.execute_batch("CREATE TEMP TABLE retained_stats(stats TEXT,cache_in TEXT,cache_read TEXT,ttft_sample REAL,duration_sample REAL,throughput_sample REAL);").unwrap();
    let now = "2026-10-27T00:00:00Z"
        .parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap();
    let mut rows = vec![];
    for index in 0..120 {
        let mut row = super::super::tests::request(now + chrono::Duration::seconds(index));
        row.id = format!("attempt-{index:03}");
        row.logical_id = Some(format!("retry-{}", index / 3));
        row.clock_id = Some("same-clock".into());
        row.start_tick_ms = Some(index as f64 * 2000.0);
        row.duration_ms = Some(1000.0 + index as f64);
        row.ttft_ms = Some(index as f64);
        row.status = match index % 15 {
            0 => "error",
            4 => "cancelled",
            7 => "in_progress",
            _ => "success",
        }
        .into();
        row.usage.input_tokens = Some(if index == 101 { u64::MAX } else { index as u64 });
        row.usage.cached_input_tokens = Some(index as u64 / 2);
        row.usage.total_tokens = (index % 7 != 0).then_some(index as u64 * 2);
        rows.push(row);
    }
    let mut requests = requests::logical_requests(rows, &HashMap::new());
    // Controlled cost outcomes exercise actual/estimated separation and currencies;
    // no price feed, network or real account is used.
    for (index, request) in requests.iter_mut().enumerate() {
        for (attempt, row) in request.attempts.iter_mut().enumerate() {
            row.cost = if index % 5 == 0 {
                Cost::unknown("missing_usage")
            } else {
                Cost {
                    kind: if attempt == 0 { "actual" } else { "estimated" },
                    amount: Some(if index % 9 == 0 {
                        0.0
                    } else {
                        index as f64 / 1000.0
                    }),
                    currency: Some(if index % 2 == 0 { "USD" } else { "CNY" }),
                    reason: None,
                    source: None,
                    price: None,
                    components: vec![],
                }
            };
        }
        let fact = GatewayFact::from_request(
            request,
            "00000000-0000-4000-8000-000000000001",
            crate::agents::builtin_directory(),
        )
        .unwrap();
        let stats = fact.statistics();
        let (input, cached) = fact.cache_counters();
        history
            .db
            .execute(
                "INSERT INTO retained_stats VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    encode(stats).unwrap(),
                    input,
                    cached,
                    stats.ttft.p50,
                    stats.duration.p50,
                    stats.throughput.p50
                ],
            )
            .unwrap();
    }
    let expected = requests::Summary::of(&requests);
    let actual = aggregates::summary(&history.db, "retained_stats").unwrap();
    let actual_value = serde_json::to_value(&actual).unwrap();
    let mut expected_value = serde_json::to_value(&expected).unwrap();
    // The existing SQL summary adds one request contribution at a time. The
    // canonical in-memory summary adds each attempt; binary floating-point
    // addition can differ by a final bit. Only amount fields get a tight bound.
    for kind in ["actual", "estimated"] {
        for (index, total) in actual_value["cost"][kind]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
        {
            let got = total["amount"].as_f64().unwrap();
            let expected = expected_value["cost"][kind][index]["amount"]
                .as_f64()
                .unwrap();
            assert!((got - expected).abs() <= 1e-12);
            expected_value["cost"][kind][index]["amount"] = total["amount"].clone();
        }
    }
    assert_eq!(actual_value, expected_value);
    let store = ObservationStore::open(temp.path()).unwrap();
    let ledger = store
        .retention_ledger("00000000-0000-4000-8000-000000000001")
        .unwrap();
    let batch = requests
        .iter()
        .enumerate()
        .map(|(index, request)| (request, [index as u8; 32]))
        .collect::<Vec<_>>();
    assert_eq!(
        ledger
            .stage_batch(&batch, crate::agents::builtin_directory())
            .unwrap()
            .inserted,
        requests.len()
    );
    assert_eq!(
        serde_json::to_value(ledger.staging_summary().unwrap()).unwrap(),
        serde_json::to_value(&actual).unwrap()
    );
    let columns = history
        .db
        .prepare("PRAGMA temp.table_info(retained_stats)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        columns,
        [
            "stats",
            "cache_in",
            "cache_read",
            "ttft_sample",
            "duration_sample",
            "throughput_sample"
        ]
    );
}
