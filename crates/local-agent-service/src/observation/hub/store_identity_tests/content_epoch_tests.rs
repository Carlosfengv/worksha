use super::*;
use local_agent_core::observation::content_epoch;
use rusqlite::{Connection, TransactionBehavior};

fn purge(fixture: &Fixture) {
    let mut db = Connection::open(fixture.current().join("observations.sqlite3")).unwrap();
    let old = content_epoch::read(&db).unwrap();
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    assert_eq!(tx.execute("DELETE FROM model_observations", []).unwrap(), 1);
    content_epoch::advance(&tx, &old).unwrap();
    tx.commit().unwrap();
}

#[test]
fn a_committed_epoch_change_invalidates_frozen_pages_and_rebuilds_without_removed_details() {
    let fixture = Fixture::new();
    let snapshot = fixture.freeze();
    purge(&fixture);
    let mut ran = false;
    assert!(
        fixture
            .hub
            .cached_query(|history| {
                ran = true;
                frozen_page(history, &snapshot)
            })
            .is_err()
    );
    assert!(!ran);
    assert!(fixture.hub.sql.lock().unwrap().is_none());
    assert!(
        fixture
            .hub
            .query(|history| history.request_detail("fixture-row"))
            .unwrap()
            .is_none()
    );
    assert!(
        fixture
            .hub
            .cached_query(|history| frozen_page(history, &snapshot))
            .is_err()
    );
}

#[test]
fn an_epoch_changed_during_csv_encoding_discards_the_late_chunk_and_all_old_leases() {
    let fixture = Fixture::new();
    let snapshot = fixture.freeze();
    let id = uuid::Uuid::new_v4().to_string();
    fixture
        .hub
        .cached_query(|history| {
            history
                .begin_csv_export(&id, &snapshot, chrono::Utc::now())
                .map_err(|e| e.to_string())
        })
        .unwrap();
    let result = fixture.hub.cached_query(|history| {
        let chunk = history
            .csv_export_chunk(&id, -1, chrono::Utc::now())
            .map_err(|e| e.to_string())?;
        assert!(chunk.csv.contains("old-store"));
        purge(&fixture);
        Ok(chunk)
    });
    assert!(result.is_err());
    assert!(fixture.hub.sql.lock().unwrap().is_none());
    assert!(
        fixture
            .hub
            .cached_query(|history| history
                .finish_csv_export(&id, 0, 1, chrono::Utc::now())
                .map_err(|e| e.to_string()))
            .is_err()
    );
}

#[test]
fn an_epoch_changed_during_an_ordinary_read_discards_the_late_detail() {
    let fixture = Fixture::new();
    assert!(
        fixture
            .hub
            .query(|history| {
                let value = detail(history)?;
                assert_eq!(value["provider"], "old-store");
                purge(&fixture);
                Ok(value)
            })
            .is_err()
    );
    assert!(fixture.hub.sql.lock().unwrap().is_none());
    assert!(
        fixture
            .hub
            .query(|history| history.request_detail("fixture-row"))
            .unwrap()
            .is_none()
    );
}

#[test]
fn unknown_metadata_blocks_cached_and_fresh_reads_without_recreating_a_token() {
    for mutation in [
        "DELETE FROM observation_content_epoch",
        "DROP TABLE observation_content_epoch",
        "UPDATE observation_content_epoch SET format=2",
        "UPDATE observation_content_epoch SET value='corrupt'",
        "PRAGMA user_version=2",
        "PRAGMA user_version=0",
    ] {
        let fixture = Fixture::new();
        fixture.freeze();
        let db = Connection::open(fixture.current().join("observations.sqlite3")).unwrap();
        db.execute_batch(mutation).unwrap();
        let mut ran = false;
        assert!(
            fixture
                .hub
                .cached_query(|_| {
                    ran = true;
                    Ok(())
                })
                .is_err(),
            "{mutation}"
        );
        assert!(!ran);
        assert!(fixture.hub.sql.lock().unwrap().is_none());
        assert!(fixture.hub.query(detail).is_err(), "{mutation}");
        assert_eq!(
            db.query_row("SELECT count(*) FROM model_observations", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}

#[test]
fn uncommitted_and_rolled_back_changes_do_not_revoke_a_valid_frozen_read() {
    let fixture = Fixture::new();
    let snapshot = fixture.freeze();
    let mut db = Connection::open(fixture.current().join("observations.sqlite3")).unwrap();
    let old = content_epoch::read(&db).unwrap();
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    tx.execute("DELETE FROM model_observations", []).unwrap();
    content_epoch::advance(&tx, &old).unwrap();
    let page = fixture
        .hub
        .cached_query(|history| frozen_page(history, &snapshot))
        .unwrap();
    assert_eq!(page["items"][0]["provider"], "old-store");
    drop(tx);
    assert_eq!(content_epoch::read(&db).unwrap(), old);
    assert!(
        fixture
            .hub
            .cached_query(|history| frozen_page(history, &snapshot))
            .is_ok()
    );
    purge(&fixture);
    assert!(
        fixture
            .hub
            .cached_query(|history| frozen_page(history, &snapshot))
            .is_err()
    );
}

#[test]
fn an_error_after_a_committed_content_change_still_drops_the_old_projection() {
    let fixture = Fixture::new();
    fixture.freeze();
    let result: Result<(), String> = fixture.hub.cached_query(|_| {
        purge(&fixture);
        Err("injected encoding failure".into())
    });
    assert!(result.is_err());
    assert!(fixture.hub.sql.lock().unwrap().is_none());
}
