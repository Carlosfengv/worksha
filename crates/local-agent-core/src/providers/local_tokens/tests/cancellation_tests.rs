use super::*;

fn count(f: &Fixture) -> (i64, i64, i64) {
    f.store.connection.query_row("SELECT cancellation_count,cancellation_bytes,(SELECT count(*) FROM local_token_cancellations) FROM local_token_scope WHERE id=1",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap()
}

#[test]
fn cancellation_fences_every_purpose_across_reopen_without_inventing_a_request_or_vault_value() {
    for purpose in [
        Purpose::FixedRoute,
        Purpose::DynamicBinding,
        Purpose::ModelService,
    ] {
        let f = Fixture::new();
        let request = f.request(purpose);
        let saved = f
            .manager
            .cancel_unowned_local_token(&f.store, &request.identity)
            .unwrap();
        assert_eq!(
            saved,
            Cancellation {
                version: 1,
                identity: request.identity.clone()
            }
        );
        assert_eq!(
            serde_json::to_value(&saved)
                .unwrap()
                .as_object()
                .unwrap()
                .len(),
            2
        );
        let before = count(&f);
        assert_eq!(
            f.manager
                .cancel_unowned_local_token(&f.store, &request.identity)
                .unwrap(),
            saved
        );
        assert_eq!(count(&f), before);
        assert!(
            f.store
                .local_token_record(&request.identity)
                .unwrap()
                .is_none()
        );
        assert!(
            f.manager
                .local_token_admission_guard(&f.store, &request.identity)
                .err()
                .unwrap()
                .code
                == "local_token_cancelled"
        );
        let data = f.root.path().join("data");
        drop(f.store);
        let reopened = Store::open(&data).unwrap();
        assert_eq!(
            reopened
                .local_token_cancellation(&request.identity)
                .unwrap(),
            Some(saved)
        );
        assert_eq!(
            f.manager
                .begin_local_token(&reopened, request)
                .err()
                .unwrap()
                .code,
            "local_token_cancelled"
        );
        assert!(f.vault.writes.lock().unwrap().is_empty());
    }
}

#[test]
fn cancellation_rejects_wrong_source_or_unverified_namespace_before_any_write() {
    let f = Fixture::new();
    let original = f.request(Purpose::FixedRoute).identity;
    for store in [true, false] {
        let mut wrong = original.clone();
        if store {
            wrong.store_id = uuid::Uuid::new_v4().to_string();
        } else {
            wrong.namespace_id = uuid::Uuid::new_v4().to_string();
        }
        assert!(
            f.manager
                .cancel_unowned_local_token(&f.store, &wrong)
                .is_err()
        );
        assert_eq!(count(&f), (0, 0, 0));
    }
    f.store
        .connection
        .execute(
            "UPDATE credential_recovery_scope SET state='unverified' WHERE id=1",
            [],
        )
        .unwrap();
    assert_eq!(
        f.manager
            .cancel_unowned_local_token(&f.store, &original)
            .unwrap_err()
            .code,
        "credential_namespace_unverified"
    );
    assert_eq!(count(&f), (0, 0, 0));
    assert!(f.vault.writes.lock().unwrap().is_empty());
}

#[test]
fn ignored_or_rewritten_cancellation_and_watermark_writes_roll_back_without_confirmation() {
    for trigger in [
        "CREATE TRIGGER fault BEFORE INSERT ON local_token_cancellations BEGIN SELECT RAISE(IGNORE);END;",
        "CREATE TRIGGER fault BEFORE UPDATE ON local_token_scope BEGIN SELECT RAISE(IGNORE);END;",
        "CREATE TRIGGER fault AFTER INSERT ON local_token_cancellations BEGIN UPDATE local_token_cancellations SET value_hash=printf('%064d',0) WHERE operation_id=NEW.operation_id;END;",
        "CREATE TRIGGER fault AFTER UPDATE ON local_token_scope BEGIN UPDATE local_token_scope SET cancellation_bytes=cancellation_bytes+1 WHERE id=1;END;",
    ] {
        let f = Fixture::new();
        let id = f.request(Purpose::FixedRoute).identity;
        f.store.connection.execute_batch(trigger).unwrap();
        assert!(f.manager.cancel_unowned_local_token(&f.store, &id).is_err());
        assert_eq!(count(&f), (0, 0, 0));
        assert!(f.store.connection.is_autocommit());
        assert!(f.vault.writes.lock().unwrap().is_empty());
        f.store
            .connection
            .execute_batch("DROP TRIGGER fault")
            .unwrap();
        assert_eq!(
            f.manager
                .cancel_unowned_local_token(&f.store, &id)
                .unwrap()
                .identity,
            id
        );
    }
}

#[test]
fn the_admission_lock_and_existing_owner_prevent_a_cancellation_from_replacing_work() {
    let f = Fixture::new();
    let request = f.request(Purpose::DynamicBinding);
    let guard = f
        .manager
        .local_token_admission_guard(&f.store, &request.identity)
        .unwrap();
    assert_eq!(
        f.manager
            .cancel_unowned_local_token(&f.store, &request.identity)
            .unwrap_err()
            .code,
        "busy"
    );
    assert_eq!(count(&f), (0, 0, 0));
    drop(guard);
    let mut owner = f
        .manager
        .begin_local_token(&f.store, request.clone())
        .unwrap();
    assert_eq!(
        f.manager
            .cancel_unowned_local_token(&f.store, &request.identity)
            .unwrap_err()
            .code,
        "busy"
    );
    owner.ensure_credential().unwrap();
    let reference = owner.operation().credential_ref.clone();
    drop(owner);
    assert!(
        f.manager
            .cancel_unowned_local_token(&f.store, &request.identity)
            .is_err()
    );
    assert_eq!(count(&f), (0, 0, 0));
    let mut owner = f
        .manager
        .resume_local_token(&f.store, &request.identity)
        .unwrap();
    assert_eq!(owner.cancel().unwrap().credential_ref, reference);
    assert_eq!(
        owner.check_credential().unwrap().unwrap().status,
        crate::providers::credential_operations::Status::Ready
    );
}

#[test]
fn an_existing_native_preview_is_not_replaced_with_a_no_owner_cancellation_proof() {
    let f = Fixture::new();
    let mut id = f.request(Purpose::FixedRoute).identity;
    let preview = f
        .manager
        .prepare(
            crate::providers::BindingDraft {
                operation: None,
                agent_id: "claude".into(),
                expected_revision: "absent".into(),
                provider_id: None,
                expected_provider_revision: None,
                model: None,
            },
            None,
        )
        .unwrap();
    id.operation_id = preview.id.clone();
    let before = f.vault.writes.lock().unwrap().len();
    assert!(f.manager.cancel_unowned_local_token(&f.store, &id).is_err());
    assert_eq!(count(&f), (0, 0, 0));
    assert_eq!(
        f.manager
            .status()
            .unwrap()
            .transactions
            .iter()
            .find(|t| t.id == preview.id)
            .unwrap()
            .state,
        "pending"
    );
    assert_eq!(f.vault.writes.lock().unwrap().len(), before);
}

#[test]
fn lost_or_corrupt_cancellation_evidence_never_allows_a_new_target_write() {
    for deleted in [false, true] {
        let f = Fixture::new();
        let request = f.request(Purpose::FixedRoute);
        f.manager
            .cancel_unowned_local_token(&f.store, &request.identity)
            .unwrap();
        if deleted {
            f.store
                .connection
                .execute("DELETE FROM local_token_cancellations", [])
                .unwrap();
        } else {
            f.store
                .connection
                .execute(
                    "UPDATE local_token_cancellations SET value_hash=?1",
                    ["0".repeat(64)],
                )
                .unwrap();
        }
        assert!(f.store.local_token_cancellation(&request.identity).is_err());
        assert!(f.manager.begin_local_token(&f.store, request).is_err());
        assert!(f.vault.writes.lock().unwrap().is_empty());
        assert_eq!(count(&f).0, 1);
    }
}

#[test]
fn cancellation_index_cannot_be_null_or_retarget_an_original_proof() {
    let f = Fixture::new();
    let request = f.request(Purpose::FixedRoute);
    let unrelated = f.request(Purpose::DynamicBinding);
    f.manager
        .cancel_unowned_local_token(&f.store, &request.identity)
        .unwrap();
    assert!(
        f.store
            .connection
            .execute("UPDATE local_token_cancellations SET operation_id=NULL", [])
            .is_err()
    );
    let other = uuid::Uuid::new_v4().to_string();
    f.store
        .connection
        .execute(
            "UPDATE local_token_cancellations SET operation_id=?1",
            [&other],
        )
        .unwrap();
    assert!(f.store.local_token_cancellation(&request.identity).is_err());
    assert!(f.manager.begin_local_token(&f.store, request).is_err());
    assert!(f.manager.begin_local_token(&f.store, unrelated).is_err());
    assert!(f.vault.writes.lock().unwrap().is_empty());
    assert_eq!(count(&f).0, 1);
}

#[test]
fn full_cancellation_capacity_preserves_original_proofs_and_pending_capacity() {
    let f = Fixture::new();
    let base = f.request(Purpose::FixedRoute).identity;
    let tx = f.store.connection.unchecked_transaction().unwrap();
    let mut bytes = 0;
    let mut first = None;
    for _ in 0..CANCELLATION_LIMIT {
        let id = IdentityV3 {
            operation_id: uuid::Uuid::new_v4().to_string(),
            ..base.clone()
        };
        let value = Cancellation {
            version: 1,
            identity: id.clone(),
        };
        let raw = serde_json::to_vec(&value).unwrap();
        bytes += raw.len();
        tx.execute(
            "INSERT INTO local_token_cancellations VALUES(?1,?2,?3,?4,?5)",
            rusqlite::params![
                id.operation_id,
                id.store_id,
                id.namespace_id,
                raw,
                files::hash(&raw)
            ],
        )
        .unwrap();
        if first.is_none() {
            first = Some(id);
        }
    }
    tx.execute(
        "UPDATE local_token_scope SET cancellation_count=?1,cancellation_bytes=?2",
        rusqlite::params![CANCELLATION_LIMIT as i64, bytes as i64],
    )
    .unwrap();
    tx.commit().unwrap();
    assert_eq!(
        f.manager
            .local_token_protocol(&f.store)
            .unwrap()
            .pending
            .len(),
        0
    );
    assert_eq!(
        f.manager
            .cancel_unowned_local_token(&f.store, &base)
            .unwrap_err()
            .code,
        "capacity"
    );
    let first = first.unwrap();
    assert_eq!(
        f.manager
            .cancel_unowned_local_token(&f.store, &first)
            .unwrap()
            .identity,
        first
    );
    assert_eq!(count(&f).0, CANCELLATION_LIMIT as i64);
    assert!(f.vault.writes.lock().unwrap().is_empty());
}

#[test]
fn schema14_upgrade_preserves_owned_ciphertext_and_reference_with_a_private_consistent_backup() {
    let f = Fixture::new();
    let request = f.request(Purpose::DynamicBinding);
    let mut owner = f
        .manager
        .begin_local_token(&f.store, request.clone())
        .unwrap();
    let reference = owner.ensure_credential().unwrap().credential_ref;
    drop(owner);
    let snapshot = || {
        let mut q=f.store.connection.prepare("SELECT domain,name,hex(value),value_hash,revision FROM local_token_values ORDER BY domain,name").unwrap();
        q.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
    };
    let values = snapshot();
    let raw: String = f
        .store
        .connection
        .query_row(
            "SELECT value FROM local_token_owners WHERE operation_id=?1",
            [&request.identity.operation_id],
            |row| row.get(0),
        )
        .unwrap();
    f.store.connection.execute_batch("DROP TABLE local_token_cancellations;ALTER TABLE local_token_scope DROP COLUMN cancellation_count;ALTER TABLE local_token_scope DROP COLUMN cancellation_bytes;UPDATE local_token_scope SET format=1;PRAGMA user_version=14;").unwrap();
    let data = f.root.path().join("data");
    drop(f.store);
    let reopened = Store::open(&data).unwrap();
    let backup =
        rusqlite::Connection::open(data.join("local-agent.pre-local-token-admission-v14.sqlite3"))
            .unwrap();
    assert_eq!(
        backup
            .query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        14
    );
    assert_eq!(
        backup
            .query_row("SELECT format FROM local_token_scope", [], |r| r
                .get::<_, u32>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        reopened
            .connection
            .query_row(
                "SELECT value FROM local_token_owners WHERE operation_id=?1",
                [&request.identity.operation_id],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        raw
    );
    let mut query=reopened.connection.prepare("SELECT domain,name,hex(value),value_hash,revision FROM local_token_values ORDER BY domain,name").unwrap();
    let after = query
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(values, after);
    assert_eq!(
        f.manager
            .resume_local_token(&reopened, &request.identity)
            .unwrap()
            .ensure_credential()
            .unwrap()
            .credential_ref,
        reference
    );
    assert!(
        reopened
            .local_token_cancellation(&request.identity)
            .unwrap()
            .is_none()
    );
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(data.join("local-agent.pre-local-token-admission-v14.sqlite3"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}
