use super::{runtime::Context, store::*};
use local_agent_core::plugin::policy as p;
fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn rank_for(service: &str, version: &str) -> local_agent_core::domain::routing_policy::RankRequest {
    use local_agent_core::domain::routing_policy::*;
    let model = id();
    RankRequest {
        service_id: service.into(),
        service_version_id: version.into(),
        preferred_model_id: None,
        default_model_id: Some(model.clone()),
        configured_order: vec![model.clone()],
        candidates: vec![Candidate {
            model_id: model,
            capabilities: CandidateCapabilities {
                context_window: 96000,
                max_output_tokens: 4096,
                tools: true,
                thinking: false,
                input_modalities: vec!["text".into()],
            },
            health: CandidateHealth::Unknown,
        }],
    }
}

#[tokio::test]
async fn policy_scope_interrupted_unbind_does_not_select_native_mode() {
    use super::super::Manager;
    use local_agent_core::providers::vault::MemoryVault;
    use std::sync::Arc;
    let root = tempfile::tempdir().unwrap();
    let manager = Arc::new(Manager::open(root.path(), Arc::new(MemoryVault::default())).unwrap());
    let store = id();
    let service = id();
    let version = id();
    {
        let h = manager.policy.lock().unwrap();
        let c = context();
        let bound = h
            .bind(
                BindRequest {
                    store: &store,
                    service: &service,
                    version: &version,
                    expected: &None,
                    package: &p::PackageQuery {
                        plugin_id: c.package.manifest.id.clone(),
                        digest: c.package.digest.clone(),
                    },
                    failure: p::FailureMode::Reject,
                    pending: None,
                },
                &Authorization::from_context(&c, "owned-instance"),
            )
            .unwrap();
        h.db().unwrap().execute_batch("CREATE TRIGGER reject_unbind BEFORE INSERT ON policy_controls BEGIN SELECT RAISE(FAIL,'owned unbind persistence failure'); END;").unwrap();
        assert!(h.unbind(&store, &service, &bound.status.revision).is_err());
        assert!(
            h.cached_control(&store, &service)
                .unwrap()
                .unwrap()
                .head
                .is_none()
        );
    }
    let result = manager
        .policy_rank(store.clone(), rank_for(&service, &version))
        .await;
    assert_eq!(
        result
            .err()
            .expect("interrupted unbind must not silently select native ordering")
            .code,
        "invalid_record"
    );
    {
        let h = manager.policy.lock().unwrap();
        h.db()
            .unwrap()
            .execute_batch("DROP TRIGGER reject_unbind;")
            .unwrap();
        let expected = h
            .cached_control(&store, &service)
            .unwrap()
            .unwrap()
            .revision;
        h.unbind(&store, &service, &expected).unwrap();
    }
    assert!(
        manager
            .policy_rank(store, rank_for(&service, &version))
            .await
            .unwrap()
            .order
            .is_none()
    );
}

#[test]
fn policy_scope_double_loss_preserves_unknown_from_surviving_version_binding() {
    let root = tempfile::tempdir().unwrap();
    let h = Host::new(root.path());
    let store = id();
    let service = id();
    let version = id();
    let c = context();
    h.bind(
        BindRequest {
            store: &store,
            service: &service,
            version: &version,
            expected: &None,
            package: &p::PackageQuery {
                plugin_id: c.package.manifest.id.clone(),
                digest: c.package.digest.clone(),
            },
            failure: p::FailureMode::Reject,
            pending: None,
        },
        &Authorization::from_context(&c, "owned-instance"),
    )
    .unwrap();
    h.db()
        .unwrap()
        .execute(
            "DELETE FROM policy_controls WHERE scope=?1",
            [Host::scope(&store, &service)],
        )
        .unwrap();
    std::fs::remove_file(marker_path(&h)).unwrap();
    drop(h);
    let before = std::fs::read(root.path().join("routing-policy.sqlite3")).unwrap();
    let h = Host::new(root.path());
    let presence = h.presence.read().unwrap();
    assert!(presence.unknown.contains(&service));
    assert!(!presence.unavailable);
    assert!(presence.scopes.is_empty());
    drop(presence);
    assert_eq!(
        h.consistent_control(&store, &service).unwrap_err().code,
        "invalid_record"
    );
    assert_eq!(
        std::fs::read(root.path().join("routing-policy.sqlite3")).unwrap(),
        before
    );
    assert!(
        !h.root
            .read_dir()
            .unwrap()
            .any(|r| r.unwrap().path().is_file())
    );
}

#[test]
fn policy_scope_explicit_unbound_control_is_not_replaced_by_historical_evidence() {
    let root = tempfile::tempdir().unwrap();
    let h = Host::new(root.path());
    let store = id();
    let service = id();
    let version = id();
    let c = context();
    let bound = h
        .bind(
            BindRequest {
                store: &store,
                service: &service,
                version: &version,
                expected: &None,
                package: &p::PackageQuery {
                    plugin_id: c.package.manifest.id.clone(),
                    digest: c.package.digest.clone(),
                },
                failure: p::FailureMode::Reject,
                pending: None,
            },
            &Authorization::from_context(&c, "owned-instance"),
        )
        .unwrap();
    let revision = h.unbind(&store, &service, &bound.status.revision).unwrap();
    std::fs::remove_file(marker_path(&h)).unwrap();
    drop(h);
    let h = Host::new(root.path());
    assert!(h.presence.read().unwrap().unknown.is_empty());
    let current = h.consistent_control(&store, &service).unwrap().unwrap();
    assert_eq!(current.revision, revision);
    assert!(current.head.is_none());
}

#[test]
fn policy_scope_double_loss_preserves_unresolved_publication_identity_without_binding() {
    let root = tempfile::tempdir().unwrap();
    let h = Host::new(root.path());
    let intent = prepared(&h);
    h.db()
        .unwrap()
        .execute("DELETE FROM policy_controls", [])
        .unwrap();
    std::fs::remove_file(marker_path(&h)).unwrap();
    drop(h);
    let h = Host::new(root.path());
    let presence = h.presence.read().unwrap();
    assert_eq!(presence.unknown.len(), 1);
    assert!(presence.unknown.contains(&intent.status.service_id));
    assert!(!presence.unavailable);
    drop(presence);
    let retained = h.intent(&intent.status.operation_id).unwrap();
    assert_eq!(
        encode(&retained.status.request).unwrap(),
        encode(&intent.status.request).unwrap()
    );
    assert_eq!(retained.status.state, "prepared");
}

#[tokio::test]
async fn policy_scope_missing_evidence_blocks_only_its_service_and_keeps_native_database_lazy() {
    use super::super::Manager;
    use local_agent_core::providers::vault::MemoryVault;
    use std::sync::Arc;
    let root = tempfile::tempdir().unwrap();
    let manager = Arc::new(Manager::open(root.path(), Arc::new(MemoryVault::default())).unwrap());
    let store = id();
    let service = id();
    let version = id();
    assert!(
        manager
            .policy_rank(store.clone(), rank_for(&service, &version))
            .await
            .unwrap()
            .order
            .is_none()
    );
    assert!(!root.path().join("routing-policy.sqlite3").exists());
    {
        let h = manager.policy.lock().unwrap();
        let c = context();
        h.bind(
            BindRequest {
                store: &store,
                service: &service,
                version: &version,
                expected: &None,
                package: &p::PackageQuery {
                    plugin_id: c.package.manifest.id.clone(),
                    digest: c.package.digest.clone(),
                },
                failure: p::FailureMode::Reject,
                pending: None,
            },
            &Authorization::from_context(&c, "owned-instance"),
        )
        .unwrap();
        h.db()
            .unwrap()
            .execute("DELETE FROM policy_controls", [])
            .unwrap();
        std::fs::remove_file(marker_path(&h)).unwrap();
    }
    drop(manager);
    let manager = Arc::new(Manager::open(root.path(), Arc::new(MemoryVault::default())).unwrap());
    assert_eq!(
        manager
            .policy_rank(store.clone(), rank_for(&service, &version))
            .await
            .err()
            .unwrap()
            .code,
        "invalid_record"
    );
    assert!(
        manager
            .policy_rank(store, rank_for(&id(), &id()))
            .await
            .unwrap()
            .order
            .is_none()
    );
}

#[test]
fn policy_scope_full_physical_registry_keeps_known_controls_and_rejects_only_new_scopes() {
    let root = tempfile::tempdir().unwrap();
    let h = Host::new(root.path());
    let store = id();
    let mut controls = vec![];
    for _ in 0..256 {
        let control = Control {
            store_id: store.clone(),
            service_id: id(),
            revision: id(),
            authority: id(),
            version: None,
            head: None,
            pending: None,
            pending_request_identity: None,
        };
        h.save_control(&control).unwrap();
        controls.push(control);
    }
    drop(h);
    let h = Host::new(root.path());
    assert_eq!(h.presence.read().unwrap().scopes.len(), 256);
    assert!(!h.presence.read().unwrap().unavailable);
    let mut replacement = controls[0].clone();
    replacement.revision = id();
    h.save_control(&replacement).unwrap();
    assert_eq!(
        h.consistent_control(&store, &replacement.service_id)
            .unwrap()
            .unwrap(),
        replacement
    );
    let mut foreign = replacement;
    foreign.service_id = id();
    assert_eq!(h.save_control(&foreign).unwrap_err().code, "resource_limit");
    assert_eq!(std::fs::read_dir(&h.root).unwrap().count(), 256);
    assert_eq!(
        h.db()
            .unwrap()
            .query_row("SELECT count(*) FROM policy_controls", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        256
    );
}
fn context() -> Context {
    let manifest: p::Manifest = serde_json::from_str(include_str!(
        "../../../../../../sdk/plugins/examples/pure-policy/package/manifest.json"
    ))
    .unwrap();
    Context {
        package: Installed {
            manifest,
            digest: "1".repeat(64),
            signing_key: "2".repeat(64),
            bytes: 100,
            expanded: 100,
            removed: false,
        },
        path: "unused".into(),
        before: Selection {
            digest: "1".repeat(64),
            previous: None,
            enabled: true,
            epoch: 2,
        },
        epoch: 2,
        publisher_revision: 3,
        preflight: false,
        binding: None,
    }
}

impl super::super::Manager {
    pub(crate) fn bind_unavailable_policy_fixture(
        &self,
        store: &str,
        service: &str,
        version: &str,
    ) -> String {
        let fixture = context();
        self.policy
            .lock()
            .unwrap()
            .bind(
                BindRequest {
                    store,
                    service,
                    version,
                    expected: &None,
                    package: &p::PackageQuery {
                        plugin_id: fixture.package.manifest.id.clone(),
                        digest: fixture.package.digest.clone(),
                    },
                    failure: p::FailureMode::Builtin,
                    pending: None,
                },
                &Authorization::from_context(&fixture, &self.catalog_instance),
            )
            .unwrap()
            .status
            .revision
    }

    pub(crate) fn unbind_policy_fixture(&self, store: &str, service: &str, revision: &str) {
        self.policy
            .lock()
            .unwrap()
            .unbind(store, service, revision)
            .unwrap();
    }
}

#[test]
fn version_bindings_keep_old_authority_until_explicit_unbind_and_restart_restores_no_activation() {
    let temp = tempfile::tempdir().unwrap();
    let h = Host::new(temp.path());
    let store = id();
    let service = id();
    let a = id();
    let b = id();
    let c = context();
    let q = p::PackageQuery {
        plugin_id: c.package.manifest.id.clone(),
        digest: c.package.digest.clone(),
    };
    let first = h
        .bind(
            BindRequest {
                store: &store,
                service: &service,
                version: &a,
                expected: &None,
                package: &q,
                failure: p::FailureMode::Reject,
                pending: None,
            },
            &Authorization::from_context(&c, "first-instance"),
        )
        .unwrap();
    assert!(
        h.bind(
            BindRequest {
                store: &store,
                service: &service,
                version: &b,
                expected: &None,
                package: &q,
                failure: p::FailureMode::Builtin,
                pending: None
            },
            &Authorization::from_context(&c, "first-instance")
        )
        .is_err()
    );
    let second = h
        .bind(
            BindRequest {
                store: &store,
                service: &service,
                version: &b,
                expected: &Some(first.status.revision.clone()),
                package: &q,
                failure: p::FailureMode::Builtin,
                pending: None,
            },
            &Authorization::from_context(&c, "first-instance"),
        )
        .unwrap();
    assert_eq!(first.control_authority, second.control_authority);
    assert_eq!(
        h.binding(&store, &service, &a)
            .unwrap()
            .unwrap()
            .status
            .revision,
        first.status.revision
    );
    let revision = h.unbind(&store, &service, &second.status.revision).unwrap();
    let control = h.consistent_control(&store, &service).unwrap().unwrap();
    assert_eq!(revision, control.revision);
    assert!(control.head.is_none());
    assert_ne!(control.authority, first.control_authority);
    drop(h);
    let restarted = Host::new(temp.path());
    assert!(restarted.active.is_empty());
    assert_eq!(
        restarted
            .consistent_control(&store, &service)
            .unwrap()
            .unwrap()
            .revision,
        revision
    );
}
#[test]
fn marker_first_database_failure_is_detected_and_explicit_unbind_can_repair_without_guessing() {
    let temp = tempfile::tempdir().unwrap();
    let h = Host::new(temp.path());
    let store = id();
    let service = id();
    let c = Control {
        store_id: store.clone(),
        service_id: service.clone(),
        revision: id(),
        authority: id(),
        version: None,
        head: None,
        pending: None,
        pending_request_identity: None,
    };
    h.save_control(&c).unwrap();
    h.db().unwrap().execute_batch("CREATE TRIGGER deny_control BEFORE INSERT ON policy_controls BEGIN SELECT RAISE(FAIL,'owned test failure'); END;").unwrap();
    let mut changed = c.clone();
    changed.revision = id();
    assert!(h.save_control(&changed).is_err());
    assert_eq!(
        h.consistent_control(&store, &service).unwrap_err().code,
        "invalid_record"
    );
    h.db()
        .unwrap()
        .execute_batch("DROP TRIGGER deny_control")
        .unwrap();
    let repaired = h.unbind(&store, &service, &changed.revision).unwrap();
    assert_eq!(
        h.consistent_control(&store, &service)
            .unwrap()
            .unwrap()
            .revision,
        repaired
    );
}
#[test]
fn corrupt_marker_only_identifies_its_required_service_and_pending_cannot_be_discarded() {
    let temp = tempfile::tempdir().unwrap();
    let h = Host::new(temp.path());
    let store = id();
    let service = id();
    let c = Control {
        store_id: store.clone(),
        service_id: service.clone(),
        revision: id(),
        authority: id(),
        version: None,
        head: None,
        pending: Some(id()),
        pending_request_identity: None,
    };
    h.save_control(&c).unwrap();
    assert!(h.unbind(&store, &service, &c.revision).is_err());
    let path = std::fs::read_dir(&h.root)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::write(path, b"broken-owned-marker").unwrap();
    drop(h);
    let h = Host::new(temp.path());
    let cache = h.presence.read().unwrap();
    assert_eq!(cache.unknown.len(), 1);
    assert!(cache.unknown.contains(&service));
    assert!(!cache.unavailable);
}

#[test]
fn full_registry_rejects_only_new_scopes_before_writing_and_does_not_expand_past_startup_budget() {
    let temp = tempfile::tempdir().unwrap();
    let h = Host::new(temp.path());
    let store = id();
    {
        let mut cache = h.presence.write().unwrap();
        for _ in 0..256 {
            let service = id();
            cache.scopes.insert(
                (store.clone(), service.clone()),
                Control {
                    store_id: store.clone(),
                    service_id: service,
                    revision: id(),
                    authority: id(),
                    version: None,
                    head: None,
                    pending: None,
                    pending_request_identity: None,
                },
            );
        }
    }
    let c = Control {
        store_id: store,
        service_id: id(),
        revision: id(),
        authority: id(),
        version: None,
        head: None,
        pending: None,
        pending_request_identity: None,
    };
    assert_eq!(h.marker(&c).unwrap_err().code, "resource_limit");
    assert!(!h.root.exists());
    assert!(!h.presence.read().unwrap().unavailable);
}

pub(super) fn prepared(h: &Host) -> Intent {
    use local_agent_core::model_services::{
        ServiceDraft,
        operations::{OperationAction, OperationRequest},
    };
    let request = OperationRequest {
        store_id: id(),
        operation_id: id(),
        action: OperationAction::Save {
            draft: Box::new(ServiceDraft {
                id: id(),
                revision: 0,
                name: "Owned policy publication".into(),
                default_model_id: id(),
                models: vec![],
                routing_policy: local_agent_core::model_services::policy::RoutingPolicy {
                    enabled: true,
                    ..Default::default()
                },
            }),
        },
    };
    let service = request.action.identity().1.to_string();
    let c = Control {
        store_id: request.store_id.clone(),
        service_id: service.clone(),
        revision: id(),
        authority: id(),
        version: None,
        head: None,
        pending: Some(request.operation_id.clone()),
        pending_request_identity: Some(operation_identity(&request).unwrap()),
    };
    h.save_control(&c).unwrap();
    let context = context();
    let i = Intent {
        status: p::PublishStatus {
            store_id: request.store_id.clone(),
            operation_id: request.operation_id.clone(),
            service_id: service,
            state: "prepared".into(),
            error_code: None,
            binding: None,
            request,
        },
        package: p::PackageQuery {
            plugin_id: context.package.manifest.id.clone(),
            digest: context.package.digest.clone(),
        },
        failure: p::FailureMode::Reject,
        expected: None,
        before_revision: 0,
        control_revision: c.revision,
        authorization: Some(Authorization::from_context(&context, "original-instance")),
        cancellation_revision: None,
    };
    h.save_intent(&i).unwrap();
    i
}
fn bind_prepared(h: &Host, i: &Intent) -> Binding {
    h.bind(
        BindRequest {
            store: &i.status.store_id,
            service: &i.status.service_id,
            version: &id(),
            expected: &Some(i.control_revision.clone()),
            package: &i.package,
            failure: i.failure,
            pending: Some(&i.status.operation_id),
        },
        i.authorization.as_ref().unwrap(),
    )
    .unwrap()
}
fn marker_path(h: &Host) -> std::path::PathBuf {
    std::fs::read_dir(&h.root)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
}
#[test]
fn publication_keeps_pending_until_completion_and_failed_binding_transaction_leaves_barrier() {
    let temp = tempfile::tempdir().unwrap();
    let h = Host::new(temp.path());
    let mut i = prepared(&h);
    h.db().unwrap().execute_batch("CREATE TRIGGER deny_binding BEFORE INSERT ON policy_bindings BEGIN SELECT RAISE(FAIL,'owned binding failure'); END;").unwrap();
    let result = h.bind(
        BindRequest {
            store: &i.status.store_id,
            service: &i.status.service_id,
            version: &id(),
            expected: &Some(i.control_revision.clone()),
            package: &i.package,
            failure: i.failure,
            pending: Some(&i.status.operation_id),
        },
        i.authorization.as_ref().unwrap(),
    );
    assert!(result.is_err());
    assert!(
        h.consistent_control(&i.status.store_id, &i.status.service_id)
            .unwrap()
            .unwrap()
            .head
            .is_none()
    );
    h.db()
        .unwrap()
        .execute_batch("DROP TRIGGER deny_binding")
        .unwrap();
    let b = bind_prepared(&h, &i);
    assert_eq!(b.instance, "original-instance");
    assert!(i.authorization.as_ref().unwrap().matches(&b));
    assert_eq!(
        h.consistent_control(&i.status.store_id, &i.status.service_id)
            .unwrap()
            .unwrap()
            .pending,
        Some(i.status.operation_id.clone())
    );
    assert_eq!(
        h.unbind(&i.status.store_id, &i.status.service_id, &b.status.revision)
            .unwrap_err()
            .code,
        "result_pending"
    );
    h.db().unwrap().execute_batch("CREATE TRIGGER deny_finish BEFORE INSERT ON policy_intents WHEN json_extract(NEW.record,'$.status.state')='committed' BEGIN SELECT RAISE(FAIL,'owned finalize failure'); END;").unwrap();
    i.status.state = "committed".into();
    i.status.binding = Some(b.status.clone());
    assert!(h.save_intent(&i).is_err());
    assert_eq!(
        h.control(&i.status.store_id, &i.status.service_id)
            .unwrap()
            .unwrap()
            .pending,
        Some(i.status.operation_id.clone())
    );
    h.db()
        .unwrap()
        .execute_batch("DROP TRIGGER deny_finish")
        .unwrap();
    h.save_intent(&i).unwrap();
    h.clear_publish_pending(
        &b.status,
        &i.status.operation_id,
        &operation_identity(&i.status.request).unwrap(),
    )
    .unwrap();
    assert!(
        h.consistent_control(&i.status.store_id, &i.status.service_id)
            .unwrap()
            .unwrap()
            .pending
            .is_none()
    );
}
#[test]
fn committed_query_repairs_database_first_clear_after_marker_failure_and_cannot_change_newer_choice()
 {
    let temp = tempfile::tempdir().unwrap();
    let h = Host::new(temp.path());
    let mut i = prepared(&h);
    let b = bind_prepared(&h, &i);
    i.status.state = "committed".into();
    i.status.binding = Some(b.status.clone());
    h.save_intent(&i).unwrap();
    let marker = marker_path(&h);
    std::fs::remove_file(&marker).unwrap();
    std::fs::create_dir(&marker).unwrap();
    let identity = operation_identity(&i.status.request).unwrap();
    assert!(
        h.clear_publish_pending(&b.status, &i.status.operation_id, &identity)
            .is_err()
    );
    assert!(
        h.control(&i.status.store_id, &i.status.service_id)
            .unwrap()
            .unwrap()
            .pending
            .is_none()
    );
    assert!(
        h.cached_control(&i.status.store_id, &i.status.service_id)
            .unwrap()
            .unwrap()
            .pending
            .is_some()
    );
    drop(h);
    let h = Host::new(temp.path());
    assert!(
        h.presence
            .read()
            .unwrap()
            .unknown
            .contains(&i.status.service_id)
    );
    std::fs::remove_dir(&marker).unwrap();
    h.clear_publish_pending(&b.status, &i.status.operation_id, &identity)
        .unwrap();
    assert!(
        !h.presence
            .read()
            .unwrap()
            .unknown
            .contains(&i.status.service_id)
    );
    let later = h
        .bind(
            BindRequest {
                store: &i.status.store_id,
                service: &i.status.service_id,
                version: &id(),
                expected: &Some(b.status.revision.clone()),
                package: &i.package,
                failure: p::FailureMode::Builtin,
                pending: None,
            },
            &Authorization::from_context(&context(), "later-instance"),
        )
        .unwrap();
    h.clear_publish_pending(&b.status, &i.status.operation_id, &identity)
        .unwrap();
    assert_eq!(
        h.consistent_control(&i.status.store_id, &i.status.service_id)
            .unwrap()
            .unwrap()
            .head
            .unwrap(),
        later.status
    );
}
#[test]
fn prepared_cancellation_is_atomic_and_query_only_repairs_its_own_marker() {
    let temp = tempfile::tempdir().unwrap();
    let h = Host::new(temp.path());
    let i = prepared(&h);
    h.db().unwrap().execute_batch("CREATE TRIGGER deny_cancel BEFORE INSERT ON policy_intents WHEN json_extract(NEW.record,'$.status.state')='cancelled' BEGIN SELECT RAISE(FAIL,'owned cancel failure'); END;").unwrap();
    assert!(h.cancel_prepared(i.clone()).is_err());
    assert_eq!(
        h.intent(&i.status.operation_id).unwrap().status.state,
        "prepared"
    );
    assert_eq!(
        h.consistent_control(&i.status.store_id, &i.status.service_id)
            .unwrap()
            .unwrap()
            .pending,
        Some(i.status.operation_id.clone())
    );
    h.db()
        .unwrap()
        .execute_batch("DROP TRIGGER deny_cancel")
        .unwrap();
    let marker = marker_path(&h);
    std::fs::remove_file(&marker).unwrap();
    std::fs::create_dir(&marker).unwrap();
    assert!(h.cancel_prepared(i.clone()).is_err());
    let cancelled = h.intent(&i.status.operation_id).unwrap();
    assert_eq!(cancelled.status.state, "cancelled");
    drop(h);
    let h = Host::new(temp.path());
    std::fs::remove_dir(&marker).unwrap();
    h.repair_cancelled(&cancelled).unwrap();
    let c = h
        .consistent_control(&i.status.store_id, &i.status.service_id)
        .unwrap()
        .unwrap();
    assert!(c.pending.is_none());
    let later_revision = h.unbind(&c.store_id, &c.service_id, &c.revision).unwrap();
    h.repair_cancelled(&cancelled).unwrap();
    assert_eq!(
        h.consistent_control(&c.store_id, &c.service_id)
            .unwrap()
            .unwrap()
            .revision,
        later_revision
    );
}
#[test]
fn startup_missing_required_marker_stays_unknown_until_explicit_cas_unbind() {
    let temp = tempfile::tempdir().unwrap();
    let h = Host::new(temp.path());
    let i = prepared(&h);
    let b = bind_prepared(&h, &i);
    h.clear_publish_pending(
        &b.status,
        &i.status.operation_id,
        &operation_identity(&i.status.request).unwrap(),
    )
    .unwrap();
    let marker = marker_path(&h);
    std::fs::remove_file(&marker).unwrap();
    drop(h);
    let h = Host::new(temp.path());
    assert!(h.active.is_empty());
    assert!(!h.presence.read().unwrap().unavailable);
    assert!(
        h.presence
            .read()
            .unwrap()
            .unknown
            .contains(&i.status.service_id)
    );
    assert_eq!(
        h.consistent_control(&i.status.store_id, &i.status.service_id)
            .unwrap_err()
            .code,
        "invalid_record"
    );
    h.unbind(&i.status.store_id, &i.status.service_id, &b.status.revision)
        .unwrap();
    assert!(
        !h.presence
            .read()
            .unwrap()
            .unknown
            .contains(&i.status.service_id)
    );
    assert!(
        h.consistent_control(&i.status.store_id, &i.status.service_id)
            .unwrap()
            .unwrap()
            .head
            .is_none()
    );
}
#[test]
fn startup_malformed_control_is_scoped_and_does_not_disable_unrelated_native_services() {
    let temp = tempfile::tempdir().unwrap();
    let h = Host::new(temp.path());
    let i = prepared(&h);
    h.db()
        .unwrap()
        .execute(
            "UPDATE policy_controls SET record='broken-owned-control' WHERE scope=?1",
            [Host::scope(&i.status.store_id, &i.status.service_id)],
        )
        .unwrap();
    drop(h);
    let h = Host::new(temp.path());
    let cache = h.presence.read().unwrap();
    assert_eq!(cache.unknown.len(), 1);
    assert!(cache.unknown.contains(&i.status.service_id));
    assert!(!cache.unavailable);
}

#[test]
fn stored_control_and_binding_cannot_redirect_to_another_scope_or_version() {
    let temp = tempfile::tempdir().unwrap();
    let h = Host::new(temp.path());
    let i = prepared(&h);
    let b = bind_prepared(&h, &i);
    let scope = Host::scope(&i.status.store_id, &i.status.service_id);
    let c = h
        .control(&i.status.store_id, &i.status.service_id)
        .unwrap()
        .unwrap();
    let mut foreign = c.clone();
    foreign.service_id = id();
    h.db()
        .unwrap()
        .execute(
            "UPDATE policy_controls SET record=?1 WHERE scope=?2",
            rusqlite::params![encode(&foreign).unwrap(), scope],
        )
        .unwrap();
    assert_eq!(
        h.control(&i.status.store_id, &i.status.service_id)
            .unwrap_err()
            .code,
        "invalid_record"
    );
    h.db()
        .unwrap()
        .execute(
            "UPDATE policy_controls SET record=?1 WHERE scope=?2",
            rusqlite::params![encode(&c).unwrap(), scope],
        )
        .unwrap();
    let mut foreign = b.clone();
    foreign.status.service_id = id();
    h.db()
        .unwrap()
        .execute(
            "UPDATE policy_bindings SET record=?1 WHERE scope=?2",
            rusqlite::params![encode(&foreign).unwrap(), scope],
        )
        .unwrap();
    assert_eq!(
        h.binding(
            &i.status.store_id,
            &i.status.service_id,
            &b.status.service_version_id
        )
        .err()
        .unwrap()
        .code,
        "invalid_record"
    );
    foreign = b.clone();
    foreign.status.service_version_id = id();
    h.db()
        .unwrap()
        .execute(
            "UPDATE policy_bindings SET record=?1 WHERE scope=?2",
            rusqlite::params![encode(&foreign).unwrap(), scope],
        )
        .unwrap();
    assert_eq!(
        h.binding(
            &i.status.store_id,
            &i.status.service_id,
            &b.status.service_version_id
        )
        .err()
        .unwrap()
        .code,
        "invalid_record"
    );
}
