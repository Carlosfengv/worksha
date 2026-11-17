use super::*;
fn provider(store: &Store, model: &str) -> Provider {
    let mut p = store.providers().unwrap().remove(0);
    p.endpoint = "http://127.0.0.1:32101".into();
    p.configured = true;
    p.model = model.into();
    p.models = vec!["a".into(), "b".into()];
    p.credential_ref = None;
    p
}
fn turn(session: &str, chain: &[&str], continuation: bool, portable: bool) -> Turn {
    Turn {
        session: Some(session.into()),
        digest: chain.join(":"),
        chain: chain.iter().map(|s| (*s).into()).collect(),
        continuation,
        portable,
    }
}
fn change(store: &Store, b: &Binding, p: &Provider) -> Change {
    let c = store
        .dynamic_prepare(&uuid::Uuid::new_v4().to_string(), &b.id, &b.generation, p)
        .unwrap();
    store.dynamic_commit(&c.id).unwrap()
}
#[test]
fn turn_continuations_old_retries_and_restarts_keep_their_versions() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let a = provider(&store, "a");
    let b = provider(&store, "b");
    let binding = store
        .dynamic_create("claude", "scope", &uuid::Uuid::new_v4().to_string(), &a)
        .unwrap();
    let first = store
        .dynamic_admit(&binding.id, &turn("s", &["one"], false, true))
        .unwrap();
    let changed = change(&store, &binding, &b);
    let b_version = changed.published.unwrap();
    assert_eq!(
        store
            .dynamic_admit(&binding.id, &turn("s", &["one"], true, true))
            .unwrap()
            .version
            .id,
        first.version.id
    );
    assert_eq!(
        store
            .dynamic_admit(&binding.id, &turn("s", &["one", "two"], false, true))
            .unwrap()
            .version
            .id,
        b_version.id
    );
    store
        .connection
        .execute_batch("ALTER TABLE gateway_route_leases DROP COLUMN output_validated")
        .unwrap();
    drop(store);
    let store = Store::open(dir.path()).unwrap();
    assert_eq!(
        store
            .dynamic_admit(&binding.id, &turn("s", &["one"], true, true))
            .unwrap()
            .version
            .id,
        first.version.id
    );
    assert_eq!(
        store
            .dynamic_admit(&binding.id, &turn("s", &["one", "two"], true, true))
            .unwrap()
            .version
            .id,
        b_version.id
    );
    assert_eq!(
        store
            .dynamic_admit(&binding.id, &turn("new", &["one"], false, true))
            .unwrap()
            .version
            .id,
        b_version.id
    );
}
#[test]
fn compacted_and_unidentified_sessions_never_silently_switch() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let binding = store
        .dynamic_create(
            "opencode",
            "scope",
            &uuid::Uuid::new_v4().to_string(),
            &provider(&store, "a"),
        )
        .unwrap();
    let old = store
        .dynamic_admit(&binding.id, &turn("s", &["one", "two"], false, true))
        .unwrap()
        .version
        .id;
    let mut unknown = turn("ignored", &["one"], false, true);
    unknown.session = None;
    store.dynamic_admit(&binding.id, &unknown).unwrap();
    change(&store, &binding, &provider(&store, "b"));
    for chain in [
        &["summary"][..],
        &["summary", "three"][..],
        &["summary", "three", "four"][..],
    ] {
        assert_eq!(
            store
                .dynamic_admit(&binding.id, &turn("s", chain, false, true))
                .unwrap()
                .version
                .id,
            old
        );
    }
    unknown.chain.push("two".into());
    unknown.digest = "another".into();
    assert_eq!(
        store
            .dynamic_admit(&binding.id, &unknown)
            .unwrap()
            .version
            .id,
        old
    );
}

#[test]
fn opencode_upgrade_requires_both_histories_and_reuses_exact_legacy_leases() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let binding = store
        .dynamic_create(
            "opencode",
            "scope",
            &uuid::Uuid::new_v4().to_string(),
            &provider(&store, "a"),
        )
        .unwrap();
    let route = &binding.id;
    for (session, tail) in [
        ("restored", &["unknown-one", "unknown-two"][..]),
        ("different", &["other-one", "other-two"][..]),
    ] {
        let first = turn(session, &["legacy-one"], false, true);
        assert!(store.dynamic_admit(route, &first).unwrap().fresh);
        let pinned = store
            .dynamic_admit(route, &turn(session, tail, false, true))
            .unwrap();
        assert_eq!(pinned.reason, "session_boundary_unconfirmed");
    }
    let current = change(&store, &binding, &provider(&store, "b"))
        .published
        .unwrap();
    for session in ["restored", "different"] {
        let primary = turn(
            session,
            &["legacy-one", "legacy-two", "legacy-three"],
            false,
            true,
        );
        let alternate = turn(
            session,
            &["unknown-one", "unknown-two", "unknown-three"],
            false,
            true,
        );
        let (selected, effective) = store
            .gateway_admit(route, &primary, &[alternate], None, None, None)
            .map(|(selection, effective)| (selection, effective.digest.clone()))
            .unwrap();
        assert_eq!(effective, primary.digest);
        assert_eq!(selected.fresh, session == "restored");
        if selected.fresh {
            assert_eq!(selected.reason, "new_user_turn");
            assert_eq!(selected.version.id, current.id);
        } else {
            assert_eq!(selected.reason, "session_boundary_unconfirmed");
            assert_eq!(selected.version.id, binding.target.id);
        }
    }

    let retry = turn("restored", &["legacy-one", "legacy-two"], true, true);
    let alternate = [turn(
        "restored",
        &["unknown-one", "unknown-two"],
        true,
        true,
    )];
    let (selected, effective) = store
        .gateway_admit(route, &retry, &alternate, None, None, None)
        .unwrap();
    assert_eq!(effective.digest, alternate[0].digest);
    assert!(!selected.fresh);
    assert_eq!(selected.reason, "session_boundary_unconfirmed");
}

#[test]
fn opencode_can_reselect_after_native_user_boundaries_were_merged() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let binding = store
        .dynamic_create(
            "opencode",
            "scope",
            &uuid::Uuid::new_v4().to_string(),
            &provider(&store, "a"),
        )
        .unwrap();
    for session in ["verified", "missing_anchor", "tool", "signed"] {
        let first = turn(session, &["first", "old"], false, true);
        assert!(store.dynamic_admit(&binding.id, &first).unwrap().fresh);
        let untrusted = turn(session, &["first", "old+new"], false, true);
        assert_eq!(
            store.dynamic_admit(&binding.id, &untrusted).unwrap().reason,
            "session_boundary_unconfirmed"
        );
    }
    let current = change(&store, &binding, &provider(&store, "b"))
        .published
        .unwrap();
    for session in ["verified", "missing_anchor", "tool", "signed"] {
        let candidate = turn(
            session,
            &["first", "old+new+next"],
            session == "tool",
            session != "signed",
        );
        let mut proof = OpenCodeRouteProof {
            prefixes: HashMap::from([("first:old+new".into(), 3)]),
            ..Default::default()
        };
        if session != "missing_anchor" {
            proof.prefixes.insert("first:old".into(), 2);
        }
        let (selected, _) = store
            .gateway_admit(&binding.id, &candidate, &[], Some(&proof), None, None)
            .unwrap();
        assert_eq!(selected.fresh, session == "verified");
        if session == "verified" {
            assert_eq!(selected.version.id, current.id);
            let (retry, _) = store
                .gateway_admit(&binding.id, &candidate, &[], Some(&proof), None, None)
                .unwrap();
            assert!(!retry.fresh);
            assert_eq!(retry.version.id, current.id);
        } else {
            assert_eq!(selected.version.id, binding.target.id);
        }
    }
}
#[test]
fn thinking_history_stays_on_original_and_orphan_continuations_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let binding = store
        .dynamic_create(
            "claude",
            "scope",
            &uuid::Uuid::new_v4().to_string(),
            &provider(&store, "a"),
        )
        .unwrap();
    let old = store
        .dynamic_admit(&binding.id, &turn("s", &["one"], false, true))
        .unwrap()
        .version
        .id;
    change(&store, &binding, &provider(&store, "b"));
    assert_eq!(
        store
            .dynamic_admit(&binding.id, &turn("s", &["one", "two"], false, false))
            .unwrap()
            .version
            .id,
        old
    );
    assert_eq!(
        store
            .dynamic_admit(&binding.id, &turn("orphan", &["one"], true, true))
            .err()
            .unwrap()
            .code,
        "unknown_continuation"
    );
    assert!(
        store
            .dynamic_admit(&binding.id, &turn("old-thinking", &["one"], false, false))
            .is_err()
    );
}
#[test]
fn commit_idempotency_cas_cancellation_and_rollback_are_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let a = provider(&store, "a");
    let b = provider(&store, "b");
    let binding = store
        .dynamic_create("claude", "scope", &uuid::Uuid::new_v4().to_string(), &a)
        .unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let c = store.dynamic_prepare(&id, &binding.id, "1", &b).unwrap();
    assert_eq!(
        store.dynamic_prepare(&id, &binding.id, "1", &b).unwrap().id,
        c.id
    );
    assert!(store.dynamic_prepare(&id, &binding.id, "1", &a).is_err());
    assert!(
        store
            .dynamic_prepare(&uuid::Uuid::new_v4().to_string(), &binding.id, "1", &a)
            .is_err()
    );
    let published = store.dynamic_commit(&id).unwrap();
    assert_eq!(
        store.dynamic_commit(&id).unwrap().published.unwrap().id,
        published.published.unwrap().id
    );
    assert!(store.dynamic_cancel(&id).is_err());
    assert!(
        store
            .dynamic_prepare(&uuid::Uuid::new_v4().to_string(), &binding.id, "1", &a)
            .is_err()
    );
    let binding = store.dynamic_binding(&binding.id).unwrap().unwrap();
    let pending = store
        .dynamic_prepare(
            &uuid::Uuid::new_v4().to_string(),
            &binding.id,
            &binding.generation,
            &a,
        )
        .unwrap();
    store.dynamic_cancel(&pending.id).unwrap();
    assert!(store.dynamic_commit(&pending.id).is_err());
    let restored = change(&store, &binding, &a).published.unwrap();
    assert_ne!(restored.id, binding.target.id);
    assert_eq!(restored.provider.model, "a");
    assert_eq!(
        store
            .dynamic_binding(&binding.id)
            .unwrap()
            .unwrap()
            .generation,
        "3"
    );
}
#[test]
fn capacity_does_not_evict_existing_tools_and_migration_keeps_a_real_backup() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let b = store
        .dynamic_create(
            "claude",
            "scope",
            &uuid::Uuid::new_v4().to_string(),
            &provider(&store, "a"),
        )
        .unwrap();
    store.connection.execute("INSERT INTO gateway_route_leases(binding,session,turn,version,chain,trusted,reason) SELECT ?1,'s'||value,'t',?2,'[]',0,'fixture' FROM json_each(?3)",params![b.id,b.target.id,serde_json::to_string(&(0..MAX_LEASES).collect::<Vec<_>>()).unwrap()]).unwrap();
    assert_eq!(
        store
            .dynamic_admit(&b.id, &turn("new", &["one"], false, true))
            .err()
            .unwrap()
            .code,
        "capacity"
    );
    assert_eq!(
        store
            .dynamic_admit(
                &b.id,
                &Turn {
                    session: Some("s0".into()),
                    digest: "t".into(),
                    chain: vec![],
                    continuation: true,
                    portable: true
                }
            )
            .unwrap()
            .version
            .id,
        b.target.id
    );
    drop(store);
    let old = tempfile::tempdir().unwrap();
    let db = Connection::open(old.path().join("local-agent.sqlite3")).unwrap();
    db.execute_batch("CREATE TABLE sentinel(value TEXT); INSERT INTO sentinel VALUES('keep');PRAGMA user_version=2;").unwrap();
    drop(db);
    Store::open(&old.path().canonicalize().unwrap()).unwrap();
    let backup = Connection::open(old.path().join("local-agent.pre-dynamic-v2.sqlite3")).unwrap();
    assert_eq!(
        backup
            .query_row("SELECT value FROM sentinel", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "keep"
    );
    assert_eq!(
        backup
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn migration_rejects_unusable_or_linked_backups_without_advancing_schema() {
    for linked in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let db = Connection::open(root.join("local-agent.sqlite3")).unwrap();
        db.execute_batch("PRAGMA user_version=2;").unwrap();
        let backup = root.join("local-agent.pre-dynamic-v2.sqlite3");
        if linked {
            let untouched = root.join("untouched");
            std::fs::write(&untouched, b"keep").unwrap();
            std::os::unix::fs::symlink(&untouched, &backup).unwrap();
        } else {
            std::fs::write(&backup, b"partial backup").unwrap();
        }
        assert!(Store::open(&root).is_err());
        assert_eq!(
            db.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
                .unwrap(),
            2
        );
        if linked {
            assert_eq!(std::fs::read(root.join("untouched")).unwrap(), b"keep");
        }
    }
}

#[test]
fn m1_full_change_log_archives_terminal_receipts_and_rejects_replay() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let a = provider(&store, "a");
    let b = provider(&store, "b");
    let binding = store
        .dynamic_create("claude", "scope", &uuid::Uuid::new_v4().to_string(), &a)
        .unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    store
        .dynamic_prepare(&id, &binding.id, &binding.generation, &b)
        .unwrap();
    assert_eq!(
        retention::reclaim_changes(&store.connection)
            .unwrap_err()
            .code,
        "capacity"
    );
    let cancelled = store.dynamic_cancel(&id).unwrap();
    let mut older = cancelled.clone();
    older.created_at = "2000-01-01T00:00:00Z".into();
    store
        .connection
        .execute(
            "UPDATE gateway_route_changes SET value=?2 WHERE id=?1",
            params![id, encode(&older).unwrap()],
        )
        .unwrap();
    store
        .connection
        .execute(
            "WITH RECURSIVE x(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM x WHERE n<4095)
         INSERT INTO gateway_route_changes SELECT printf('archive-fixture-%04d',n),?1,'cancelled',
          json_set(?2,'$.id',printf('archive-fixture-%04d',n)) FROM x",
            params![binding.id, encode(&cancelled).unwrap()],
        )
        .unwrap();
    let next_id = uuid::Uuid::new_v4().to_string();
    store
        .connection
        .execute_batch(
            "CREATE TRIGGER reject_m1_preview BEFORE INSERT ON gateway_route_changes BEGIN
               SELECT RAISE(ABORT,'fixture');
             END;",
        )
        .unwrap();
    assert!(
        store
            .dynamic_prepare(&next_id, &binding.id, &binding.generation, &b)
            .is_err()
    );
    assert_eq!(
        store
            .connection
            .query_row(
                "SELECT count(*) FROM gateway_route_change_archive",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );
    store
        .connection
        .execute_batch("DROP TRIGGER reject_m1_preview")
        .unwrap();
    store
        .dynamic_prepare(&next_id, &binding.id, &binding.generation, &b)
        .unwrap();
    assert!(
        store
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM gateway_route_change_archive WHERE id=?1)",
                [&id],
                |r| r.get::<_, bool>(0)
            )
            .unwrap()
    );
    drop(store);
    let store = Store::open(dir.path()).unwrap();
    assert_eq!(
        store
            .dynamic_prepare(&id, &binding.id, &binding.generation, &b)
            .unwrap()
            .state,
        "cancelled"
    );
    assert_eq!(store.dynamic_commit(&id).unwrap_err().code, "conflict");
    assert_eq!(store.dynamic_change(&next_id).unwrap().state, "pending");
    assert_eq!(
        store
            .dynamic_binding(&binding.id)
            .unwrap()
            .unwrap()
            .generation,
        binding.generation
    );
}

#[test]
fn m1_full_lease_log_rejects_old_tools_and_only_revives_a_proven_archived_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let binding = store
        .dynamic_create(
            "opencode",
            "scope",
            &uuid::Uuid::new_v4().to_string(),
            &provider(&store, "a"),
        )
        .unwrap();
    let old = turn("old", &["one"], false, true);
    store.dynamic_admit(&binding.id, &old).unwrap();
    change(&store, &binding, &provider(&store, "b"));
    let head = turn("old", &["one", "two"], false, true);
    store.dynamic_admit(&binding.id, &head).unwrap();
    let seed = |start, finish| {
        store
            .connection
            .execute(
                "WITH RECURSIVE x(n) AS (VALUES(?2) UNION ALL SELECT n+1 FROM x WHERE n<?3)
             INSERT INTO gateway_route_leases(seq,binding,session,turn,version,chain,trusted,reason)
             SELECT n,?1,'other-'||n,'seed',target,'[\"seed\"]',1,'new_session'
             FROM x,gateway_bindings WHERE id=?1",
                params![binding.id, start, finish],
            )
            .unwrap();
        store.connection.execute(
            "INSERT OR IGNORE INTO gateway_session_watermarks(binding,session,head_seq,updated_at_ms)
             SELECT binding,session,seq,unixepoch()*1000 FROM gateway_route_leases WHERE binding=?1",
            [&binding.id],
        ).unwrap();
        store
            .connection
            .execute(
                "UPDATE gateway_route_sequences SET sequence=max(sequence,?1) WHERE kind='lease'",
                [finish],
            )
            .unwrap();
    };
    seed(3, 4096);
    store
        .dynamic_admit(&binding.id, &turn("new", &["first"], false, true))
        .unwrap();
    let old_tool = turn("old", &["one"], true, true);
    assert_eq!(
        store
            .dynamic_admit(&binding.id, &old_tool)
            .unwrap_err()
            .code,
        "stale_or_evicted_continuation"
    );
    store.connection.execute(
        "UPDATE gateway_session_watermarks SET updated_at_ms=0 WHERE binding=?1 AND session='old'",
        [&binding.id],
    ).unwrap();
    store
        .dynamic_admit(&binding.id, &turn("newer", &["first"], false, true))
        .unwrap();
    let last_seq: i64 = store
        .connection
        .query_row(
            "SELECT sequence FROM gateway_route_sequences WHERE kind='lease'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    store
        .connection
        .execute("DELETE FROM gateway_route_leases WHERE seq=?1", [last_seq])
        .unwrap();
    drop(store);
    let store = Store::open(dir.path()).unwrap();
    for rejected in [
        old_tool,
        head,
        turn("old", &["wrong", "two", "three"], false, true),
    ] {
        assert_eq!(
            store
                .dynamic_admit(&binding.id, &rejected)
                .unwrap_err()
                .code,
            "session_lease_archived"
        );
    }
    let restored = turn("old", &["one", "two", "three"], false, true);
    assert!(store.dynamic_admit(&binding.id, &restored).unwrap().fresh);
    assert_eq!(
        store
            .connection
            .query_row(
                "SELECT seq FROM gateway_route_leases WHERE binding=?1 AND session='old'",
                [&binding.id],
                |r| r.get::<_, i64>(0),
            )
            .unwrap(),
        last_seq + 1
    );
}

#[test]
fn m1_manual_archive_rechecks_preview_and_preserves_unresolved_session() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let binding = store
        .dynamic_create(
            "opencode",
            "scope",
            &uuid::Uuid::new_v4().to_string(),
            &provider(&store, "a"),
        )
        .unwrap();
    let id = store.database_id().unwrap();
    let admit = |session, chain: &[&str]| {
        store
            .dynamic_admit(&binding.id, &turn(session, chain, false, true))
            .unwrap();
    };
    admit("closed", &["one"]);
    admit("unresolved", &["other"]);
    let db = &store.connection;
    db.execute(
        "INSERT INTO service_route_attempts VALUES(1,?1,'unresolved','other',?2,'a',NULL,'outcome_unknown','fixture')",
        params![binding.id, binding.target.id],
    ).unwrap();
    let readonly = store.open_reader().unwrap();
    let before = readonly.preview_route_archive(&id).unwrap();
    assert_eq!((before.sessions, before.leases), (1, 1));
    let mut request = ArchiveRequest {
        store_id: id.clone(),
        confirmation: Some(before.confirmation),
    };
    admit("closed", &["one", "two"]);
    assert_eq!(
        store.archive_route_sessions(&request).err().unwrap().code,
        "conflict"
    );
    let current = readonly.preview_route_archive(&id).unwrap();
    assert_eq!((current.sessions, current.leases), (1, 2));
    request.confirmation = Some(current.confirmation);
    db.execute_batch(
        "CREATE TRIGGER fail_archive_delete BEFORE DELETE ON gateway_route_leases
        BEGIN SELECT RAISE(ABORT,'fixture'); END;",
    )
    .unwrap();
    assert!(store.archive_route_sessions(&request).is_err());
    assert_eq!(readonly.preview_route_archive(&id).unwrap().leases, 2);
    db.execute_batch("DROP TRIGGER fail_archive_delete")
        .unwrap();
    assert_eq!(store.archive_route_sessions(&request).unwrap().leases, 2);
    assert!(store.archive_route_sessions(&request).is_err());
    assert_eq!(readonly.preview_route_archive(&id).unwrap().sessions, 0);
    assert_eq!(
        store
            .dynamic_admit(&binding.id, &turn("closed", &["one", "two"], true, true))
            .unwrap_err()
            .code,
        "session_lease_archived"
    );
    admit("closed", &["one", "two", "three"]);
    assert_eq!(
        db.query_row(
            "SELECT outcome FROM service_route_attempts WHERE id=1",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "outcome_unknown"
    );
}

#[test]
fn m1_change_archive_budget_keeps_receipts_and_recovery_at_capacity() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let a = provider(&store, "a");
    let b = provider(&store, "b");
    let binding = store
        .dynamic_create("opencode", "first", &uuid::Uuid::new_v4().to_string(), &a)
        .unwrap();
    let older = change(&store, &binding, &b);
    let binding = store.dynamic_binding(&binding.id).unwrap().unwrap();
    let latest = change(&store, &binding, &a);
    let binding = store.dynamic_binding(&binding.id).unwrap().unwrap();
    let recovery_id = uuid::Uuid::new_v4().to_string();
    let recovery = store
        .dynamic_prepare_restore(&recovery_id, &older, &binding.generation)
        .unwrap();
    assert_eq!(
        retention::reclaim_changes(&store.connection)
            .unwrap_err()
            .code,
        "capacity"
    );
    store.dynamic_cancel(&recovery.id).unwrap();
    assert_eq!(
        retention::reclaim_changes_with_limit(&store.connection, 0)
            .unwrap_err()
            .code,
        "route_archive_capacity"
    );
    let history = |store: &Store, id: &str| {
        store
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM gateway_route_change_archive WHERE id=?1)",
                [id],
                |r| r.get::<_, bool>(0),
            )
            .unwrap()
    };
    assert!(!history(&store, &older.id));
    assert!(!history(&store, &recovery.id));
    retention::reclaim_changes(&store.connection).unwrap();
    assert!(history(&store, &older.id));
    assert!(history(&store, &recovery.id));
    assert!(!history(&store, &latest.id));
    drop(store);
    let store = Store::open(dir.path()).unwrap();
    assert_eq!(store.dynamic_commit(&older.id).unwrap().state, "applied");
    assert_eq!(
        store.dynamic_cancel(&recovery.id).unwrap().state,
        "cancelled"
    );
    assert_eq!(
        store
            .dynamic_prepare(&older.id, &binding.id, &older.expected_generation, &a)
            .unwrap_err()
            .code,
        "conflict"
    );
    let new_id = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        store
            .dynamic_prepare_restore(&new_id, &older, &binding.generation)
            .unwrap()
            .state,
        "pending"
    );
    assert_eq!(
        store
            .dynamic_binding(&binding.id)
            .unwrap()
            .unwrap()
            .generation,
        binding.generation
    );
    assert_eq!(store.dynamic_commit(&new_id).unwrap().state, "applied");
}

#[test]
fn m1_version_capacity_can_reclaim_a_route_after_lease_capacity_evicts_its_old_turn() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let binding = store
        .dynamic_create(
            "opencode",
            "scope",
            &uuid::Uuid::new_v4().to_string(),
            &provider(&store, "a"),
        )
        .unwrap();
    let db = &store.connection;
    db.execute(
        "WITH RECURSIVE x(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM x WHERE n<4095)
         INSERT INTO gateway_route_versions SELECT 'old-route-'||n,?1,
           json_set(v.value,'$.id','old-route-'||n)
           FROM x,gateway_route_versions v WHERE v.id=?2",
        params![binding.id, binding.target.id],
    )
    .unwrap();
    db.execute(
        "WITH RECURSIVE x(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM x WHERE n<4095)
         INSERT INTO gateway_route_leases(seq,binding,session,turn,version,chain,trusted,reason)
         SELECT n,?1,CASE WHEN n<3 THEN 'history' ELSE 'fixture-'||n END,
           CASE WHEN n=2 THEN 'one:two' ELSE 'one' END,'old-route-'||n,
           CASE WHEN n=2 THEN '[\"one\",\"two\"]' ELSE '[\"one\"]' END,1,'new_user_turn'
         FROM x",
        [&binding.id],
    )
    .unwrap();
    db.execute(
        "INSERT INTO gateway_route_leases(seq,binding,session,turn,version,chain,trusted,reason)
         VALUES(4096,?1,'recent','one',?2,'[\"one\"]',1,'new_session')",
        params![binding.id, binding.target.id],
    )
    .unwrap();
    db.execute_batch("UPDATE gateway_route_sequences SET sequence=4096 WHERE kind='lease';
        INSERT INTO gateway_session_watermarks(binding,session,head_seq,updated_at_ms)
        SELECT binding,session,max(seq),unixepoch()*1000 FROM gateway_route_leases GROUP BY binding,session;"
    ).unwrap();
    change(&store, &binding, &provider(&store, "b"));
    assert_eq!(
        store
            .dynamic_admit(&binding.id, &turn("history", &["one", "two"], false, true))
            .unwrap()
            .version
            .id,
        "old-route-2",
    );
    assert_eq!(
        store
            .dynamic_admit(&binding.id, &turn("history", &["one"], true, true))
            .unwrap_err()
            .code,
        "stale_or_evicted_continuation",
    );
    assert!(
        !db.query_row(
            "SELECT EXISTS(SELECT 1 FROM gateway_route_versions WHERE id='old-route-1')",
            [],
            |r| r.get::<_, bool>(0)
        )
        .unwrap()
    );
}
