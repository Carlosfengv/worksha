use super::*;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

fn calls(count: usize) -> crate::sessions::CallCatalog {
    let prototype = crate::associated_cost_tests::catalog().calls[0].clone();
    crate::sessions::CallCatalog::shared(std::iter::once(Arc::new(
        (0..count)
            .map(|n| {
                let mut call = prototype.clone();
                call.id = format!("native-bulk-{n}");
                call.response_id = Some(format!("bulk-response-{n}"));
                call.timestamp = Some((now() - chrono::Duration::hours(1)).to_rfc3339());
                call.usage = Some(Usage {
                    input_tokens: Some(10),
                    output_tokens: Some(2),
                    total_tokens: Some(12),
                    ..Default::default()
                });
                call
            })
            .collect(),
    )))
}

fn indexes(h: &History) -> Vec<(String, Option<String>)> {
    let mut statement = h.db.prepare("SELECT name,sql FROM sqlite_temp_master WHERE type='index' AND tbl_name IN ('native_base','record_meta') ORDER BY name").unwrap();
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn native_count(h: &History) -> i64 {
    h.db.query_row("SELECT count(*) FROM native_base", [], |row| row.get(0))
        .unwrap()
}

fn assert_empty_native_projection(h: &History) {
    for table in [
        "native_base",
        "record_meta",
        "record_payloads",
        "current",
        "contexts",
    ] {
        let count: i64 =
            h.db.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "{table} retained a partial bulk load");
    }
}

#[test]
fn first_populated_catalog_defers_indexes_after_empty_reads_and_keeps_totals() {
    for (empty_reads, tool_only) in [(0, false), (2, false), (1, true)] {
        let dir = tempfile::tempdir().unwrap();
        let mut h = History::open(dir.path()).unwrap();
        h.refresh(&[]).unwrap();
        for n in 0..empty_reads {
            let mut empty = if tool_only {
                calls(2)
            } else {
                Default::default()
            };
            if tool_only {
                for call in Arc::make_mut(&mut empty.chunks[0]) {
                    call.kind = "tool".into();
                }
            }
            h.sync_native_catalog(format!("empty-{n}"), &HashMap::new(), &empty)
                .unwrap();
        }
        let before = indexes(&h);
        h.db.execute_batch("CREATE TEMP TRIGGER require_bulk_load BEFORE INSERT ON native_base WHEN EXISTS(SELECT 1 FROM sqlite_temp_master WHERE name='records_order') BEGIN SELECT RAISE(ABORT,'secondary index maintained during initial bulk'); END;").unwrap();
        h.sync_native_catalog("populated".into(), &HashMap::new(), &calls(6))
            .unwrap_or_else(|e| panic!("empty_reads={empty_reads}: {e}"));
        assert_eq!(indexes(&h), before);
        assert_eq!(native_count(&h), 6);
        let q = requests::Query {
            source: Some("native".into()),
            ..Default::default()
        };
        let report = h.source_report(&q, now(), vec![], None).unwrap();
        assert_eq!(report["native"]["requests"], 6);
        assert_eq!(report["native"]["usage"]["totalTokens"], 72);
    }
}

#[test]
fn first_bulk_after_empty_rolls_back_index_rebuild_and_duplicates_before_retry() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    h.sync_native_catalog("empty".into(), &HashMap::new(), &Default::default())
        .unwrap();
    let before = indexes(&h);
    let recreated = Arc::new(AtomicUsize::new(0));
    let count = recreated.clone();
    h.db.authorizer(Some(move |context: AuthContext<'_>| {
        if matches!(
            context.action,
            AuthAction::CreateTempIndex {
                table_name: "native_base" | "record_meta",
                ..
            }
        ) && count.fetch_add(1, Ordering::SeqCst) == 2
        {
            return Authorization::Deny;
        }
        Authorization::Allow
    }))
    .unwrap();
    let result = h.sync_native_catalog("failed".into(), &HashMap::new(), &calls(3));
    h.db.authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
        .unwrap();
    assert!(result.is_err());
    assert_eq!(recreated.load(Ordering::SeqCst), 3);
    assert_eq!(indexes(&h), before);
    assert_empty_native_projection(&h);
    assert_eq!(h.native_revision.as_deref(), Some("empty"));
    assert!(h.native_request_calls.is_empty());

    // Catalogs deduplicate IDs before projection; use the shared raw-row path
    // to prove primary-key enforcement while secondary indexes are absent.
    let row = requests::sources::native_records(calls(1).iter().cloned(), &HashMap::new())
        .pop()
        .unwrap();
    assert!(
        h.sync_native("duplicate".into(), &HashMap::new(), [row.clone(), row])
            .is_err()
    );
    assert_eq!(indexes(&h), before);
    assert_empty_native_projection(&h);
    assert_eq!(h.native_revision.as_deref(), Some("empty"));
    h.sync_native_catalog("retry".into(), &HashMap::new(), &calls(3))
        .unwrap();
    assert_eq!(native_count(&h), 3);
    assert_eq!(indexes(&h), before);
}

#[test]
fn populated_gateway_and_incremental_catalog_updates_keep_secondary_indexes() {
    let dir = tempfile::tempdir().unwrap();
    ObservationStore::open(dir.path())
        .unwrap()
        .save_batch(&[row(0)])
        .unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    h.sync_native_catalog("empty".into(), &HashMap::new(), &Default::default())
        .unwrap();
    let before = indexes(&h);
    h.db.authorizer(Some(|context: AuthContext<'_>| {
        if matches!(
            context.action,
            AuthAction::DropTempIndex {
                table_name: "native_base" | "record_meta",
                ..
            }
        ) {
            Authorization::Deny
        } else {
            Authorization::Allow
        }
    }))
    .unwrap();
    let mut native = calls(2);
    h.sync_native_catalog("populated".into(), &HashMap::new(), &native)
        .unwrap();
    Arc::make_mut(&mut native.chunks[0])[0].model = Some("changed-model".into());
    h.sync_native_catalog("changed".into(), &HashMap::new(), &native)
        .unwrap();
    h.db.authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
        .unwrap();
    assert_eq!(indexes(&h), before);
    assert_eq!(native_count(&h), 2);
    assert_eq!(h.observation_page(None, None, 0).unwrap()["total"], 1);
}
