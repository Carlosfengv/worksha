use super::*;
use crate::observation::retention::{
    PartDimension,
    test_support::{STORE, directory, retry},
};
use serde_json::{Value, json};

fn source() -> (tempfile::TempDir, ObservationStore, Request) {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let (request, rows) = retry();
    store.save_batch(&rows).unwrap();
    (temp, store, request)
}

fn stage(ledger: &FactLedger<'_>, request: &Request) {
    ledger
        .stage_queryable_batch(&[request], directory(), &[])
        .unwrap();
}

fn value(db: &Connection) -> String {
    db.query_row("SELECT value FROM retention_fact_staging", [], |r| r.get(0))
        .unwrap()
}

fn replace(db: &Connection, value: &str) {
    db.execute(
        "UPDATE retention_fact_staging SET value=?1,projection_digest=?2",
        params![value, digest(value)],
    )
    .unwrap();
}

#[test]
fn all_read_views_rehash_current_bytes_but_share_successful_semantic_checks() {
    let (_temp, store, request) = source();
    let ledger = store.retention_ledger(STORE).unwrap();
    stage(&ledger, &request);
    assert_eq!(ledger.validation_cache.borrow().counts(), (0, 0, 0));
    assert_eq!(ledger.staging_summary().unwrap().requests, 1);
    assert_eq!(ledger.attribution_coverage().unwrap().complete_facts, 1);
    assert_eq!(
        ledger.staging_scope_summary(None, None).unwrap().requests,
        1
    );
    ledger
        .staging_part_summary(PartDimension::Provider, "provider-a")
        .unwrap();
    ledger.staging_provider_contribution(None).unwrap();
    ledger.staging_model_contribution(None).unwrap();
    let filter = GatewayFilter::default();
    assert_eq!(
        ledger.staging_summary_filtered(&filter).unwrap().requests,
        1
    );
    ledger
        .staging_part_summary_filtered(PartDimension::Provider, "provider-a", &filter)
        .unwrap();
    ledger
        .staging_provider_contribution_filtered(None, &filter)
        .unwrap();
    ledger
        .staging_model_contribution_filtered(None, &filter)
        .unwrap();
    assert_eq!(ledger.validation_cache.borrow().counts(), (1, 10, 1));
}

#[test]
fn warmed_entries_never_hide_changed_key_state_source_or_projection_bytes() {
    for change in 0..5 {
        let (_temp, store, request) = source();
        let ledger = store.retention_ledger(STORE).unwrap();
        stage(&ledger, &request);
        ledger.staging_summary().unwrap();
        match change {
            0 => store
                .0
                .execute("UPDATE retention_fact_staging SET value=value||' '", []),
            1 => store
                .0
                .execute("UPDATE retention_fact_staging SET key=?1", ["f".repeat(64)]),
            2 => store.0.execute(
                "UPDATE retention_fact_staging SET source_fingerprint='invalid'",
                [],
            ),
            3 => {
                store
                    .0
                    .execute_batch("PRAGMA ignore_check_constraints=ON")
                    .unwrap();
                store
                    .0
                    .execute("UPDATE retention_fact_staging SET state='future-state'", [])
            }
            _ => store.0.execute(
                "UPDATE retention_fact_staging SET projection_digest=?1",
                ["a".repeat(64)],
            ),
        }
        .unwrap();
        // Even a filter selecting no records must validate the whole set.
        let filter = GatewayFilter {
            session: Some("never-matches".into()),
            ..Default::default()
        };
        assert_eq!(
            ledger.staging_summary_filtered(&filter).err(),
            Some(LedgerError::InvalidRecord),
            "change {change}"
        );
        let cache = ledger.validation_cache.borrow();
        assert!(cache.entries.is_empty());
        assert_eq!(cache.entries.capacity(), 0);
    }
}

#[test]
fn newly_checksummed_future_or_invalid_semantics_are_not_warm_cache_hits() {
    for field in ["version", "namespace", "requests", "unknown", "agent"] {
        let (_temp, store, request) = source();
        let ledger = store.retention_ledger(STORE).unwrap();
        stage(&ledger, &request);
        ledger.staging_summary().unwrap();
        let mut data: Value = serde_json::from_str(&value(&store.0)).unwrap();
        match field {
            "version" => data["version"] = json!(4),
            "namespace" => data["namespace"] = json!(uuid::Uuid::new_v4().to_string()),
            "requests" => data["stats"]["requests"] = json!(2),
            "unknown" => data["stats"]["futureField"] = json!(0),
            _ => data["attribution"]["attempts"][0]["agentIdentity"] = Value::Null,
        }
        replace(&store.0, &serde_json::to_string(&data).unwrap());
        assert_eq!(
            ledger.staging_summary().err(),
            Some(LedgerError::InvalidRecord),
            "field {field}"
        );
        assert_eq!(ledger.validation_cache.borrow().counts(), (0, 2, 2));
    }
}

#[test]
fn equivalent_new_bytes_get_a_full_check_and_explicit_release_frees_metadata() {
    let (_temp, store, request) = source();
    let ledger = store.retention_ledger(STORE).unwrap();
    stage(&ledger, &request);
    let before = serde_json::to_value(ledger.staging_summary().unwrap()).unwrap();
    let data: Value = serde_json::from_str(&value(&store.0)).unwrap();
    replace(&store.0, &serde_json::to_string_pretty(&data).unwrap());
    let after = serde_json::to_value(ledger.staging_summary().unwrap()).unwrap();
    assert_eq!(after, before);
    assert_eq!(ledger.validation_cache.borrow().counts(), (1, 2, 2));
    ledger.release_query_validation_cache();
    assert_eq!(ledger.validation_cache.borrow().entries.capacity(), 0);
    assert_eq!(ledger.staging_summary().unwrap().requests, 1);
    assert_eq!(ledger.validation_cache.borrow().counts(), (1, 3, 3));
}

#[test]
fn current_set_and_legacy_coverage_are_recounted_after_deletion_and_upgrade() {
    let (_temp, store, request) = source();
    let ledger = store.retention_ledger(STORE).unwrap();
    stage(&ledger, &request);
    assert_eq!(ledger.staging_summary().unwrap().requests, 1);
    store
        .0
        .execute("DELETE FROM retention_fact_staging", [])
        .unwrap();
    assert_eq!(ledger.staging_summary().unwrap().requests, 0);
    assert_eq!(ledger.validation_cache.borrow().entries.capacity(), 0);
    assert_eq!(ledger.attribution_coverage().unwrap().complete_facts, 0);
    ledger
        .stage_batch(&[(&request, [1; 32])], directory())
        .unwrap();
    assert_eq!(ledger.staging_summary().unwrap().requests, 1);
    assert_eq!(ledger.attribution_coverage().unwrap().summary_only_facts, 1);
    assert_eq!(
        ledger.staging_provider_contribution(None).err(),
        Some(LedgerError::IncompleteFacts)
    );
    ledger
        .stage_attributed_batch(&[&request], directory(), &[])
        .unwrap();
    assert_eq!(ledger.attribution_coverage().unwrap().complete_facts, 1);
    assert_eq!(ledger.validation_cache.borrow().counts().2, 3);
    stage(&ledger, &request);
    assert_eq!(ledger.attribution_coverage().unwrap().complete_facts, 1);
    // The deleted v3 memo was reclaimed, so restoring it requires a fresh check.
    assert_eq!(ledger.validation_cache.borrow().counts().2, 4);
    assert_eq!(ledger.staging_summary().unwrap().requests, 1);
    assert_eq!(ledger.validation_cache.borrow().counts().2, 4);
}

#[test]
fn external_commits_are_read_and_corruption_cannot_be_hidden_by_a_warm_entry() {
    let (temp, store, request) = source();
    let ledger = store.retention_ledger(STORE).unwrap();
    stage(&ledger, &request);
    ledger.staging_summary().unwrap();
    let original = value(&store.0);
    let external = Connection::open(temp.path().join("observations.sqlite3")).unwrap();
    let mut invalid: Value = serde_json::from_str(&original).unwrap();
    invalid["version"] = json!(99);
    replace(&external, &serde_json::to_string(&invalid).unwrap());
    assert_eq!(
        ledger.staging_summary().err(),
        Some(LedgerError::InvalidRecord)
    );
    assert_eq!(ledger.validation_cache.borrow().counts().0, 0);
    replace(&external, &original);
    assert_eq!(ledger.staging_summary().unwrap().requests, 1);
    external
        .execute("DELETE FROM retention_fact_staging", [])
        .unwrap();
    assert_eq!(ledger.staging_summary().unwrap().requests, 0);
}

#[test]
fn rolled_back_changes_never_leave_cached_set_counts_or_poison_restored_values() {
    let (_temp, store, request) = source();
    let ledger = store.retention_ledger(STORE).unwrap();
    stage(&ledger, &request);
    ledger.staging_summary().unwrap();
    let tx = store.0.unchecked_transaction().unwrap();
    tx.execute("UPDATE retention_fact_staging SET state='sealed'", [])
        .unwrap();
    assert_eq!(
        ledger
            .validation_cache
            .borrow_mut()
            .coverage(&tx, &ledger.namespace)
            .unwrap()
            .complete_facts,
        0
    );
    tx.rollback().unwrap();
    assert_eq!(ledger.staging_summary().unwrap().requests, 1);
    assert_eq!(ledger.validation_cache.borrow().counts(), (1, 3, 1));
}

#[test]
fn sealed_values_are_still_checked_even_though_they_do_not_contribute() {
    let (_temp, store, request) = source();
    let ledger = store.retention_ledger(STORE).unwrap();
    stage(&ledger, &request);
    ledger.staging_summary().unwrap();
    store
        .0
        .execute("UPDATE retention_fact_staging SET state='sealed'", [])
        .unwrap();
    assert_eq!(ledger.staging_summary().unwrap().requests, 0);
    assert_eq!(ledger.validation_cache.borrow().counts(), (1, 2, 1));
    let mut invalid: Value = serde_json::from_str(&value(&store.0)).unwrap();
    invalid["version"] = json!(4);
    replace(&store.0, &serde_json::to_string(&invalid).unwrap());
    assert_eq!(
        ledger.staging_summary().err(),
        Some(LedgerError::InvalidRecord)
    );
    assert_eq!(ledger.validation_cache.borrow().counts().0, 0);
}

#[test]
fn scope_and_case_insensitive_temp_shadows_reject_and_release_warm_metadata() {
    for shadow in [false, true] {
        let (_temp, store, request) = source();
        let ledger = store.retention_ledger(STORE).unwrap();
        stage(&ledger, &request);
        ledger.staging_summary().unwrap();
        if shadow {
            let old: i64 = store
                .0
                .query_row("PRAGMA temp.schema_version", [], |r| r.get(0))
                .unwrap();
            store.0.execute_batch(&format!("CREATE TEMP TABLE RETENTION_FACT_STAGING AS SELECT * FROM main.retention_fact_staging;PRAGMA temp.schema_version={old};")).unwrap();
        } else {
            let namespace = uuid::Uuid::new_v4().to_string();
            store
                .0
                .execute(
                    "UPDATE retention_fact_scope SET namespace=?1,identity_digest=?2",
                    params![namespace, scope_digest_v2(STORE, &namespace)],
                )
                .unwrap();
        }
        assert_eq!(
            ledger.staging_summary().err(),
            Some(LedgerError::InvalidRecord)
        );
        assert_eq!(ledger.validation_cache.borrow().entries.capacity(), 0);
    }
}

#[test]
fn stable_same_path_replacement_cannot_inherit_warm_query_qualification() {
    let (temp, store, request) = source();
    let ledger = store.retention_ledger(STORE).unwrap();
    stage(&ledger, &request);
    ledger.staging_summary().unwrap();
    let copy = temp.path().join("copy.sqlite3");
    store
        .0
        .execute("VACUUM INTO ?1", [copy.to_str().unwrap()])
        .unwrap();
    let bytes = std::fs::read(&copy).unwrap();
    let path = temp.path().join("observations.sqlite3");
    std::fs::rename(&copy, &path).unwrap();
    assert_eq!(
        ledger.staging_summary().err(),
        Some(LedgerError::SourceChanged)
    );
    assert_eq!(ledger.validation_cache.borrow().entries.capacity(), 0);
    assert_eq!(bytes, std::fs::read(path).unwrap());
}

#[test]
fn verified_bytes_do_not_transfer_to_another_key_or_namespace() {
    let (_temp, store, request) = source();
    let ledger = store.retention_ledger(STORE).unwrap();
    stage(&ledger, &request);
    let fact = value(&store.0);
    let data: Value = serde_json::from_str(&fact).unwrap();
    let key = data["key"].as_str().unwrap();
    let mut cache = Cache::default();
    cache
        .check(key, &ledger.namespace, &digest(&fact), &fact)
        .unwrap();
    for (key, namespace) in [
        ("a".repeat(64), ledger.namespace.clone()),
        (key.into(), uuid::Uuid::new_v4().to_string()),
    ] {
        assert_eq!(
            cache.check(&key, &namespace, &digest(&fact), &fact),
            Err(LedgerError::InvalidRecord)
        );
    }
    assert_eq!(cache.counts(), (1, 3, 3));
}

#[test]
fn entry_limit_never_turns_uncached_records_into_unvalidated_records() {
    let (_temp, store, request) = source();
    let ledger = store.retention_ledger(STORE).unwrap();
    let mut fact = GatewayFact::from_request(&request, &ledger.namespace, directory()).unwrap();
    let mut cache = Cache::default();
    for index in 0..=MAX_ENTRIES {
        fact.key = format!("{index:064x}");
        let value = serde_json::to_string(&fact).unwrap();
        assert!(
            !cache
                .check(&fact.key, &ledger.namespace, &digest(&value), &value)
                .unwrap()
        );
    }
    assert_eq!(cache.counts().0, MAX_ENTRIES);
    let value = serde_json::to_string(&fact).unwrap();
    cache
        .check(&fact.key, &ledger.namespace, &digest(&value), &value)
        .unwrap();
    assert_eq!(cache.counts().2, MAX_ENTRIES as u64 + 2);
    cache.clear();
    assert_eq!(cache.entries.capacity(), 0);
}

#[test]
fn legacy_views_reject_file_replacement_during_aggregation_before_returning() {
    use rusqlite::functions::{Aggregate, Context, FunctionFlags};
    use std::{
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };
    struct Swap {
        copy: PathBuf,
        path: PathBuf,
        swapped: Arc<AtomicBool>,
    }
    impl Aggregate<Option<String>, String> for Swap {
        fn init(&self, _: &mut Context<'_>) -> rusqlite::Result<Option<String>> {
            Ok(None)
        }
        fn step(&self, c: &mut Context<'_>, state: &mut Option<String>) -> rusqlite::Result<()> {
            *state = Some(c.get(0)?);
            if !self.swapped.swap(true, Ordering::SeqCst) {
                std::fs::rename(&self.copy, &self.path)
                    .map_err(|e| rusqlite::Error::UserFunctionError(Box::new(e)))?;
            }
            Ok(())
        }
        fn finalize(
            &self,
            _: &mut Context<'_>,
            state: Option<Option<String>>,
        ) -> rusqlite::Result<String> {
            Ok(state.flatten().unwrap_or_default())
        }
    }
    for view in 0..5 {
        let (temp, store, request) = source();
        let ledger = store.retention_ledger(STORE).unwrap();
        stage(&ledger, &request);
        ledger.staging_summary().unwrap();
        let copy = temp.path().join("copy.sqlite3");
        store
            .0
            .execute("VACUUM INTO ?1", [copy.to_str().unwrap()])
            .unwrap();
        let swapped = Arc::new(AtomicBool::new(false));
        for (name, args) in [("summary_sum", 3), ("stats_sum", 1)] {
            store
                .0
                .create_aggregate_function(
                    name,
                    args,
                    FunctionFlags::SQLITE_UTF8,
                    Swap {
                        copy: copy.clone(),
                        path: temp.path().join("observations.sqlite3"),
                        swapped: swapped.clone(),
                    },
                )
                .unwrap();
        }
        let error = match view {
            0 => ledger.staging_summary().err(),
            1 => ledger.staging_scope_summary(None, None).err(),
            2 => ledger
                .staging_part_summary(PartDimension::Provider, &request.attempts[0].provider_key)
                .err(),
            3 => ledger.staging_provider_contribution(None).err(),
            _ => ledger.staging_model_contribution(None).err(),
        };
        assert!(swapped.load(Ordering::SeqCst), "view {view}");
        assert_eq!(error, Some(LedgerError::SourceChanged), "view {view}");
        assert_eq!(ledger.validation_cache.borrow().entries.capacity(), 0);
    }
}
