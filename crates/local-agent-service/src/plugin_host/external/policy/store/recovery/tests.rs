use super::*;
use std::os::unix::fs::PermissionsExt;
fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn core(root: &Path) -> Store {
    Store::open(&root.join("core")).unwrap()
}
fn bound(h: &Host, core: &Store) -> (Scope, Binding) {
    bound_named(h, core, id())
}
fn bound_named(h: &Host, core: &Store, service_id: String) -> (Scope, Binding) {
    let scope = Scope {
        store_id: core.model_service_operation_store_id().unwrap(),
        service_id,
    };
    let b = h
        .bind(
            BindRequest {
                store: &scope.store_id,
                service: &scope.service_id,
                version: &id(),
                expected: &None,
                package: &p::PackageQuery {
                    plugin_id: "test.policy".into(),
                    digest: "a".repeat(64),
                },
                failure: p::FailureMode::Reject,
                pending: None,
            },
            &Authorization {
                instance: id(),
                epoch: 1,
                publisher_revision: 1,
            },
        )
        .unwrap();
    (scope, b)
}

#[test]
fn legacy_uuid_spellings_keep_their_original_recovery_scope() {
    for original in [
        "ABCDEFAB-CDEF-4ABC-8DEF-ABCDEFABCDEF",
        "urn:uuid:abcdefab-cdef-1abc-8def-abcdefabcdef",
        "{abcdefab-cdef-4abc-8def-abcdefabcdef}",
        "abcdefabcdef4abc8defabcdefabcdef",
    ] {
        let root = tempfile::tempdir().unwrap();
        let core = core(root.path());
        let h = Host::new(root.path());
        let (scope, binding) = bound_named(&h, &core, original.into());
        lose(&h, &scope);
        drop(h);
        let mut h = Host::new(root.path());
        let (entries, _) = h.recovery_list(&core, &scope.store_id).unwrap();
        assert!(
            entries.iter().any(|e| e.scope == scope),
            "existing spelling missing: {original}"
        );
        let inspection = h.recovery_inspect(&core, scope.clone()).unwrap();
        assert!(
            inspection.can_restore_native,
            "existing spelling refused: {original}"
        );
        let reference = inspection.reference.unwrap();
        assert_eq!(reference.scope, scope);
        h.recovery_resolve(&core, reference, Mode::Native {})
            .unwrap();
        assert_eq!(
            h.consistent_control(&scope.store_id, original)
                .unwrap()
                .unwrap()
                .service_id,
            original
        );
        assert_eq!(
            encode(
                &h.binding(
                    &scope.store_id,
                    original,
                    &binding.status.service_version_id
                )
                .unwrap()
                .unwrap()
            )
            .unwrap(),
            encode(&binding).unwrap()
        );
        assert!(h.presence.read().unwrap().unknown.is_empty());
    }
}

#[test]
fn legacy_uuid_alias_recovery_cannot_clear_another_raw_scope_guard() {
    let root = tempfile::tempdir().unwrap();
    let core = core(root.path());
    let h = Host::new(root.path());
    let original = "ABCDEFAB-CDEF-4ABC-8DEF-ABCDEFABCDEF";
    let (upper, _) = bound_named(&h, &core, original.into());
    let (lower, _) = bound_named(&h, &core, original.to_lowercase());
    lose(&h, &upper);
    lose(&h, &lower);
    drop(h);
    let mut h = Host::new(root.path());
    let inspection = h.recovery_inspect(&core, lower.clone()).unwrap();
    h.recovery_resolve(&core, inspection.reference.unwrap(), Mode::Native {})
        .unwrap();
    assert!(
        h.presence
            .read()
            .unwrap()
            .unknown
            .contains(&original.to_lowercase()),
        "another original spelling is still unknown and must remain guarded"
    );
    let inspection = h.recovery_inspect(&core, upper.clone()).unwrap();
    h.recovery_resolve(&core, inspection.reference.unwrap(), Mode::Native {})
        .unwrap();
    assert!(h.presence.read().unwrap().unknown.is_empty());
}
fn lose(h: &Host, scope: &Scope) {
    h.db()
        .unwrap()
        .execute(
            "DELETE FROM policy_controls WHERE scope=?1",
            [Host::scope(&scope.store_id, &scope.service_id)],
        )
        .unwrap();
    std::fs::remove_file(
        h.root
            .join(marker_name(&scope.store_id, &scope.service_id).unwrap()),
    )
    .unwrap();
}
#[test]
fn consistent_orphan_requirement_can_be_retired_once_without_recreating_service() {
    let root = tempfile::tempdir().unwrap();
    let core = core(root.path());
    let mut h = Host::new(root.path());
    let (scope, binding) = bound(&h, &core);
    let inspection = h.recovery_inspect(&core, scope.clone()).unwrap();
    assert_eq!(inspection.entry.service_exists, Some(false));
    assert_eq!(inspection.entry.state, State::NeedsRecovery);
    assert!(inspection.can_restore_native && !inspection.can_restore_pending);
    h.recovery_resolve(&core, inspection.reference.unwrap(), Mode::Native {})
        .unwrap();
    assert!(core_state(&core, &scope).unwrap().is_null());
    assert_eq!(
        encode(
            &h.binding(
                &scope.store_id,
                &scope.service_id,
                &binding.status.service_version_id
            )
            .unwrap()
            .unwrap()
        )
        .unwrap(),
        encode(&binding).unwrap()
    );
    let inspection = h.recovery_inspect(&core, scope).unwrap();
    assert_eq!(inspection.entry.state, State::Consistent);
    assert!(!inspection.can_restore_native && !inspection.can_restore_pending);
    assert!(inspection.reference.is_none());
}
fn pending(h: &Host, core: &Store) -> (Scope, Intent) {
    let mut i = super::super::super::tests::prepared(h);
    let old_scope = Scope {
        store_id: i.status.store_id.clone(),
        service_id: i.status.service_id.clone(),
    };
    let mut c = h
        .control(&old_scope.store_id, &old_scope.service_id)
        .unwrap()
        .unwrap();
    lose(h, &old_scope);
    let scope = Scope {
        store_id: core.model_service_operation_store_id().unwrap(),
        service_id: i.status.service_id.clone(),
    };
    i.status.store_id = scope.store_id.clone();
    i.status.request.store_id = scope.store_id.clone();
    c.store_id = scope.store_id.clone();
    c.pending_request_identity = Some(operation_identity(&i.status.request).unwrap());
    h.save_control(&c).unwrap();
    h.save_intent(&i).unwrap();
    (scope, i)
}
#[test]
fn native_recovery_preserves_history_retires_authority_and_is_idempotent() {
    let root = tempfile::tempdir().unwrap();
    let core = core(root.path());
    let h = Host::new(root.path());
    let (scope, b) = bound(&h, &core);
    lose(&h, &scope);
    drop(h);
    let mut h = Host::new(root.path());
    let before = std::fs::read(&h.db_path).unwrap();
    let inspection = h.recovery_inspect(&core, scope.clone()).unwrap();
    assert_eq!(inspection.entry.service_exists, Some(false));
    assert!(inspection.can_restore_native);
    assert!(!inspection.can_restore_pending);
    assert_eq!(std::fs::read(&h.db_path).unwrap(), before);
    let tables: u32 = h
        .db()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name='policy_scope_recoveries'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tables, 0);
    let reference = inspection.reference.unwrap();
    let revision = h
        .recovery_resolve(&core, reference.clone(), Mode::Native {})
        .unwrap();
    let c = h
        .consistent_control(&scope.store_id, &scope.service_id)
        .unwrap()
        .unwrap();
    assert!(c.head.is_none() && c.pending.is_none());
    assert_ne!(c.authority, b.control_authority);
    assert_eq!(
        encode(
            &h.binding(
                &scope.store_id,
                &scope.service_id,
                &b.status.service_version_id
            )
            .unwrap()
            .unwrap()
        )
        .unwrap(),
        encode(&b).unwrap()
    );
    assert_eq!(
        h.recovery_resolve(&core, reference.clone(), Mode::Native {})
            .unwrap(),
        revision
    );
    let backup: String = h
        .db()
        .unwrap()
        .query_row(
            "SELECT record FROM policy_scope_recoveries WHERE id=?1",
            [&reference.reference_id],
            |r| r.get(0),
        )
        .unwrap();
    let r: Receipt = decode(&backup).unwrap();
    assert!(r.before.control.is_none() && r.before.marker.is_none());
    assert_eq!(r.before.fingerprint.len(), 64);
    let later = Control {
        revision: id(),
        authority: id(),
        ..c
    };
    h.save_control(&later).unwrap();
    assert_eq!(
        h.recovery_resolve(&core, reference, Mode::Native {})
            .unwrap_err()
            .code,
        "stale_snapshot"
    );
    assert_eq!(
        h.consistent_control(&scope.store_id, &scope.service_id)
            .unwrap(),
        Some(later)
    );
}
#[test]
fn references_expire_replace_and_reject_same_bytes_at_a_new_file_identity() {
    let root = tempfile::tempdir().unwrap();
    let core = core(root.path());
    let h = Host::new(root.path());
    let (scope, _) = bound(&h, &core);
    let path = h
        .root
        .join(marker_name(&scope.store_id, &scope.service_id).unwrap());
    std::fs::remove_file(&path).unwrap();
    drop(h);
    let mut h = Host::new(root.path());
    let first = h
        .recovery_inspect(&core, scope.clone())
        .unwrap()
        .reference
        .unwrap();
    let second = h
        .recovery_inspect(&core, scope.clone())
        .unwrap()
        .reference
        .unwrap();
    assert_eq!(
        h.recovery_resolve(&core, first, Mode::Native {})
            .unwrap_err()
            .code,
        "stale_snapshot"
    );
    h.recovery_references
        .get_mut(&second.reference_id)
        .unwrap()
        .created = Instant::now() - std::time::Duration::from_secs(61);
    assert_eq!(
        h.recovery_resolve(&core, second, Mode::Native {})
            .unwrap_err()
            .code,
        "stale_snapshot"
    );
    let current = h
        .recovery_inspect(&core, scope.clone())
        .unwrap()
        .reference
        .unwrap();
    let c = h
        .control(&scope.store_id, &scope.service_id)
        .unwrap()
        .unwrap();
    write_private(&path, encode(&c).unwrap().as_bytes()).unwrap();
    assert_eq!(
        h.recovery_resolve(&core, current, Mode::Native {})
            .unwrap_err()
            .code,
        "stale_snapshot"
    );
    assert!(
        h.control(&scope.store_id, &scope.service_id)
            .unwrap()
            .unwrap()
            .head
            .is_some()
    );
}
#[test]
fn pending_recovery_keeps_original_request_and_cancellation_without_grants() {
    let root = tempfile::tempdir().unwrap();
    let core = core(root.path());
    let h = Host::new(root.path());
    let (scope, i) = pending(&h, &core);
    let old = h
        .control(&scope.store_id, &scope.service_id)
        .unwrap()
        .unwrap();
    lose(&h, &scope);
    drop(h);
    let mut h = Host::new(root.path());
    let inspection = h.recovery_inspect(&core, scope.clone()).unwrap();
    assert!(!inspection.can_restore_native);
    assert!(inspection.can_restore_pending);
    assert_eq!(
        inspection.entry.pending_operation_ids,
        vec![i.status.operation_id.clone()]
    );
    h.recovery_resolve(
        &core,
        inspection.reference.unwrap(),
        Mode::PendingOriginal {},
    )
    .unwrap();
    let restored = h
        .consistent_control(&scope.store_id, &scope.service_id)
        .unwrap()
        .unwrap();
    assert_ne!(restored.authority, old.authority);
    assert_eq!(restored.revision, i.control_revision);
    assert_eq!(restored.pending, Some(i.status.operation_id.clone()));
    assert_eq!(
        restored.pending_request_identity,
        old.pending_request_identity
    );
    assert_eq!(
        encode(&h.intent(&i.status.operation_id).unwrap()).unwrap(),
        encode(&i).unwrap()
    );
    assert!(core.model_service_operation_for(&i.status.request).is_err());
    let cancelled = h.cancel_prepared(i).unwrap();
    assert_eq!(cancelled.status.state, "cancelled");
    assert!(
        h.consistent_control(&scope.store_id, &scope.service_id)
            .unwrap()
            .unwrap()
            .pending
            .is_none()
    );
}
#[test]
fn ambiguous_or_missing_prior_publication_evidence_is_blocked() {
    let root = tempfile::tempdir().unwrap();
    let core = core(root.path());
    let h = Host::new(root.path());
    let (scope, mut i) = pending(&h, &core);
    i.expected = Some(id());
    h.save_intent(&i).unwrap();
    lose(&h, &scope);
    drop(h);
    let mut h = Host::new(root.path());
    let inspection = h.recovery_inspect(&core, scope.clone()).unwrap();
    assert!(!inspection.can_restore_native && !inspection.can_restore_pending);
    assert!(inspection.reference.is_none());
    i.expected = None;
    h.save_intent(&i).unwrap();
    let mut other = i.clone();
    other.status.operation_id = id();
    other.status.request.operation_id = other.status.operation_id.clone();
    h.save_intent(&other).unwrap();
    let inspection = h.recovery_inspect(&core, scope).unwrap();
    assert_eq!(inspection.entry.pending_operation_ids.len(), 2);
    assert!(inspection.reference.is_none());
}

#[test]
fn surviving_original_pending_control_witnesses_a_prior_unbound_revision() {
    let root = tempfile::tempdir().unwrap();
    let core = core(root.path());
    let h = Host::new(root.path());
    let (scope, mut i) = pending(&h, &core);
    i.expected = Some(i.control_revision.clone());
    h.save_intent(&i).unwrap();
    std::fs::remove_file(
        h.root
            .join(marker_name(&scope.store_id, &scope.service_id).unwrap()),
    )
    .unwrap();
    drop(h);
    let mut h = Host::new(root.path());
    let inspection = h.recovery_inspect(&core, scope.clone()).unwrap();
    assert!(inspection.can_restore_pending && !inspection.can_restore_native);
    h.recovery_resolve(
        &core,
        inspection.reference.unwrap(),
        Mode::PendingOriginal {},
    )
    .unwrap();
    let c = h
        .consistent_control(&scope.store_id, &scope.service_id)
        .unwrap()
        .unwrap();
    assert!(c.head.is_none() && c.version.is_none());
    assert_eq!(c.pending, Some(i.status.operation_id.clone()));
    assert_eq!(
        encode(&h.intent(&i.status.operation_id).unwrap()).unwrap(),
        encode(&i).unwrap()
    );
}

#[test]
fn a_different_surviving_marker_cannot_be_overwritten_by_pending_reconstruction() {
    let root = tempfile::tempdir().unwrap();
    let core = core(root.path());
    let h = Host::new(root.path());
    let (scope, i) = pending(&h, &core);
    lose(&h, &scope);
    let later = Control {
        store_id: scope.store_id.clone(),
        service_id: scope.service_id.clone(),
        revision: id(),
        authority: id(),
        version: None,
        head: None,
        pending: None,
        pending_request_identity: None,
    };
    h.marker(&later).unwrap();
    drop(h);
    let mut h = Host::new(root.path());
    let inspection = h.recovery_inspect(&core, scope.clone()).unwrap();
    assert!(!inspection.can_restore_pending && !inspection.can_restore_native);
    assert!(inspection.reference.is_none());
    let bytes = std::fs::read(
        h.root
            .join(marker_name(&scope.store_id, &scope.service_id).unwrap()),
    )
    .unwrap();
    assert_eq!(serde_json::from_slice::<Control>(&bytes).unwrap(), later);
    assert_eq!(
        encode(&h.intent(&i.status.operation_id).unwrap()).unwrap(),
        encode(&i).unwrap()
    );
}
#[test]
fn sql_committed_marker_failure_is_unknown_and_exact_receipt_finishes_after_restart() {
    let root = tempfile::tempdir().unwrap();
    let core = core(root.path());
    let h = Host::new(root.path());
    let (scope, _) = bound(&h, &core);
    lose(&h, &scope);
    drop(h);
    let mut h = Host::new(root.path());
    let reference = h
        .recovery_inspect(&core, scope.clone())
        .unwrap()
        .reference
        .unwrap();
    std::fs::set_permissions(&h.root, std::fs::Permissions::from_mode(0o500)).unwrap();
    let result = h.recovery_resolve(&core, reference.clone(), Mode::Native {});
    std::fs::set_permissions(&h.root, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(result.unwrap_err().code, "scope_recovery_unknown");
    assert!(
        h.consistent_control(&scope.store_id, &scope.service_id)
            .is_err()
    );
    drop(h);
    let mut h = Host::new(root.path());
    let revision = h
        .recovery_resolve(&core, reference.clone(), Mode::Native {})
        .unwrap();
    assert_eq!(
        h.consistent_control(&scope.store_id, &scope.service_id)
            .unwrap()
            .unwrap()
            .revision,
        revision
    );
    assert_eq!(
        h.recovery_resolve(&core, reference, Mode::PendingOriginal {})
            .unwrap_err()
            .code,
        "stale_snapshot"
    );
}
#[test]
fn unknown_scope_reuses_its_reserved_slot_at_physical_capacity() {
    let root = tempfile::tempdir().unwrap();
    let core = core(root.path());
    let h = Host::new(root.path());
    let (scope, _) = bound(&h, &core);
    for _ in 1..LIMIT {
        let (s, b) = bound(&h, &core);
        h.unbind(&s.store_id, &s.service_id, &b.status.revision)
            .unwrap();
    }
    lose(&h, &scope);
    drop(h);
    let mut h = Host::new(root.path());
    assert_eq!(
        h.presence.read().unwrap().scopes.len() + h.presence.read().unwrap().unknown.len(),
        LIMIT
    );
    let inspection = h.recovery_inspect(&core, scope.clone()).unwrap();
    assert!(inspection.can_restore_native);
    h.recovery_resolve(&core, inspection.reference.unwrap(), Mode::Native {})
        .unwrap();
    assert_eq!(h.presence.read().unwrap().scopes.len(), LIMIT);
    assert!(h.presence.read().unwrap().unknown.is_empty());
    assert_eq!(std::fs::read_dir(&h.root).unwrap().count(), LIMIT);
}
#[test]
fn corrupt_pending_data_and_foreign_store_cannot_be_overwritten() {
    let root = tempfile::tempdir().unwrap();
    let core = core(root.path());
    let h = Host::new(root.path());
    let (scope, _) = bound(&h, &core);
    lose(&h, &scope);
    h.db()
        .unwrap()
        .execute(
            "INSERT INTO policy_intents VALUES(?1,?2)",
            params![id(), "{broken original record"],
        )
        .unwrap();
    drop(h);
    let mut h = Host::new(root.path());
    let inspection = h.recovery_inspect(&core, scope.clone()).unwrap();
    assert!(inspection.reference.is_none());
    assert_eq!(inspection.entry.state, State::Blocked);
    let (entries, _) = h.recovery_list(&core, &scope.store_id).unwrap();
    assert!(entries.iter().any(|e| e.scope == scope));
    assert!(h.recovery_list(&core, &id()).is_err());
    let original: String = h
        .db()
        .unwrap()
        .query_row("SELECT record FROM policy_intents", [], |r| r.get(0))
        .unwrap();
    assert_eq!(original, "{broken original record");
}
#[test]
fn empty_recovery_does_not_create_a_policy_repository() {
    let root = tempfile::tempdir().unwrap();
    let core = core(root.path());
    let mut h = Host::new(root.path());
    let store = core.model_service_operation_store_id().unwrap();
    let (entries, limited) = h.recovery_list(&core, &store).unwrap();
    assert!(entries.is_empty() && !limited);
    let inspection = h
        .recovery_inspect(
            &core,
            Scope {
                store_id: store,
                service_id: id(),
            },
        )
        .unwrap();
    assert!(inspection.reference.is_none());
    assert!(!h.db_path.exists() && !h.root.exists());
}

#[test]
fn urn_pending_identity_survives_both_control_losses_without_rewriting_the_request() {
    let root = tempfile::tempdir().unwrap();
    let core = core(root.path());
    let h = Host::new(root.path());
    let (old_scope, mut i) = pending(&h, &core);
    let mut c = h
        .control(&old_scope.store_id, &old_scope.service_id)
        .unwrap()
        .unwrap();
    lose(&h, &old_scope);
    let raw = format!("urn:uuid:{}", old_scope.service_id);
    let local_agent_core::model_services::operations::OperationAction::Save { draft } =
        &mut i.status.request.action
    else {
        unreachable!()
    };
    draft.id = raw.clone();
    i.status.service_id = raw.clone();
    c.service_id = raw.clone();
    c.pending_request_identity = Some(operation_identity(&i.status.request).unwrap());
    h.save_control(&c).unwrap();
    h.save_intent(&i).unwrap();
    let scope = Scope {
        store_id: old_scope.store_id,
        service_id: raw,
    };
    lose(&h, &scope);
    drop(h);
    let mut h = Host::new(root.path());
    assert!(
        h.recovery_list(&core, &scope.store_id)
            .unwrap()
            .0
            .iter()
            .any(|e| e.scope == scope)
    );
    let inspection = h.recovery_inspect(&core, scope.clone()).unwrap();
    assert!(inspection.can_restore_pending && !inspection.can_restore_native);
    h.recovery_resolve(
        &core,
        inspection.reference.unwrap(),
        Mode::PendingOriginal {},
    )
    .unwrap();
    assert_eq!(
        encode(&h.intent(&i.status.operation_id).unwrap()).unwrap(),
        encode(&i).unwrap()
    );
    assert_eq!(
        h.consistent_control(&scope.store_id, &scope.service_id)
            .unwrap()
            .unwrap()
            .pending,
        Some(i.status.operation_id)
    );
}

#[test]
fn two_unknown_spellings_keep_two_reserved_slots_at_the_256_scope_limit() {
    let root = tempfile::tempdir().unwrap();
    let core = core(root.path());
    let h = Host::new(root.path());
    let raw = "ABCDEFAB-CDEF-4ABC-8DEF-ABCDEFABCDEF";
    let (upper, _) = bound_named(&h, &core, raw.into());
    let (lower, _) = bound_named(&h, &core, raw.to_lowercase());
    for _ in 2..LIMIT {
        bound(&h, &core);
    }
    lose(&h, &upper);
    lose(&h, &lower);
    drop(h);
    let mut h = Host::new(root.path());
    assert_eq!(h.presence.read().unwrap().scope_count(), LIMIT);
    for scope in [lower, upper] {
        let inspection = h.recovery_inspect(&core, scope).unwrap();
        assert!(inspection.can_restore_native);
        h.recovery_resolve(&core, inspection.reference.unwrap(), Mode::Native {})
            .unwrap();
        assert_eq!(h.presence.read().unwrap().scope_count(), LIMIT);
    }
    assert_eq!(std::fs::read_dir(&h.root).unwrap().count(), LIMIT);
    assert!(h.presence.read().unwrap().unknown.is_empty());
}

#[test]
fn reference_capacity_preserves_other_scopes_and_latest_inspection_replaces_only_its_own() {
    let root = tempfile::tempdir().unwrap();
    let core = core(root.path());
    let h = Host::new(root.path());
    let scopes: Vec<_> = (0..33)
        .map(|_| {
            let (scope, _) = bound(&h, &core);
            lose(&h, &scope);
            scope
        })
        .collect();
    drop(h);
    let mut h = Host::new(root.path());
    let refs: Vec<_> = scopes[..32]
        .iter()
        .map(|scope| {
            h.recovery_inspect(&core, scope.clone())
                .unwrap()
                .reference
                .unwrap()
        })
        .collect();
    assert_eq!(
        h.recovery_inspect(&core, scopes[32].clone())
            .err()
            .unwrap()
            .code,
        "busy"
    );
    let replacement = h
        .recovery_inspect(&core, scopes[0].clone())
        .unwrap()
        .reference
        .unwrap();
    assert_eq!(h.recovery_references.len(), 32);
    assert_eq!(
        h.recovery_resolve(&core, refs[0].clone(), Mode::Native {})
            .unwrap_err()
            .code,
        "stale_snapshot"
    );
    h.recovery_resolve(&core, refs[1].clone(), Mode::Native {})
        .unwrap();
    assert!(
        h.recovery_inspect(&core, scopes[32].clone())
            .unwrap()
            .reference
            .is_some()
    );
    h.recovery_resolve(&core, replacement, Mode::Native {})
        .unwrap();
}

#[test]
fn full_recovery_receipts_reject_new_changes_but_keep_the_exact_known_result() {
    let root = tempfile::tempdir().unwrap();
    let core = core(root.path());
    let h = Host::new(root.path());
    let (first, _) = bound(&h, &core);
    lose(&h, &first);
    drop(h);
    let mut h = Host::new(root.path());
    let reference = h.recovery_inspect(&core, first).unwrap().reference.unwrap();
    let revision = h
        .recovery_resolve(&core, reference.clone(), Mode::Native {})
        .unwrap();
    let original: String = h
        .db()
        .unwrap()
        .query_row(
            "SELECT record FROM policy_scope_recoveries WHERE id=?1",
            [&reference.reference_id],
            |r| r.get(0),
        )
        .unwrap();
    for _ in 1..LIMIT {
        let mut receipt: Receipt = decode(&original).unwrap();
        receipt.reference.reference_id = id();
        h.db()
            .unwrap()
            .execute(
                "INSERT INTO policy_scope_recoveries VALUES(?1,?2)",
                params![receipt.reference.reference_id, encode(&receipt).unwrap()],
            )
            .unwrap();
    }
    let (next, _) = bound(&h, &core);
    lose(&h, &next);
    let inspection = h.recovery_inspect(&core, next.clone()).unwrap();
    assert_eq!(
        h.recovery_resolve(&core, inspection.reference.unwrap(), Mode::Native {})
            .unwrap_err()
            .code,
        "resource_limit"
    );
    assert!(
        h.control(&next.store_id, &next.service_id)
            .unwrap()
            .is_none()
    );
    assert!(
        !h.root
            .join(marker_name(&next.store_id, &next.service_id).unwrap())
            .exists()
    );
    assert_eq!(
        h.recovery_resolve(&core, reference.clone(), Mode::Native {})
            .unwrap(),
        revision
    );
    let count: u32 = h
        .db()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM policy_scope_recoveries", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(count, 256);
    let unchanged: String = h
        .db()
        .unwrap()
        .query_row(
            "SELECT record FROM policy_scope_recoveries WHERE id=?1",
            [&reference.reference_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(unchanged, original);
}
