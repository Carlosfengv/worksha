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
