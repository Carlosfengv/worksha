use super::*;

#[test]
fn expired_cooldowns_survive_unrelated_writes_and_a_stale_probe_result() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    store
        .service_set_cooldown("original", now - 2000, "upstream_rate_limit")
        .unwrap();
    store
        .service_set_cooldown("other", now + 30_000, "upstream_failure")
        .unwrap();
    assert!(store.service_cooldown_probe("original").unwrap());
    store
        .service_set_cooldown("original", now - 100, "upstream_failure")
        .unwrap();
    drop(store);

    let store = Store::open(root.path()).unwrap();
    store
        .service_recover_cooldowns(&["original"], now - 1000)
        .unwrap();
    assert!(store.service_cooldown_probe("original").unwrap());
    store.service_recover_cooldowns(&["original"], now).unwrap();
    assert!(!store.service_cooldown_probe("original").unwrap());
    assert!(store.service_cooldown("other").unwrap().is_some());
}

#[test]
fn recovery_requires_an_expired_marker_and_no_active_restriction_for_the_target() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let target = ["account", "model", "access-account", "access-model"];
    let pending = |now| store.service_recovery_pending(target, now).unwrap();
    assert!(!pending(1000));
    store
        .service_set_cooldown("model", 1000, "upstream_failure")
        .unwrap();
    store
        .service_set_cooldown("other", 9000, "upstream_rate_limit")
        .unwrap();
    assert!(!pending(999));
    assert!(pending(1000));
    store
        .service_set_cooldown("access-account", 2000, "upstream_credential_rejected")
        .unwrap();
    assert!(!pending(1000));
    assert!(pending(2000));
}

use crate::routing::{ALIAS, OpenCodeRouteProof, Selection, Turn, UncommittedDispatch};
fn source(store: &Store, second: bool) -> Provider {
    let mut p = store.providers().unwrap().remove(0);
    if second {
        p.id = format!("custom-{}", uuid::Uuid::new_v4());
        p.revision = 0;
    }
    p.name = if second { "Source B" } else { "Source A" }.into();
    p.endpoint = "http://127.0.0.1:32101".into();
    p.configured = true;
    p.protocol = "messages".into();
    p.model = "a".into();
    p.models = vec!["a".into(), "b".into()];
    p.disabled_models = vec![];
    p.credential_ref = None;
    p.model_catalog = None;
    let id = p.id.clone();
    store.save_provider(p).unwrap();
    store
        .providers()
        .unwrap()
        .into_iter()
        .find(|p| p.id == id)
        .unwrap()
}
pub(super) fn draft(store: &Store) -> ServiceDraft {
    let a = source(store, false);
    let b = source(store, true);
    let models = [(&a, "a"), (&b, "b")]
        .into_iter()
        .map(|(p, m)| ModelDraft {
            id: uuid::Uuid::new_v4().to_string(),
            name: m.into(),
            provider_id: p.id.clone(),
            model: m.into(),
            child: None,
            capabilities: Capabilities::default(),
        })
        .collect::<Vec<_>>();
    ServiceDraft {
        id: uuid::Uuid::new_v4().to_string(),
        revision: 0,
        name: "Coding".into(),
        default_model_id: models[0].id.clone(),
        models,
        routing_policy: RoutingPolicy::default(),
    }
}

#[test]
fn opencode_old_title_recovery_preserves_unresolved_dispatches_across_turns() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let mut config = draft(&store);
    config.routing_policy.enabled = true;
    let service = store.save_model_service(config).unwrap();
    let (binding, _) = store
        .create_model_service_binding(
            "opencode",
            "scope",
            &uuid::Uuid::new_v4().to_string(),
            &service,
        )
        .unwrap();
    for (session, previous_untrusted, pending) in [
        ("title", false, false),
        ("poisoned", true, false),
        ("pending", false, true),
    ] {
        let turn = |chain: &[&str]| Turn {
            session: Some(session.into()),
            digest: chain.join(":"),
            chain: chain.iter().map(|text| (*text).into()).collect(),
            continuation: false,
            portable: true,
        };
        let admit = |turn: &Turn, proof: Option<&OpenCodeRouteProof>, output| {
            store
                .gateway_admit(&binding.id, turn, &[], proof, Some(ALIAS), Some(output))
                .map(|(selected, _)| selected)
        };
        let dispatch = |turn: &Turn, state| {
            store.connection.execute(
                "INSERT INTO service_route_dispatch(binding,session,turn,owner,state,value) VALUES(?1,?2,?3,?4,?5,'{}')",
                params![binding.id,session,turn.digest,uuid::Uuid::new_v4().to_string(),state],
            ).unwrap();
        };
        let first = turn(&["first"]);
        assert!(admit(&first, None, 100).unwrap().fresh);
        dispatch(&first, "committed");
        let title = turn(&["title"]);
        assert_eq!(
            admit(&title, None, 32_000).err().unwrap().code,
            "invalid_output_limit"
        );
        store.connection.execute(
            "UPDATE gateway_route_leases SET output_validated=1 WHERE binding=?1 AND session=?2 AND turn=?3",
            params![binding.id,session,title.digest],
        ).unwrap();
        if pending {
            dispatch(&title, "pending");
        }
        let mut proof = OpenCodeRouteProof {
            prefixes: [(first.digest, 1)].into(),
            legacy_title_turns: vec![title.digest.clone()],
        };
        let second = turn(&["first", "second"]);
        if previous_untrusted {
            assert!(!admit(&second, None, 100).unwrap().fresh);
        } else {
            assert_eq!(admit(&second, Some(&proof), 100).unwrap().fresh, !pending);
        }
        if previous_untrusted || pending {
            proof.prefixes.insert(second.digest, 2);
            let third = turn(&["first", "second", "third"]);
            assert_eq!(admit(&third, Some(&proof), 100).unwrap().fresh, !pending);
        }
        let unchanged: (bool, String) = store.connection.query_row(
            "SELECT trusted,reason FROM gateway_route_leases WHERE binding=?1 AND session=?2 AND turn=?3",
            params![binding.id,session,title.digest], |row| Ok((row.get(0)?,row.get(1)?)),
        ).unwrap();
        assert_eq!(unchanged, (false, "session_boundary_unconfirmed".into()));
    }
}
pub(super) fn attach(store: &Store, service: &ModelService) -> ServiceAttachment {
    let b = store
        .dynamic_create(
            "claude",
            "scope",
            &uuid::Uuid::new_v4().to_string(),
            &service.models[0].provider,
        )
        .unwrap();
    store.attach_model_service(&b.id, service).unwrap()
}
fn turn(session: &str, chain: &[&str], continuation: bool) -> Turn {
    Turn {
        session: Some(session.into()),
        digest: chain.join(":"),
        chain: chain.iter().map(|v| (*v).into()).collect(),
        continuation,
        portable: true,
    }
}

#[test]
fn policy_commits_fallback_without_changing_preference_and_retains_pending_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let mut d = draft(&store);
    d.routing_policy.enabled = true;
    d.routing_policy.order = d.models.iter().map(|m| m.id.clone()).collect();
    let preferred = d.models[1].id.clone();
    let service = store.save_model_service(d).unwrap();
    let a = attach(&store, &service);
    let t = turn("policy", &["one"], false);
    let selected = store
        .model_service_admit(&a.binding_id, &t, &format!("local-agent-model-{preferred}"))
        .unwrap();
    let plan = store
        .service_dispatch(
            &a.binding_id,
            &t,
            &selected,
            |_| Default::default(),
            |_, _, _| Default::default(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(plan.candidates.len(), 2);
    assert_eq!(
        plan.candidates[0].service.as_ref().unwrap().model_id,
        preferred
    );
    let mut fallback = plan.candidates[1].clone();
    let choice = fallback.service.as_mut().unwrap();
    choice.attempt = 2;
    choice.fallback_reason = Some("upstream_failure".into());
    assert_eq!(
        choice.preferred_model_id.as_deref(),
        Some(preferred.as_str())
    );
    let attempt = store.service_attempt(&plan, &fallback).unwrap();
    store.service_attempt_begin_send(&plan, attempt).unwrap();
    assert_eq!(
        store
            .service_dispatch(
                &a.binding_id,
                &t,
                &selected,
                |_| Default::default(),
                |_, _, _| Default::default()
            )
            .unwrap_err()
            .code,
        "dispatch_uncertain"
    );
    store
        .service_attempt_result(attempt, 200, "committed")
        .unwrap();
    store.service_dispatch_finish(&plan, true).unwrap();
    let continuation = turn("policy", &["one"], true);
    let pinned = store
        .model_service_admit(&a.binding_id, &continuation, ALIAS)
        .unwrap();
    assert_eq!(pinned.version.id, fallback.version.id);
    assert!(!pinned.fresh);
    assert!(
        store
            .service_dispatch(
                &a.binding_id,
                &continuation,
                &pinned,
                |_| Default::default(),
                |_, _, _| Default::default()
            )
            .unwrap()
            .is_none()
    );
    let next = turn("policy", &["one", "two"], false);
    let selected = store
        .model_service_admit(
            &a.binding_id,
            &next,
            &format!("local-agent-model-{preferred}"),
        )
        .unwrap();
    let plan = store
        .service_dispatch(
            &a.binding_id,
            &next,
            &selected,
            |_| Default::default(),
            |_, _, _| Default::default(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        plan.candidates[0].service.as_ref().unwrap().model_id,
        preferred
    );
    let attempt = store.service_attempt(&plan, &plan.candidates[0]).unwrap();
    store.service_attempt_begin_send(&plan, attempt).unwrap();
    drop(store);
    let reopened = Store::open(dir.path()).unwrap();
    let replay = reopened
        .model_service_admit(&a.binding_id, &next, ALIAS)
        .unwrap();
    assert_eq!(
        reopened
            .service_dispatch(
                &a.binding_id,
                &next,
                &replay,
                |_| Default::default(),
                |_, _, _| Default::default()
            )
            .unwrap_err()
            .code,
        "dispatch_uncertain"
    );
}

#[test]
fn v4_upgrade_saves_consistent_backup_and_defaults_old_policies_off() {
    let dir = tempfile::tempdir().unwrap();
    let directory = dir.path().canonicalize().unwrap();
    let store = Store::open(&directory).unwrap();
    let service = store.save_model_service(draft(&store)).unwrap();
    let attachment = attach(&store, &service);
    store
        .connection
        .execute(
            "UPDATE model_service_versions SET value=json_remove(value,'$.routingPolicy')",
            [],
        )
        .unwrap();
    store
        .connection
        .execute_batch(
            "DROP TABLE local_token_cancellations;DROP TABLE local_token_owners;DROP TABLE local_token_values;DROP TABLE local_token_scope;DROP TABLE native_import_operations;DROP TABLE native_import_scope;DROP TABLE credential_legacy_operations;DROP TABLE credential_legacy_scope;DROP TABLE credential_recovery_values;DROP TABLE credential_recovery_scope;",
        )
        .unwrap();
    store
        .connection
        .pragma_update(None, "user_version", 4)
        .unwrap();
    drop(store);
    let store = Store::open(&directory).unwrap();
    assert!(
        !store
            .model_service(&service.id)
            .unwrap()
            .routing_policy
            .enabled
    );
    assert!(
        store
            .model_service_attachment(&attachment.binding_id)
            .unwrap()
            .is_some()
    );
    let backup =
        Connection::open(directory.join("local-agent.pre-routing-policy-v4.sqlite3")).unwrap();
    assert_eq!(
        backup
            .query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        4
    );
    assert_eq!(
        backup
            .query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
}

#[test]
fn policy_excludes_smaller_candidates_and_unknown_sessions_do_not_start_dispatches() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let mut d = draft(&store);
    d.routing_policy.enabled = true;
    let original = store.save_model_service(d.clone()).unwrap();
    let attachment = attach(&store, &original);
    d.revision = original.revision;
    d.models[1].capabilities.context_window = 16000;
    store.save_model_service(d).unwrap();
    let t = turn("capability", &["one"], false);
    let selected = store
        .model_service_admit(&attachment.binding_id, &t, ALIAS)
        .unwrap();
    let plan = store
        .service_dispatch(
            &attachment.binding_id,
            &t,
            &selected,
            |_| Default::default(),
            |_, _, _| Default::default(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(plan.candidates.len(), 1);
    let mut unknown = turn("unknown", &["one"], false);
    unknown.session = None;
    let selected = store
        .model_service_admit(&attachment.binding_id, &unknown, ALIAS)
        .unwrap();
    assert!(!selected.fresh);
    assert!(
        store
            .service_dispatch(
                &attachment.binding_id,
                &unknown,
                &selected,
                |_| Default::default(),
                |_, _, _| Default::default()
            )
            .unwrap()
            .is_none()
    );
}

#[test]
fn crud_cross_provider_versions_and_stale_editor_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let mut d = draft(&store);
    let first = store.save_model_service(d.clone()).unwrap();
    assert_ne!(first.models[0].provider.id, first.models[1].provider.id);
    assert_eq!(store.model_services().unwrap().len(), 1);
    assert_eq!(
        store.save_model_service(d.clone()).unwrap_err().code,
        "conflict"
    );
    d.revision = 1;
    d.name = "Updated".into();
    let second = store.save_model_service(d).unwrap();
    assert_eq!(second.revision, 2);
    assert_eq!(
        version(&store.connection, &first.version_id).unwrap().name,
        "Coding"
    );
    assert!(store.delete_model_service(&second.id, 1, &[]).is_err());
    store.delete_model_service(&second.id, 2, &[]).unwrap();
    assert!(store.model_services().unwrap().is_empty());
    assert!(version(&store.connection, &first.version_id).is_ok());
}
#[test]
fn default_and_specified_models_share_a_durable_turn_lease() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let mut d = draft(&store);
    let s = store.save_model_service(d.clone()).unwrap();
    let a = attach(&store, &s);
    let first = store
        .model_service_admit(&a.binding_id, &turn("s", &["one"], false), ALIAS)
        .unwrap();
    assert_eq!(first.version.provider.model, "a");
    let continuation = store
        .model_service_admit(
            &a.binding_id,
            &turn("s", &["one"], true),
            &s.models[1].alias(),
        )
        .unwrap();
    assert_eq!(first.version.id, continuation.version.id);
    assert_eq!(continuation.service.unwrap().mode, "default");
    let next = store
        .model_service_admit(
            &a.binding_id,
            &turn("s", &["one", "two"], false),
            &s.models[1].alias(),
        )
        .unwrap();
    assert_eq!(next.version.provider.model, "b");
    assert_eq!(next.service.as_ref().unwrap().mode, "specified");
    d.revision = 1;
    d.default_model_id = d.models[1].id.clone();
    store.save_model_service(d).unwrap();
    assert_eq!(
        store
            .model_service_admit(&a.binding_id, &turn("new", &["hello"], false), ALIAS)
            .unwrap()
            .version
            .provider
            .model,
        "b"
    );
    assert_eq!(
        store
            .model_service_admit(
                &a.binding_id,
                &turn("explicit", &["hello"], false),
                &s.models[0].alias()
            )
            .unwrap()
            .version
            .provider
            .model,
        "a"
    );
    drop(store);
    let reopened = Store::open(dir.path()).unwrap();
    let replay = reopened
        .model_service_admit(&a.binding_id, &turn("s", &["one", "two"], true), ALIAS)
        .unwrap();
    assert_eq!(replay.version.id, next.version.id);
    assert_eq!(replay.service.unwrap().model_id, s.models[1].id);
}
#[test]
fn deleted_service_keeps_replay_but_rejects_new_work_and_unknown_models() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let s = store.save_model_service(draft(&store)).unwrap();
    let a = attach(&store, &s);
    let first = store
        .model_service_admit(&a.binding_id, &turn("s", &["one"], false), ALIAS)
        .unwrap();
    assert_eq!(
        store
            .delete_model_service(&s.id, 1, std::slice::from_ref(&a.binding_id))
            .unwrap_err()
            .code,
        "in_use"
    );
    store.delete_model_service(&s.id, 1, &[]).unwrap();
    assert_eq!(
        store
            .model_service_admit(&a.binding_id, &turn("s", &["one"], true), ALIAS)
            .unwrap()
            .version
            .id,
        first.version.id
    );
    assert!(
        store
            .model_service_admit(&a.binding_id, &turn("new", &["one"], false), ALIAS)
            .is_err()
    );
    assert_eq!(
        store
            .model_service_admit(&a.binding_id, &turn("s", &["one"], true), "anything")
            .unwrap_err()
            .code,
        "invalid_model"
    );
}
#[test]
fn installed_contract_prevents_capability_downgrade_and_unpublished_selection() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let mut d = draft(&store);
    let first = store.save_model_service(d.clone()).unwrap();
    let a = attach(&store, &first);
    d.revision = 1;
    d.models[0].capabilities.context_window = 16_000;
    let extra = ModelDraft {
        id: uuid::Uuid::new_v4().to_string(),
        ..d.models[1].clone()
    };
    let alias = format!("local-agent-model-{}", extra.id);
    d.models.push(extra);
    store.save_model_service(d).unwrap();
    assert_eq!(
        store
            .model_service_admit(&a.binding_id, &turn("s", &["one"], false), ALIAS)
            .unwrap_err()
            .code,
        "catalog_sync_required"
    );
    assert_eq!(
        store
            .model_service_admit(&a.binding_id, &turn("s", &["one"], false), &alias)
            .unwrap_err()
            .code,
        "invalid_model"
    );
    assert_eq!(
        store
            .model_service_admit(
                &a.binding_id,
                &turn("s", &["one"], false),
                &first.models[1].alias()
            )
            .unwrap()
            .version
            .provider
            .model,
        "b"
    );
}
#[test]
fn provider_edits_do_not_rewrite_published_service_targets() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let s = store.save_model_service(draft(&store)).unwrap();
    let a = attach(&store, &s);
    let mut p = s.models[0].provider.clone();
    p.endpoint = "http://127.0.0.1:32102".into();
    store.save_provider(p).unwrap();
    let selected = store
        .model_service_admit(&a.binding_id, &turn("s", &["one"], false), ALIAS)
        .unwrap();
    assert_eq!(selected.version.provider.endpoint, "http://127.0.0.1:32101");
}
#[test]
fn malformed_models_and_unsupported_capabilities_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let d = draft(&store);
    for kind in 0..5 {
        let mut invalid = d.clone();
        match kind {
            0 => invalid.models[0].capabilities.thinking = true,
            1 => invalid.models[0].capabilities.input_modalities = vec!["image".into()],
            2 => invalid.models[0].capabilities.max_output_tokens = 32000,
            3 => invalid.models[1].id = invalid.models[0].id.clone(),
            _ => invalid.default_model_id = "missing".into(),
        };
        assert!(store.save_model_service(invalid).is_err());
    }
    assert!(store.model_services().unwrap().is_empty());
}
#[test]
fn v3_upgrade_preserves_consistent_backup_and_existing_route() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let s = store.save_model_service(draft(&store)).unwrap();
    let a = attach(&store, &s);
    store
        .connection
        .execute_batch("DROP TABLE local_token_cancellations;DROP TABLE local_token_owners;DROP TABLE local_token_values;DROP TABLE local_token_scope;DROP TABLE native_import_operations;DROP TABLE native_import_scope;DROP TABLE credential_legacy_operations;DROP TABLE credential_legacy_scope;DROP TABLE credential_recovery_values;DROP TABLE credential_recovery_scope;PRAGMA user_version=3;")
        .unwrap();
    drop(store);
    let reopened = Store::open(&dir.path().canonicalize().unwrap()).unwrap();
    let backup =
        Connection::open(dir.path().join("local-agent.pre-model-services-v3.sqlite3")).unwrap();
    assert_eq!(
        backup
            .query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        3
    );
    assert!(reopened.dynamic_binding(&a.binding_id).unwrap().is_some());
    assert_eq!(
        reopened
            .connection
            .query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        Store::SCHEMA_VERSION
    );
}

#[test]
fn intent_claim_is_unique_per_turn_and_reopens_with_scope_and_outcome() {
    use policy::{IntentClassifier, IntentOutcome, IntentReceipt, IntentTurn};

    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let mut draft = draft(&store);
    draft.routing_policy.enabled = true;
    draft.routing_policy.intent_classifier = Some(IntentClassifier {
        model_id: draft.default_model_id.clone(),
        account: None,
    });
    let service = store.save_model_service(draft).unwrap();
    let binding = attach(&store, &service).binding_id;
    let root = &service.version_id;
    let child = format!("{root}/child");
    let admit = |store: &Store, session: &str, alias: &str| {
        let turn = turn(session, &["turn"], false);
        store.model_service_admit(&binding, &turn, alias).unwrap();
        IntentTurn {
            binding: binding.clone(),
            session: session.into(),
            digest: turn.digest,
        }
    };
    let eligible = admit(&store, "eligible", ALIAS);
    let old = admit(&store, "old", ALIAS);
    store
        .connection
        .execute(
            "DELETE FROM service_route_intents WHERE binding=?1 AND session=?2",
            params![binding, old.session],
        )
        .unwrap();
    let explicit = admit(&store, "explicit", &service.models[0].alias());
    assert!(matches!(
        store.service_intent_outcome(&eligible, root).unwrap(),
        IntentReceipt::Unused
    ));
    assert!(store.service_claim_intent(&eligible, root).unwrap());
    assert!(!store.service_claim_intent(&eligible, &child).unwrap());
    drop(store);

    let store = Store::open(dir.path()).unwrap();
    assert!(matches!(
        store.service_intent_outcome(&eligible, root).unwrap(),
        IntentReceipt::Recorded(IntentOutcome::Unavailable)
    ));
    assert!(matches!(
        store.service_intent_outcome(&eligible, &child).unwrap(),
        IntentReceipt::Unavailable
    ));
    assert!(
        store
            .service_complete_intent(&eligible, &child, &IntentOutcome::Unknown)
            .is_err()
    );
    let label = "排查错误";
    store
        .service_complete_intent(&eligible, root, &IntentOutcome::Matched(label.into()))
        .unwrap();
    assert!(
        store
            .service_complete_intent(&eligible, root, &IntentOutcome::Unknown)
            .is_err()
    );
    drop(store);

    let store = Store::open(dir.path()).unwrap();
    assert!(
        matches!(store.service_intent_outcome(&eligible, root).unwrap(), IntentReceipt::Recorded(IntentOutcome::Matched(saved)) if saved == label)
    );
    for no_claim in [&old, &explicit] {
        assert!(matches!(
            store.service_intent_outcome(no_claim, root).unwrap(),
            IntentReceipt::Unavailable
        ));
        assert!(!store.service_claim_intent(no_claim, root).unwrap());
    }
}

#[test]
fn prepared_turn_resumes_after_reopen_and_waits_for_a_committed_target() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let mut draft = draft(&store);
    draft.routing_policy.enabled = true;
    let service = store.save_model_service(draft).unwrap();
    let attachment = attach(&store, &service);
    let binding = &attachment.binding_id;
    let first = turn("prepared", &["original"], false);
    let selected = store.model_service_admit(binding, &first, ALIAS).unwrap();
    assert!(selected.fresh);
    assert_eq!(selected.reason, "new_session");

    // A legacy lease without dispatch evidence could have been sent by an older binary.
    let legacy = turn("legacy-preparation", &["original"], false);
    store.model_service_admit(binding, &legacy, ALIAS).unwrap();
    store
        .connection
        .execute(
            "UPDATE gateway_route_leases SET reason='new_session' WHERE binding=?1 AND session=?2",
            params![binding, legacy.session],
        )
        .unwrap();
    drop(store);

    let store = Store::open(dir.path()).unwrap();
    let alternate = format!("local-agent-model-{}", service.models[1].id);
    let continuation = Turn {
        continuation: true,
        ..first.clone()
    };
    for early in [&continuation, &legacy] {
        let selection = store
            .model_service_admit(binding, early, &alternate)
            .unwrap();
        assert_eq!(
            selection.uncommitted_dispatch,
            Some(UncommittedDispatch::NoCommittedTarget)
        );
        let error = store
            .service_dispatch(
                binding,
                early,
                &selection,
                |_| Default::default(),
                |_, _, _| Default::default(),
            )
            .unwrap_err();
        assert_eq!(error.code, "dispatch_not_committed");
    }

    let retry = store
        .model_service_admit(binding, &first, &alternate)
        .unwrap();
    assert!(retry.fresh);
    assert_eq!(retry.reason, "new_session");
    assert_eq!(retry.version.id, selected.version.id);
    assert_eq!(retry.service.as_ref().unwrap().requested_alias, ALIAS);
    let plan = store
        .service_dispatch(
            binding,
            &first,
            &retry,
            |_| Default::default(),
            |_, _, _| Default::default(),
        )
        .unwrap()
        .unwrap();
    let prepared = store.service_attempt(&plan, &plan.candidates[0]).unwrap();
    drop(store);

    let store = Store::open(dir.path()).unwrap();
    let early = store
        .model_service_admit(binding, &continuation, ALIAS)
        .unwrap();
    assert_eq!(
        store
            .service_dispatch(
                binding,
                &continuation,
                &early,
                |_| Default::default(),
                |_, _, _| Default::default(),
            )
            .unwrap_err()
            .code,
        "dispatch_uncertain"
    );
    let resumed = store
        .model_service_admit(binding, &first, &alternate)
        .unwrap();
    let resumed_plan = store
        .service_dispatch(
            binding,
            &first,
            &resumed,
            |_| Default::default(),
            |_, _, _| Default::default(),
        )
        .unwrap()
        .unwrap();
    assert_ne!(resumed_plan.owner, plan.owner);
    assert_eq!(
        resumed_plan.candidates[0].version.id,
        plan.candidates[0].version.id
    );
    assert_eq!(
        store
            .service_attempt_begin_send(&plan, prepared)
            .unwrap_err()
            .code,
        "conflict"
    );
    let outcome = store
        .connection
        .query_row(
            "SELECT outcome FROM service_route_attempts WHERE id=?1",
            [prepared],
            |row| row.get::<_, String>(0),
        )
        .unwrap();
    assert_eq!(outcome, "not_sent");
    let plan = resumed_plan;
    let winner = &plan.candidates[1];
    let attempt = store.service_attempt(&plan, winner).unwrap();
    store.service_attempt_begin_send(&plan, attempt).unwrap();
    store
        .service_attempt_result(attempt, 200, "committed")
        .unwrap();
    store.service_dispatch_finish(&plan, true).unwrap();
    let stale = store
        .service_dispatch(
            binding,
            &first,
            &retry,
            |_| Default::default(),
            |_, _, _| Default::default(),
        )
        .unwrap_err();
    assert_eq!(stale.code, "dispatch_already_committed");
    let pinned = store
        .model_service_admit(binding, &continuation, ALIAS)
        .unwrap();
    assert!(!pinned.fresh);
    assert_eq!(pinned.version.id, winner.version.id);
    assert!(
        store
            .service_dispatch(
                binding,
                &continuation,
                &pinned,
                |_| Default::default(),
                |_, _, _| Default::default(),
            )
            .unwrap()
            .is_none()
    );
}

#[test]
fn fallback_checkpoint_rolls_back_and_only_covers_the_confirmed_send() {
    for kind in [
        CheckpointKind::RateLimit,
        CheckpointKind::Budget,
        CheckpointKind::Balance,
    ] {
        check_fallback_checkpoint(kind);
    }
}

enum CheckpointKind {
    RateLimit,
    Budget,
    Balance,
}

fn check_fallback_checkpoint(kind: CheckpointKind) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let mut draft = draft(&store);
    draft.routing_policy.enabled = true;
    let service = store.save_model_service(draft).unwrap();
    let binding = attach(&store, &service).binding_id;
    let turn = turn("resume-fallback", &["turn"], false);
    let dispatch = |store: &Store| {
        let selected = store.model_service_admit(&binding, &turn, ALIAS).unwrap();
        store.service_dispatch(
            &binding,
            &turn,
            &selected,
            |_| Default::default(),
            |_, _, _| Default::default(),
        )
    };
    let plan = dispatch(&store).unwrap().unwrap();
    let first = store.service_attempt(&plan, &plan.candidates[0]).unwrap();
    store.service_attempt_begin_send(&plan, first).unwrap();
    let checkpoint = policy::FallbackCheckpoint {
        through: first,
        progress: serde_json::json!({"tried": 1}),
    };
    let budget_rejection = policy::OpenRouterBudgetRejection {
        credential: "fixture-account".into(),
        scope: policy::OpenRouterBudgetScope::KeyLimit,
        observed_at: chrono::Utc::now(),
        reset_at: None,
    };
    let balance_rejection = policy::BalanceRejection {
        credential: budget_rejection.credential.clone(),
        observed_at: budget_rejection.observed_at,
    };
    let (status, outcome, guarded) = match kind {
        CheckpointKind::RateLimit => (429, "rate_limited", "service_route_cooldowns"),
        CheckpointKind::Budget => (
            403,
            "upstream_quota_exhausted",
            "service_openrouter_reply_budget",
        ),
        CheckpointKind::Balance => (402, "upstream_quota_exhausted", "service_upstream_balance"),
    };
    let record = || {
        let admission = match kind {
            CheckpointKind::RateLimit => policy::FailedAttemptAdmission::Cooldown(
                "fixture-cooldown",
                chrono::Utc::now().timestamp_millis() + 1000,
                "upstream_rate_limit",
            ),
            CheckpointKind::Budget => {
                policy::FailedAttemptAdmission::OpenRouterBudget(&budget_rejection)
            }
            CheckpointKind::Balance => policy::FailedAttemptAdmission::Balance(&balance_rejection),
        };
        store.service_record_failed_attempt(
            &plan,
            first,
            status,
            outcome,
            Some(&checkpoint),
            admission,
        )
    };
    store.connection.execute_batch(&format!("CREATE TRIGGER fixture_block_admission BEFORE INSERT ON {guarded} BEGIN SELECT RAISE(FAIL,'fixture'); END;")).unwrap();
    assert!(record().is_err());
    let persisted = store
        .connection
        .query_row(
            "SELECT json_extract(value,'$.fallback') FROM service_route_dispatch WHERE binding=?1",
            [&binding],
            |row| row.get::<_, Option<String>>(0),
        )
        .unwrap();
    assert!(persisted.is_none());
    assert_eq!(dispatch(&store).unwrap_err().code, "dispatch_uncertain");
    store
        .connection
        .execute_batch("DROP TRIGGER fixture_block_admission")
        .unwrap();
    record().unwrap();
    let prepared = store.service_attempt(&plan, &plan.candidates[1]).unwrap();
    drop(store);

    let store = Store::open(dir.path()).unwrap();
    match kind {
        CheckpointKind::Budget => assert_eq!(
            store.openrouter_reply_holds("fixture-account").unwrap(),
            [("key_limit".into(), None)]
        ),
        CheckpointKind::Balance => assert_eq!(
            store.upstream_balance_state("fixture-account").unwrap(),
            policy::BalanceState::Exhausted
        ),
        CheckpointKind::RateLimit => (),
    }
    let recovered = dispatch(&store).unwrap().unwrap();
    assert_ne!(recovered.owner, plan.owner);
    assert_eq!(recovered.fallback.as_ref().unwrap().through, first);
    assert!(store.service_attempt_begin_send(&plan, prepared).is_err());
    let backup = store
        .service_attempt(&recovered, &recovered.candidates[1])
        .unwrap();
    store
        .service_attempt_begin_send(&recovered, backup)
        .unwrap();
    drop(store);

    let store = Store::open(dir.path()).unwrap();
    assert_eq!(dispatch(&store).unwrap_err().code, "dispatch_uncertain");
}

#[test]
fn inherited_history_waits_for_dispatch_and_rejects_conflicting_legacy_targets() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let mut draft = draft(&store);
    draft.routing_policy.enabled = true;
    let service = store.save_model_service(draft).unwrap();
    let attachment = attach(&store, &service);
    let binding = &attachment.binding_id;
    let original = turn("inheritance", &["original"], false);
    let compacted = turn("inheritance", &["compacted"], true);
    let changed_again = turn("inheritance", &["edited", "history"], false);
    let admit =
        |store: &Store, turn: &Turn| store.model_service_admit(binding, turn, ALIAS).unwrap();
    let dispatch = |store: &Store, turn: &Turn, selected: &Selection| {
        store.service_dispatch(
            binding,
            turn,
            selected,
            |_| Default::default(),
            |_, _, _| Default::default(),
        )
    };
    let selected = admit(&store, &original);
    let assert_hold = |store: &Store, turn, block, code| {
        let selected = admit(store, turn);
        assert_eq!(selected.uncommitted_dispatch, Some(block));
        assert_eq!(dispatch(store, turn, &selected).unwrap_err().code, code);
    };
    assert_hold(
        &store,
        &compacted,
        UncommittedDispatch::NoCommittedTarget,
        "dispatch_not_committed",
    );
    let lease_count = |store: &Store| {
        store
            .connection
            .query_row(
                "SELECT count(*) FROM gateway_route_leases WHERE binding=?1",
                [binding],
                |row| row.get::<_, u32>(0),
            )
            .unwrap()
    };
    assert_eq!(lease_count(&store), 1);
    let plan = dispatch(&store, &original, &selected).unwrap().unwrap();
    let first = store.service_attempt(&plan, &plan.candidates[0]).unwrap();
    store
        .service_attempt_result(first, 502, "outcome_unknown")
        .unwrap();

    let seed_alias = |store: &Store, historical: &Turn| {
        store.connection.execute(
            "INSERT INTO gateway_route_leases(binding,session,turn,version,chain,trusted,reason,service_selection)
             SELECT binding,session,?2,version,?3,0,'session_boundary_unconfirmed',service_selection
             FROM gateway_route_leases WHERE binding=?1 AND turn=?4",
            params![binding, historical.digest, serde_json::to_string(&historical.chain).unwrap(), original.digest],
        ).unwrap();
    };
    // A previous version could write this alias while the original was pending.
    seed_alias(&store, &compacted);
    drop(store);
    let store = Store::open(dir.path()).unwrap();
    for history in [&compacted, &changed_again] {
        assert_hold(
            &store,
            history,
            UncommittedDispatch::Pending,
            "dispatch_uncertain",
        );
    }
    assert_eq!(lease_count(&store), 2);

    store
        .service_attempt_result(first, 400, "rejected")
        .unwrap();
    store.service_dispatch_finish(&plan, false).unwrap();
    assert_hold(
        &store,
        &compacted,
        UncommittedDispatch::NoCommittedTarget,
        "dispatch_not_committed",
    );
    store
        .connection
        .execute(
            "UPDATE service_route_dispatch SET retry_at=0 WHERE binding=?1 AND turn=?2",
            params![binding, original.digest],
        )
        .unwrap();
    let retry = dispatch(&store, &original, &admit(&store, &original))
        .unwrap()
        .unwrap();
    let winner = &retry.candidates[1];
    let final_attempt = store.service_attempt(&retry, winner).unwrap();
    store
        .service_attempt_result(final_attempt, 200, "committed")
        .unwrap();
    store.service_dispatch_finish(&retry, true).unwrap();
    for history in [&compacted, &changed_again] {
        assert_hold(
            &store,
            history,
            UncommittedDispatch::NoCommittedTarget,
            "dispatch_not_committed",
        );
    }
    assert_eq!(lease_count(&store), 2);
    let mut conflicting_account = winner.service.clone().unwrap();
    conflicting_account.upstream_account = Some(crate::providers::upstream_accounts::AccountRef {
        id: uuid::Uuid::new_v4().to_string(),
        revision: 1,
    });
    store.connection.execute(
        "UPDATE gateway_route_leases SET version=?3,service_selection=?4 WHERE binding=?1 AND turn=?2",
        params![binding, compacted.digest, winner.version.id, serde_json::to_string(&conflicting_account).unwrap()],
    ).unwrap();
    assert_hold(
        &store,
        &compacted,
        UncommittedDispatch::NoCommittedTarget,
        "dispatch_not_committed",
    );
    let compatible = turn("inheritance", &["committed history"], true);
    seed_alias(&store, &compatible);
    let selected = admit(&store, &compatible);
    assert_eq!(selected.uncommitted_dispatch, None);
    assert_eq!(selected.version.id, winner.version.id);
    assert!(dispatch(&store, &compatible, &selected).unwrap().is_none());
    let continuation = Turn {
        continuation: true,
        ..original
    };
    let selected = admit(&store, &continuation);
    assert_eq!(selected.uncommitted_dispatch, None);
    assert_eq!(selected.version.id, winner.version.id);
    assert!(
        dispatch(&store, &continuation, &selected)
            .unwrap()
            .is_none()
    );
}

#[test]
fn route_notices_persist_once_per_turn_recover_and_do_not_rewrite_preference() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let mut d = draft(&store);
    d.routing_policy.enabled = true;
    let service = store.save_model_service(d).unwrap();
    let attachment = attach(&store, &service);
    let binding = &attachment.binding_id;
    let t = turn("notices", &["one"], false);
    let selected = store.model_service_admit(binding, &t, ALIAS).unwrap();
    assert!(
        store
            .model_service_route_notice(binding, &t, &selected, true)
            .unwrap()
            .is_none()
    );
    let next = turn("notices", &["one", "two"], false);
    let selected = store.model_service_admit(binding, &next, ALIAS).unwrap();
    let plan = store
        .service_dispatch(
            binding,
            &next,
            &selected,
            |_| Default::default(),
            |_, _, _| Default::default(),
        )
        .unwrap()
        .unwrap();
    let mut fallback = plan.candidates[1].clone();
    fallback.fresh = true;
    fallback.service.as_mut().unwrap().fallback_reason = Some("upstream_rate_limit".into());
    let original_preference = fallback
        .service
        .as_ref()
        .unwrap()
        .preferred_model_id
        .clone();
    let notice = store
        .model_service_route_notice(binding, &next, &fallback, true)
        .unwrap()
        .unwrap();
    assert!(notice.contains("限流"));
    assert!(notice.contains("Source B / b"));
    let issued = store
        .model_service_notice_hashes(binding, "notices")
        .unwrap();
    assert_eq!(
        issued,
        vec![crate::providers::files::hash(notice.trim().as_bytes())]
    );
    assert!(
        store
            .model_service_notice_hashes(binding, "other-session")
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .model_service_route_notice(binding, &next, &fallback, true)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        fallback.service.as_ref().unwrap().preferred_model_id,
        original_preference
    );
    drop(store);
    let store = Store::open(dir.path()).unwrap();
    assert!(
        store
            .model_service_route_notice(binding, &next, &fallback, true)
            .unwrap()
            .is_none()
    );
    let repeated = turn("notices", &["one", "two", "three"], false);
    store
        .model_service_admit(binding, &repeated, ALIAS)
        .unwrap();
    let backup = fallback.service.as_mut().unwrap();
    backup.attempt = 2;
    assert_eq!(
        store
            .model_service_route_notice(binding, &repeated, &fallback, true)
            .unwrap()
            .as_deref(),
        Some(notice.as_str())
    );
    assert!(
        store
            .model_service_route_notice(binding, &repeated, &fallback, true)
            .unwrap()
            .is_none()
    );
    let cooling = turn("notices", &["one", "two", "three", "four"], false);
    store.model_service_admit(binding, &cooling, ALIAS).unwrap();
    let backup = fallback.service.as_mut().unwrap();
    backup.attempt = 1;
    backup.fallback_reason = Some("rate_limit_cooldown".into());
    assert!(
        store
            .model_service_route_notice(binding, &cooling, &fallback, true)
            .unwrap()
            .is_none()
    );

    let recovered = turn("notices", &["one", "two", "three", "four", "five"], false);
    let selected = store
        .model_service_admit(binding, &recovered, ALIAS)
        .unwrap();
    let notice = store
        .model_service_route_notice(binding, &recovered, &selected, true)
        .unwrap()
        .unwrap();
    assert!(notice.contains("已恢复使用 Source A / a"));
    assert!(
        store
            .model_service_route_notice(binding, &next, &fallback, true)
            .unwrap()
            .is_none()
    );
    let continuation = turn("notices", &["one", "two", "three", "four", "five"], true);
    let selected = store
        .model_service_admit(binding, &continuation, ALIAS)
        .unwrap();
    assert!(
        store
            .model_service_route_notice(binding, &continuation, &selected, true)
            .unwrap()
            .is_none()
    );
}

#[test]
fn route_notices_cover_disabled_policy_default_changes_and_suppressed_json_targets() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let mut d = draft(&store);
    let service = store.save_model_service(d.clone()).unwrap();
    let a = attach(&store, &service);
    let t = turn("json", &["one"], false);
    let selected = store.model_service_admit(&a.binding_id, &t, ALIAS).unwrap();
    assert!(
        store
            .model_service_route_notice(&a.binding_id, &t, &selected, true)
            .unwrap()
            .is_none()
    );
    d.revision = service.revision;
    d.default_model_id = d.models[1].id.clone();
    let service = store.save_model_service(d.clone()).unwrap();
    let t = turn("json", &["one", "two"], false);
    let selected = store.model_service_admit(&a.binding_id, &t, ALIAS).unwrap();
    assert!(
        store
            .model_service_route_notice(&a.binding_id, &t, &selected, false)
            .unwrap()
            .is_none()
    );
    let t = turn("json", &["one", "two", "three"], false);
    let selected = store.model_service_admit(&a.binding_id, &t, ALIAS).unwrap();
    assert!(
        store
            .model_service_route_notice(&a.binding_id, &t, &selected, true)
            .unwrap()
            .is_none()
    );
    d.revision = service.revision;
    d.default_model_id = d.models[0].id.clone();
    store.save_model_service(d).unwrap();
    let t = turn("json", &["one", "two", "three", "four"], false);
    let selected = store.model_service_admit(&a.binding_id, &t, ALIAS).unwrap();
    assert!(
        store
            .model_service_route_notice(&a.binding_id, &t, &selected, true)
            .unwrap()
            .unwrap()
            .contains("本轮已切换至 Source A")
    );
}

#[test]
fn route_notices_v5_upgrade_backs_up_history_and_preserves_selected_alias() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let service = store.save_model_service(draft(&store)).unwrap();
    let a = attach(&store, &service);
    store
        .connection
        .execute_batch("DROP TABLE local_token_cancellations;DROP TABLE local_token_owners;DROP TABLE local_token_values;DROP TABLE local_token_scope;DROP TABLE native_import_operations;DROP TABLE native_import_scope;DROP TABLE credential_legacy_operations;DROP TABLE credential_legacy_scope;DROP TABLE credential_recovery_values;DROP TABLE credential_recovery_scope;DROP TABLE service_route_notices;PRAGMA user_version=5;")
        .unwrap();
    drop(store);
    let store = Store::open(&dir.path().canonicalize().unwrap()).unwrap();
    let backup =
        Connection::open(dir.path().join("local-agent.pre-route-notices-v5.sqlite3")).unwrap();
    assert_eq!(
        backup
            .query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        5
    );
    assert_eq!(
        backup
            .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    assert!(
        store
            .model_service_attachment(&a.binding_id)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        store.model_service(&service.id).unwrap().default_model_id,
        service.default_model_id
    );
}
