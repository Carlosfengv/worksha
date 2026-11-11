use super::tests::{Fixture, claude, fixture, write};
use super::*;
use crate::storage::default_providers;

fn request(f: &Fixture) -> ImportRequest {
    let candidate = claude(f);
    let protocol = f.manager.native_import_protocol(&f.store).unwrap();
    ImportRequest {
        candidate_id: candidate.id,
        expected_revision: candidate.revision,
        model: None,
        operation: Some(operations::Identity {
            store_id: protocol.store_id,
            namespace_id: protocol.namespace_id,
            operation_id: uuid::Uuid::new_v4().to_string(),
            expected_sequence: protocol.sequence,
        }),
    }
}
fn configured() -> Fixture {
    let f = fixture();
    write(
        &f,
        "claude",
        r#"{"env":{"ANTHROPIC_API_KEY":"operation-test-native-key","ANTHROPIC_MODEL":"model"}}"#,
    );
    f
}
fn sql(f: &Fixture, sql: &str) {
    f.store.connection.execute_batch(sql).unwrap();
}

#[test]
fn cancel_wins_before_late_submit_and_never_deletes_a_staged_credential() {
    let f = configured();
    let input = request(&f);
    let cancelled = f
        .manager
        .native_import_operation(&f.store, &input, true)
        .unwrap();
    assert_eq!(cancelled.status, operations::Status::Cancelled);
    assert_eq!(
        f.manager
            .import_provider(&f.store, input)
            .err()
            .unwrap()
            .code,
        "native_import_cancelled"
    );
    assert!(f.vault.0.lock().unwrap().is_empty());
    assert_eq!(
        f.store.providers().unwrap().len(),
        default_providers().len()
    );
    sql(
        &f,
        "CREATE TRIGGER ignore_provider BEFORE INSERT ON providers BEGIN SELECT RAISE(IGNORE); END;",
    );
    let staged = request(&f);
    assert_eq!(
        f.manager
            .import_provider(&f.store, staged.clone())
            .err()
            .unwrap()
            .code,
        "native_import_result_unknown"
    );
    let before = f.vault.0.lock().unwrap().clone();
    assert_eq!(before.len(), 2);
    assert_eq!(
        f.manager
            .native_import_operation(&f.store, &staged, true)
            .unwrap()
            .status,
        operations::Status::Cancelled
    );
    assert_eq!(f.vault.0.lock().unwrap().clone(), before);
    assert_eq!(
        f.manager
            .import_provider(&f.store, staged)
            .err()
            .unwrap()
            .code,
        "native_import_cancelled"
    );
}
#[test]
fn ignored_intent_clock_or_bytes_roll_back_before_vault() {
    for trigger in [
        "CREATE TRIGGER ignore_intent BEFORE INSERT ON native_import_operations BEGIN SELECT RAISE(IGNORE); END;",
        "CREATE TRIGGER ignore_clock BEFORE UPDATE ON native_import_scope BEGIN SELECT RAISE(IGNORE); END;",
        "CREATE TRIGGER rewrite_intent AFTER INSERT ON native_import_operations BEGIN UPDATE native_import_operations SET value_hash='changed' WHERE operation_id=NEW.operation_id; END;",
    ] {
        let f = configured();
        let input = request(&f);
        sql(&f, trigger);
        assert!(f.manager.import_provider(&f.store, input).is_err());
        assert!(
            f.vault.0.lock().unwrap().is_empty(),
            "intent must commit before external writes"
        );
        assert_eq!(
            f.manager.native_import_protocol(&f.store).unwrap().sequence,
            0
        );
        assert_eq!(
            f.store.providers().unwrap().len(),
            default_providers().len()
        );
    }
}
#[test]
fn rewritten_provider_or_ignored_final_receipt_keeps_one_pending_owner() {
    for trigger in [
        "CREATE TRIGGER rewrite_provider AFTER INSERT ON providers BEGIN UPDATE providers SET value=json_set(value,'$.name','rewritten') WHERE id=NEW.id; END;",
        "CREATE TRIGGER ignore_receipt BEFORE UPDATE ON native_import_operations BEGIN SELECT RAISE(IGNORE); END;",
        "CREATE TRIGGER ignore_bytes BEFORE UPDATE ON native_import_scope WHEN NEW.sequence=OLD.sequence BEGIN SELECT RAISE(IGNORE); END;",
    ] {
        let f = configured();
        let input = request(&f);
        sql(&f, trigger);
        assert_eq!(
            f.manager
                .import_provider(&f.store, input.clone())
                .err()
                .unwrap()
                .code,
            "native_import_result_unknown"
        );
        assert_eq!(
            f.store.providers().unwrap().len(),
            default_providers().len()
        );
        assert_eq!(
            f.manager
                .native_import_operation(&f.store, &input, false)
                .unwrap()
                .status,
            operations::Status::Pending
        );
        let accounts = f.vault.0.lock().unwrap().clone();
        sql(
            &f,
            "DROP TRIGGER IF EXISTS rewrite_provider;DROP TRIGGER IF EXISTS ignore_receipt;DROP TRIGGER IF EXISTS ignore_bytes;",
        );
        let saved = f.manager.import_provider(&f.store, input.clone()).unwrap();
        assert_eq!(
            saved.provider.id,
            format!("custom-{}", input.operation.as_ref().unwrap().operation_id)
        );
        assert_eq!(f.vault.0.lock().unwrap().clone(), accounts);
        // Lost success replies replay the saved result, even after source removal.
        std::fs::remove_file(config::path(&f.home, "claude").unwrap()).unwrap();
        assert_eq!(
            f.manager
                .import_provider(&f.store, input)
                .unwrap()
                .provider
                .id,
            saved.provider.id
        );
    }
}
#[test]
fn pending_source_or_request_change_never_rebinds_the_original_operation() {
    let f = configured();
    let input = request(&f);
    sql(
        &f,
        "CREATE TRIGGER ignore_provider BEFORE INSERT ON providers BEGIN SELECT RAISE(IGNORE); END;",
    );
    assert!(f.manager.import_provider(&f.store, input.clone()).is_err());
    let accounts = f.vault.0.lock().unwrap().clone();
    let mut changed = input.clone();
    changed.model = Some("other".into());
    assert_eq!(
        f.manager
            .import_provider(&f.store, changed)
            .err()
            .unwrap()
            .code,
        "operation_conflict"
    );
    write(
        &f,
        "claude",
        r#"{"env":{"ANTHROPIC_API_KEY":"changed-key","ANTHROPIC_MODEL":"model"}}"#,
    );
    assert_eq!(
        f.manager
            .import_provider(&f.store, input.clone())
            .err()
            .unwrap()
            .code,
        "conflict"
    );
    assert_eq!(f.vault.0.lock().unwrap().clone(), accounts);
    assert_eq!(
        f.manager
            .native_import_operation(&f.store, &input, false)
            .unwrap()
            .status,
        operations::Status::Pending
    );
    let mut foreign = input.clone();
    foreign.operation.as_mut().unwrap().store_id = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        f.manager
            .import_provider(&f.store, foreign)
            .err()
            .unwrap()
            .code,
        "credential_store_changed"
    );
    let mut foreign = input;
    foreign.operation.as_mut().unwrap().namespace_id = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        f.manager
            .import_provider(&f.store, foreign)
            .err()
            .unwrap()
            .code,
        "credential_namespace_changed"
    );
}
#[test]
fn pending_capacity_is_checked_before_a_fifth_credential_and_cancellation_releases_a_slot() {
    let f = configured();
    sql(
        &f,
        "CREATE TRIGGER ignore_provider BEFORE INSERT ON providers BEGIN SELECT RAISE(IGNORE); END;",
    );
    let mut inputs = vec![];
    for _ in 0..operations::PENDING_LIMIT {
        let input = request(&f);
        assert!(f.manager.import_provider(&f.store, input.clone()).is_err());
        inputs.push(input);
    }
    let before = f.vault.0.lock().unwrap().clone();
    let fifth = request(&f);
    assert_eq!(
        f.manager
            .import_provider(&f.store, fifth.clone())
            .err()
            .unwrap()
            .code,
        "capacity"
    );
    assert_eq!(f.vault.0.lock().unwrap().clone(), before);
    f.manager
        .native_import_operation(&f.store, &inputs[0], true)
        .unwrap();
    sql(&f, "DROP TRIGGER ignore_provider;");
    assert!(f.manager.import_provider(&f.store, fifth).is_ok());
    assert_eq!(
        f.manager
            .native_import_protocol(&f.store)
            .unwrap()
            .pending_requests
            .len(),
        3
    );
}
#[test]
fn complete_metadata_validation_precedes_any_import_credential_or_intent() {
    let f = configured();
    write(&f,"claude",&serde_json::json!({"env":{"ANTHROPIC_API_KEY":"metadata-key","ANTHROPIC_MODEL":"model","ANTHROPIC_DEFAULT_SONNET_MODEL":"x".repeat(210)}}).to_string());
    assert!(f.manager.import_provider(&f.store, request(&f)).is_err());
    assert!(f.vault.0.lock().unwrap().is_empty());
    assert_eq!(
        f.manager.native_import_protocol(&f.store).unwrap().sequence,
        0
    );
}

#[test]
fn unnumbered_http_keeps_not_found_conflict_and_model_errors_without_allocating_an_intent() {
    let f = configured();
    for (candidate, revision, model, expected) in [
        (
            "../../outside".to_owned(),
            "x".to_owned(),
            Some("m".to_owned()),
            "not_found",
        ),
        (claude(&f).id, "stale".to_owned(), None, "conflict"),
        (
            claude(&f).id,
            claude(&f).revision,
            Some("".to_owned()),
            "invalid_config",
        ),
    ] {
        let input = ImportRequest {
            candidate_id: candidate,
            expected_revision: revision,
            model,
            operation: None,
        };
        assert_eq!(
            f.manager
                .import_provider(&f.store, input)
                .err()
                .unwrap()
                .code,
            expected
        );
        assert_eq!(
            f.manager.native_import_protocol(&f.store).unwrap().sequence,
            0
        );
        assert!(f.vault.0.lock().unwrap().is_empty());
    }
    let candidate = claude(&f);
    let saved = f
        .manager
        .import_provider(
            &f.store,
            ImportRequest {
                candidate_id: candidate.id,
                expected_revision: candidate.revision,
                model: Some(format!("{}\tmodel\t", " ".repeat(900))),
                operation: None,
            },
        )
        .unwrap();
    assert_eq!(saved.provider.model, "model");
    let raw: String = f
        .store
        .connection
        .query_row("SELECT value FROM native_import_operations", [], |r| {
            r.get(0)
        })
        .unwrap();
    let record: operations::Operation = serde_json::from_str(&raw).unwrap();
    assert_eq!(record.request.model.as_deref(), Some("model"));
}

#[test]
fn a_retained_occupant_proves_a_lost_sequence_race_but_missing_or_corrupt_evidence_stays_unknown() {
    let f = configured();
    let first = request(&f);
    let mut lost = first.clone();
    lost.operation.as_mut().unwrap().operation_id = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        f.manager
            .native_import_operation(&f.store, &lost, false)
            .err()
            .unwrap()
            .code,
        "native_import_result_unknown"
    );
    assert!(f.vault.0.lock().unwrap().is_empty());
    f.manager.import_provider(&f.store, first.clone()).unwrap();
    let accounts = f.vault.0.lock().unwrap().clone();
    std::fs::remove_file(config::path(&f.home, "claude").unwrap()).unwrap();
    for cancel in [false, true] {
        let expired = f
            .manager
            .native_import_operation(&f.store, &lost, cancel)
            .unwrap();
        assert_eq!(expired.status, operations::Status::Expired);
        assert!(expired.provider.is_none());
    }
    assert_eq!(
        f.manager
            .import_provider(&f.store, lost.clone())
            .err()
            .unwrap()
            .code,
        "native_import_expired"
    );
    assert_eq!(f.vault.0.lock().unwrap().clone(), accounts);
    let count: i64 = f
        .store
        .connection
        .query_row("SELECT count(*) FROM native_import_operations", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(count, 1);
    sql(
        &f,
        "UPDATE native_import_operations SET value_hash='broken'",
    );
    assert!(
        f.manager
            .native_import_operation(&f.store, &lost, false)
            .is_err(),
        "an unreadable occupant is never proof of expiry"
    );
    assert_eq!(f.vault.0.lock().unwrap().clone(), accounts);
}

#[test]
fn a_gap_in_the_sequence_ledger_never_proves_expiry() {
    let f = configured();
    let first = request(&f);
    let mut lost = first.clone();
    lost.operation.as_mut().unwrap().operation_id = uuid::Uuid::new_v4().to_string();
    f.manager.import_provider(&f.store, first).unwrap();
    let accounts = f.vault.0.lock().unwrap().clone();
    sql(&f, "UPDATE native_import_operations SET sequence=3");
    assert!(
        f.manager
            .native_import_operation(&f.store, &lost, false)
            .is_err()
    );
    assert!(
        f.manager
            .native_import_operation(&f.store, &lost, true)
            .is_err()
    );
    assert_eq!(f.vault.0.lock().unwrap().clone(), accounts);
}

#[test]
fn identical_source_bytes_in_a_different_home_cannot_resume_an_original_intent() {
    let f = configured();
    let input = request(&f);
    sql(
        &f,
        "CREATE TRIGGER ignore_provider BEFORE INSERT ON providers BEGIN SELECT RAISE(IGNORE); END;",
    );
    assert!(f.manager.import_provider(&f.store, input.clone()).is_err());
    let accounts = f.vault.0.lock().unwrap().clone();
    let home = f.home.parent().unwrap().join("another-home");
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::copy(
        config::path(&f.home, "claude").unwrap(),
        config::path(&home, "claude").unwrap(),
    )
    .unwrap();
    let other = ProviderManager::new(
        home,
        &f.data,
        f.manager.helper_source.clone(),
        f.vault.clone(),
    )
    .unwrap();
    assert_eq!(
        other
            .import_provider(&f.store, input.clone())
            .err()
            .unwrap()
            .code,
        "conflict"
    );
    assert_eq!(f.vault.0.lock().unwrap().clone(), accounts);
    assert_eq!(
        other
            .native_import_operation(&f.store, &input, false)
            .unwrap()
            .status,
        operations::Status::Pending
    );
}
#[test]
fn actual_schema_12_layout_is_backed_up_and_recovery_identity_and_values_survive() {
    let f = configured();
    let before = f.manager.native_import_protocol(&f.store).unwrap();
    let scope = f.store.credential_scope(&before.store_id).unwrap();
    let operation_id = uuid::Uuid::new_v4().to_string();
    let saved = f
        .manager
        .create_credential_v3(
            &scope,
            &before.namespace_id,
            &operation_id,
            "upgrade key",
            Zeroizing::new("upgrade-sentinel".into()),
        )
        .unwrap();
    let old_values: Vec<(i64, String, Vec<u8>, String, i64)> = {
        let mut query=f.store.connection.prepare("SELECT domain,name,value,value_hash,revision FROM credential_recovery_values ORDER BY domain,name").unwrap();
        query
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    sql(
        &f,
        "DROP TABLE local_token_cancellations;DROP TABLE local_token_owners;DROP TABLE local_token_values;DROP TABLE local_token_scope;DROP TABLE native_import_operations;DROP TABLE native_import_scope;PRAGMA user_version=12;",
    );
    drop(f.store);
    let reopened = Store::open(&f.data).unwrap();
    let after = f.manager.native_import_protocol(&reopened).unwrap();
    assert_eq!(before.store_id, after.store_id);
    assert_eq!(before.namespace_id, after.namespace_id);
    assert_eq!(after.sequence, 0);
    let restored = f
        .manager
        .credential_operation_v3(
            &reopened.credential_scope(&after.store_id).unwrap(),
            &after.namespace_id,
            &operation_id,
            false,
        )
        .unwrap();
    assert_eq!(saved.operation.receipt, restored.operation.receipt);
    let mut query=reopened.connection.prepare("SELECT domain,name,value,value_hash,revision FROM credential_recovery_values ORDER BY domain,name").unwrap();
    let new_values: Vec<(i64, String, Vec<u8>, String, i64)> = query
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(old_values, new_values);
    let path = f.data.join("local-agent.pre-native-import-v12.sqlite3");
    let backup =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    assert_eq!(
        backup
            .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        12
    );
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}
