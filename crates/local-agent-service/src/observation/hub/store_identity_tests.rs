//! Store replacement is a fault injection; every path is owned by a TempDir.
use super::*;
use local_agent_core::{observation::sql::History as SqlHistory, requests};
use serde_json::{Value, json};
use std::path::PathBuf;

mod content_epoch_tests;

struct Fixture {
    hub: Hub,
    root: tempfile::TempDir,
}

fn observation(provider: &str) -> ModelObservation {
    serde_json::from_value(json!({
        "id":"fixture-row","agentId":"claude","provider":provider,
        "endpoint":"https://offline.invalid","bindingRevision":1,
        "startedAt":"2026-01-01T00:00:00Z","status":"success",
        "protocol":"messages","revision":1,"usage":{}
    }))
    .unwrap()
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        for (name, provider) in [("current", "old-store"), ("replacement", "new-store")] {
            let store = ObservationStore::open(&root.path().join(name)).unwrap();
            store.save(&observation(provider)).unwrap();
        }
        let hub = Hub::new(ObservationStore::open(&root.path().join("current")).unwrap()).unwrap();
        Self { hub, root }
    }

    fn current(&self) -> PathBuf {
        self.root.path().join("current")
    }

    fn remove_store(&self) {
        // Move the directory with all its private WAL files; no writer is active.
        std::fs::rename(self.current(), self.root.path().join("parked")).unwrap();
    }

    fn replace_store(&self) {
        self.remove_store();
        std::fs::rename(self.root.path().join("replacement"), self.current()).unwrap();
    }

    fn freeze(&self) -> String {
        self.hub
            .query(|history| {
                let page = history.request_page(
                    &requests::Query::default(),
                    chrono::Utc::now(),
                    Value::Null,
                    None,
                )?;
                assert_eq!(page["total"], 1);
                Ok(page["snapshot"].as_str().unwrap().to_owned())
            })
            .unwrap()
    }
}

fn detail(history: &mut SqlHistory) -> Result<Value, String> {
    history
        .request_detail("fixture-row")?
        .ok_or_else(|| "fixture row missing".into())
}

fn frozen_page(history: &mut SqlHistory, snapshot: &str) -> Result<Value, String> {
    history.request_page(
        &requests::Query {
            snapshot: Some(snapshot.into()),
            ..Default::default()
        },
        chrono::Utc::now(),
        Value::Null,
        None,
    )
}

#[test]
fn frozen_reads_do_not_run_after_the_store_is_missing_or_replaced() {
    for replaced in [false, true] {
        let fixture = Fixture::new();
        let snapshot = fixture.freeze();
        if replaced {
            fixture.replace_store();
        } else {
            fixture.remove_store();
        }
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
        // Restoring the original file must not resurrect a discarded snapshot.
        if !replaced {
            std::fs::rename(fixture.root.path().join("parked"), fixture.current()).unwrap();
        }
        assert!(fixture.hub.cached_query(|_| Ok(())).is_err());
        let current = fixture.hub.query(detail).unwrap();
        assert_eq!(
            current["provider"],
            if replaced { "new-store" } else { "old-store" }
        );
        assert!(
            fixture
                .hub
                .cached_query(|history| frozen_page(history, &snapshot))
                .is_err()
        );
    }
}

#[test]
fn a_replacement_during_a_frozen_export_discards_the_late_chunk() {
    let fixture = Fixture::new();
    let snapshot = fixture.freeze();
    let operation = uuid::Uuid::new_v4().to_string();
    fixture
        .hub
        .cached_query(|history| {
            history
                .begin_csv_export(&operation, &snapshot, chrono::Utc::now())
                .map_err(|error| error.to_string())
        })
        .unwrap();
    let result = fixture.hub.cached_query(|history| {
        let chunk = history
            .csv_export_chunk(&operation, -1, chrono::Utc::now())
            .map_err(|error| error.to_string())?;
        assert!(chunk.csv.contains("old-store"));
        fixture.replace_store();
        Ok(chunk)
    });
    assert!(result.is_err());
    assert!(fixture.hub.sql.lock().unwrap().is_none());
}

#[test]
fn an_ordinary_read_rejects_a_replacement_before_returning_its_old_detail() {
    let fixture = Fixture::new();
    let result = fixture.hub.query(|history| {
        let value = detail(history)?;
        assert_eq!(value["provider"], "old-store");
        fixture.replace_store();
        Ok(value)
    });
    assert!(result.is_err());
    assert!(fixture.hub.sql.lock().unwrap().is_none());
    assert_eq!(fixture.hub.query(detail).unwrap()["provider"], "new-store");
}

#[test]
fn an_error_during_a_replacement_still_discards_the_old_index() {
    let fixture = Fixture::new();
    fixture.freeze();
    let result: Result<(), String> = fixture.hub.cached_query(|_| {
        fixture.replace_store();
        Err("injected read failure".into())
    });
    assert!(result.is_err());
    assert!(fixture.hub.sql.lock().unwrap().is_none());
}

#[test]
fn same_file_updates_keep_frozen_rows_and_a_failed_read_keeps_the_valid_index() {
    let fixture = Fixture::new();
    let snapshot = fixture.freeze();
    ObservationStore::open(&fixture.current())
        .unwrap()
        .save(&observation("updated-in-same-store"))
        .unwrap();
    assert_eq!(
        fixture.hub.query(detail).unwrap()["provider"],
        "updated-in-same-store"
    );
    let result: Result<(), String> = fixture
        .hub
        .cached_query(|_| Err("injected read failure".into()));
    assert!(result.is_err());
    let page = fixture
        .hub
        .cached_query(|history| frozen_page(history, &snapshot))
        .unwrap();
    assert_eq!(page["items"][0]["provider"], "old-store");
    assert!(fixture.hub.sql.lock().unwrap().is_some());
}
