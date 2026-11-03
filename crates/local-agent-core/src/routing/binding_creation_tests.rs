use super::*;

fn source(store: &Store) -> Provider {
    let mut p = store.providers().unwrap().remove(0);
    p.endpoint = "http://127.0.0.1:32101".into();
    p.configured = true;
    p.protocol = "messages".into();
    p.model = "a".into();
    p.models = vec!["a".into()];
    p.credential_ref = None;
    p
}

fn rows(store: &Store, table: &str) -> i64 {
    store
        .connection
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn service(store: &Store, name: &str) -> crate::model_services::ModelService {
    use crate::model_services::{Capabilities, ModelDraft, ServiceDraft};
    let p = source(store);
    let provider_id = p.id.clone();
    store.save_provider(p).unwrap();
    let model_id = uuid::Uuid::new_v4().to_string();
    store
        .save_model_service(ServiceDraft {
            id: uuid::Uuid::new_v4().to_string(),
            revision: 0,
            name: name.into(),
            default_model_id: model_id.clone(),
            models: vec![ModelDraft {
                id: model_id,
                name: "Model".into(),
                provider_id,
                model: "a".into(),
                capabilities: Capabilities::default(),
            }],
            routing_policy: Default::default(),
        })
        .unwrap()
}

#[test]
fn binding_creation_rejects_rewritten_owner_before_commit() {
    for trigger in [
        "CREATE TRIGGER rewrite AFTER INSERT ON gateway_bindings BEGIN UPDATE gateway_bindings SET credential='00000000-0000-4000-8000-000000000000' WHERE id=NEW.id; END;",
        "CREATE TRIGGER rewrite AFTER INSERT ON gateway_bindings BEGIN UPDATE gateway_bindings SET agent='opencode',scope='other-scope',generation=2,created='changed' WHERE id=NEW.id; END;",
        "CREATE TRIGGER rewrite AFTER INSERT ON gateway_bindings BEGIN UPDATE gateway_route_versions SET value=json_set(value,'$.provider.model','rewritten') WHERE id=NEW.target; END;",
    ] {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let p = source(&store);
        store.connection.execute_batch(trigger).unwrap();
        assert!(
            store
                .dynamic_create("claude", "scope", &uuid::Uuid::new_v4().to_string(), &p)
                .is_err(),
            "a rewritten owner must never be confirmed"
        );
        assert_eq!(rows(&store, "gateway_bindings"), 0);
        assert_eq!(rows(&store, "gateway_route_versions"), 0);
    }
}

#[test]
fn binding_creation_rejects_ignored_or_rewritten_version_before_commit() {
    for trigger in [
        "CREATE TRIGGER rewrite BEFORE INSERT ON gateway_route_versions BEGIN SELECT RAISE(IGNORE); END;",
        "CREATE TRIGGER rewrite AFTER INSERT ON gateway_route_versions BEGIN UPDATE gateway_route_versions SET value=json_set(value,'$.provider.model','rewritten') WHERE id=NEW.id; END;",
        "CREATE TRIGGER rewrite AFTER INSERT ON gateway_route_versions BEGIN UPDATE gateway_route_versions SET value=json_set(value,'$.future',1) WHERE id=NEW.id; END;",
        "CREATE TRIGGER rewrite BEFORE INSERT ON gateway_bindings BEGIN SELECT RAISE(IGNORE); END;",
    ] {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let p = source(&store);
        store.connection.execute_batch(trigger).unwrap();
        assert!(
            store
                .dynamic_create("claude", "scope", &uuid::Uuid::new_v4().to_string(), &p)
                .is_err()
        );
        assert_eq!(rows(&store, "gateway_bindings"), 0);
        assert_eq!(rows(&store, "gateway_route_versions"), 0);
    }
}

#[test]
fn service_binding_creation_is_atomic_when_attachment_is_ignored_or_aborted() {
    for trigger in [
        "CREATE TRIGGER stop BEFORE INSERT ON model_service_bindings BEGIN SELECT RAISE(IGNORE); END;",
        "CREATE TRIGGER stop BEFORE INSERT ON model_service_bindings BEGIN SELECT RAISE(ABORT,'fixture failure'); END;",
    ] {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let service = service(&store, "Service");
        store.connection.execute_batch(trigger).unwrap();
        assert!(
            store
                .create_model_service_binding(
                    "claude",
                    "scope",
                    &uuid::Uuid::new_v4().to_string(),
                    &service,
                )
                .is_err()
        );
        assert_eq!(rows(&store, "gateway_bindings"), 0);
        assert_eq!(rows(&store, "gateway_route_versions"), 0);
        assert_eq!(rows(&store, "model_service_bindings"), 0);
        assert_eq!(
            store.model_service(&service.id).unwrap().version_id,
            service.version_id
        );
    }
}

#[test]
fn service_attachment_cannot_rewrite_a_binding_or_its_initial_version() {
    for trigger in [
        "CREATE TRIGGER rewrite AFTER INSERT ON model_service_bindings BEGIN UPDATE gateway_bindings SET credential='00000000-0000-4000-8000-000000000000' WHERE id=NEW.binding; END;",
        "CREATE TRIGGER rewrite AFTER INSERT ON model_service_bindings BEGIN UPDATE gateway_route_versions SET value=json_set(value,'$.provider.model','rewritten') WHERE binding=NEW.binding; END;",
    ] {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let service = service(&store, "Service");
        store.connection.execute_batch(trigger).unwrap();
        assert!(
            store
                .create_model_service_binding(
                    "opencode",
                    "scope",
                    &uuid::Uuid::new_v4().to_string(),
                    &service,
                )
                .is_err()
        );
        for table in [
            "gateway_bindings",
            "gateway_route_versions",
            "model_service_bindings",
        ] {
            assert_eq!(rows(&store, table), 0, "{table}");
        }
    }
}

#[test]
fn rewritten_attachment_is_rejected_by_both_creation_and_existing_attach_api() {
    for existing in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let expected = service(&store, "Expected");
        let other = service(&store, "Other");
        let old = existing.then(|| {
            store
                .dynamic_create(
                    "claude",
                    "scope",
                    &uuid::Uuid::new_v4().to_string(),
                    &expected.models[0].provider,
                )
                .unwrap()
        });
        store.connection.execute_batch(&format!(
            "CREATE TRIGGER rewrite AFTER INSERT ON model_service_bindings BEGIN UPDATE model_service_bindings SET service='{}',version='{}' WHERE binding=NEW.binding; END;",
            other.id, other.version_id,
        )).unwrap();
        if let Some(old) = old {
            assert!(store.attach_model_service(&old.id, &expected).is_err());
            assert_eq!(
                store
                    .dynamic_binding(&old.id)
                    .unwrap()
                    .unwrap()
                    .credential_ref,
                old.credential_ref
            );
        } else {
            assert!(
                store
                    .create_model_service_binding(
                        "claude",
                        "scope",
                        &uuid::Uuid::new_v4().to_string(),
                        &expected
                    )
                    .is_err()
            );
        }
        assert_eq!(rows(&store, "gateway_bindings"), i64::from(existing));
        assert_eq!(rows(&store, "gateway_route_versions"), i64::from(existing));
        assert_eq!(rows(&store, "model_service_bindings"), 0);
    }
}

#[test]
fn atomic_service_binding_roundtrips_after_reopen_without_creating_a_credential() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let service = service(&store, "Service");
    let reference = uuid::Uuid::new_v4().to_string();
    let (binding, attachment) = store
        .create_model_service_binding("opencode", "scope", &reference, &service)
        .unwrap();
    assert_eq!(binding.credential_ref, reference);
    assert_eq!(attachment.binding_id, binding.id);
    assert_eq!(attachment.installed.version_id, service.version_id);
    assert_eq!(rows(&store, "credential_recovery_values"), 0);
    drop(store);
    let reopened = Store::open(root.path()).unwrap();
    assert_eq!(
        encode(&reopened.dynamic_binding(&binding.id).unwrap().unwrap()).unwrap(),
        encode(&binding).unwrap()
    );
    assert_eq!(
        reopened
            .model_service_attachment(&binding.id)
            .unwrap()
            .unwrap()
            .installed
            .version_id,
        service.version_id
    );
}

#[test]
fn stale_service_snapshot_or_binding_capacity_rolls_back_the_entire_creation() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let mut invalid = service(&store, "Service");
    invalid.version_id = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        store
            .create_model_service_binding(
                "claude",
                "scope",
                &uuid::Uuid::new_v4().to_string(),
                &invalid
            )
            .unwrap_err()
            .code,
        "conflict"
    );
    assert_eq!(rows(&store, "gateway_bindings"), 0);
    assert_eq!(rows(&store, "gateway_route_versions"), 0);
    let valid = store.model_service(&invalid.id).unwrap();
    for _ in 0..64 {
        store
            .dynamic_create(
                "claude",
                "scope",
                &uuid::Uuid::new_v4().to_string(),
                &valid.models[0].provider,
            )
            .unwrap();
    }
    assert_eq!(
        store
            .create_model_service_binding(
                "claude",
                "scope",
                &uuid::Uuid::new_v4().to_string(),
                &valid
            )
            .unwrap_err()
            .code,
        "capacity"
    );
    assert_eq!(rows(&store, "gateway_bindings"), 64);
    assert_eq!(rows(&store, "gateway_route_versions"), 64);
    assert_eq!(rows(&store, "model_service_bindings"), 0);
}

#[test]
fn deferred_commit_failure_does_not_leave_a_binding_or_attachment() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let service = service(&store, "Service");
    store.connection.execute_batch("CREATE TRIGGER fail_commit AFTER INSERT ON model_service_bindings BEGIN INSERT INTO gateway_route_versions SELECT 'deferred-invalid','missing-owner',value FROM gateway_route_versions LIMIT 1; END;").unwrap();
    assert!(
        store
            .create_model_service_binding(
                "claude",
                "scope",
                &uuid::Uuid::new_v4().to_string(),
                &service
            )
            .is_err()
    );
    for table in [
        "gateway_bindings",
        "gateway_route_versions",
        "model_service_bindings",
    ] {
        assert_eq!(rows(&store, table), 0, "{table}");
    }
    assert!(store.connection.is_autocommit());
}
