use super::*;
use crate::routing::{ALIAS, Turn};
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
        .service_dispatch(&a.binding_id, &t, &selected)
        .unwrap()
        .unwrap();
    assert_eq!(plan.candidates.len(), 2);
    assert_eq!(
        plan.candidates[0].service.as_ref().unwrap().model_id,
        preferred
    );
    assert!(
        store
            .service_dispatch(&a.binding_id, &t, &selected)
            .is_err()
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
            .service_dispatch(&a.binding_id, &continuation, &pinned)
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
        .service_dispatch(&a.binding_id, &next, &selected)
        .unwrap()
        .unwrap();
    assert_eq!(
        plan.candidates[0].service.as_ref().unwrap().model_id,
        preferred
    );
    drop(store);
    let reopened = Store::open(dir.path()).unwrap();
    let replay = reopened
        .model_service_admit(&a.binding_id, &next, ALIAS)
        .unwrap();
    assert!(
        reopened
            .service_dispatch(&a.binding_id, &next, &replay)
            .is_err()
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
        .service_dispatch(&attachment.binding_id, &t, &selected)
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
            .service_dispatch(&attachment.binding_id, &unknown, &selected)
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
        .service_dispatch(binding, &next, &selected)
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
    let recovered = turn("notices", &["one", "two", "three"], false);
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
    let continuation = turn("notices", &["one", "two", "three"], true);
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
