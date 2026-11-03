use super::*;
use crate::{Provider, domain::provider_access::Target, providers::vault::MemoryVault};
use std::{os::unix::fs::PermissionsExt, path::Path, sync::Arc};

fn setup(root: &Path) -> (Store, ProviderManager) {
    let data = root.join("data");
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let store = Store::open(&data).unwrap();
    let manager = ProviderManager::new(
        home,
        &data,
        root.join("unused-helper"),
        Arc::new(MemoryVault::default()),
    )
    .unwrap();
    (store, manager)
}
fn request(store: &Store) -> Request {
    let protocol = store.provider_write_protocol().unwrap();
    let provider: Provider = serde_json::from_value(serde_json::json!({
        "id":format!("custom-{}",uuid::Uuid::new_v4()),"name":"Owned","vendor":"custom",
        "endpoint":"http://127.0.0.1:11434/v1","protocol":"responses","model":"m-private-name",
        "models":["m-private-name"],"configured":false,"category":"local","revision":0,
        "credentialRef":null,"credentialLabel":null
    }))
    .unwrap();
    Request {
        store_id: protocol.store_id,
        operation_id: uuid::Uuid::new_v4().to_string(),
        expected_sequence: protocol.sequence,
        kind: operations::Kind::Connect,
        provider,
    }
}
fn selection(request: &Request) -> Selection {
    Selection {
        plugin_id: "org.example.catalog".into(),
        digest: "a".repeat(64),
        binding_id: uuid::Uuid::new_v4().to_string(),
        target: Target::Draft {
            label: "Owned".into(),
            endpoint: request.provider.endpoint.clone(),
            protocol: "responses".into(),
            vendor: "custom".into(),
            credential_ref: None,
        },
    }
}
#[test]
fn catalog_intent_never_creates_a_business_action_and_survives_independent_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (store, manager) = setup(&root);
    let request = request(&store);
    let selected = selection(&request);
    assert!(
        manager
            .record_catalog_operation(&store, &request, &selected)
            .is_err()
    );
    assert!(
        store
            .existing_provider_operation(&request)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        serde_json::to_value(&request)
            .unwrap()
            .as_object()
            .unwrap()
            .len(),
        5
    );
    assert!(store.start_provider_operation(&request).unwrap().started);
    manager
        .record_catalog_operation(&store, &request, &selected)
        .unwrap();
    let path = manager.catalog_intent_path(&request, false).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(
        !std::fs::read_to_string(&path)
            .unwrap()
            .contains("m-private-name")
    );
    assert!(manager.catalog_operation_selection(&request).unwrap() == selected);
    assert!(
        manager
            .finish_catalog_operation_record(&store, &request)
            .is_err()
    );
    drop(manager);
    drop(store);
    let (store, manager) = setup(&root);
    assert!(!store.start_provider_operation(&request).unwrap().started);
    assert!(manager.catalog_operation_selection(&request).unwrap() == selected);
    store.cancel_provider_connection(&request).unwrap();
    manager
        .finish_catalog_operation_record(&store, &request)
        .unwrap();
    assert!(manager.read_catalog_intent(&request).unwrap().terminal);
}
#[test]
fn catalog_intent_corruption_or_changed_request_keeps_the_sql_result_independently_readable() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (store, manager) = setup(&root);
    let request = request(&store);
    let selected = selection(&request);
    store.start_provider_operation(&request).unwrap();
    manager
        .record_catalog_operation(&store, &request, &selected)
        .unwrap();
    let mut different = request.clone();
    different.provider.endpoint = "http://127.0.0.1:7777/v1".into();
    assert!(manager.catalog_operation_selection(&different).is_err());
    let path = manager.catalog_intent_path(&request, false).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let mut future: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    future["version"] = 2.into();
    std::fs::write(&path, serde_json::to_vec(&future).unwrap()).unwrap();
    let before = std::fs::read(&path).unwrap();
    assert!(manager.catalog_operation_selection(&request).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(
        store
            .provider_operation(&request.store_id, &request.operation_id)
            .unwrap()
            .status,
        Status::Pending
    );
    assert_eq!(
        store.cancel_provider_connection(&request).unwrap().status,
        Status::Cancelled
    );
    let mut overflow = request.clone();
    overflow.expected_sequence = u64::MAX;
    assert!(manager.catalog_operation_selection(&overflow).is_err());
    let save = Request {
        kind: operations::Kind::Save,
        ..super::tests::request(&store)
    };
    assert_eq!(
        store
            .start_provider_operation(&save)
            .unwrap()
            .operation
            .status,
        Status::Committed
    );
}
#[test]
fn bounded_catalog_intents_reclaim_only_confirmed_terminal_metadata_without_reopening_a_send() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (store, manager) = setup(&root);
    let first = request(&store);
    let mut active = first.clone();
    for index in 0..=LIMIT {
        if index > 0 {
            active = request(&store);
        }
        store.start_provider_operation(&active).unwrap();
        manager
            .record_catalog_operation(&store, &active, &selection(&active))
            .unwrap();
        store.cancel_provider_connection(&active).unwrap();
        manager
            .finish_catalog_operation_record(&store, &active)
            .unwrap();
    }
    assert_eq!(
        std::fs::read_dir(manager.catalog_intent_directory(false).unwrap())
            .unwrap()
            .count(),
        LIMIT
    );
    assert!(!manager.catalog_intent_path(&first, false).unwrap().exists());
    assert_eq!(
        store.start_provider_operation(&first).err().unwrap().code,
        "operation_expired"
    );
    assert_eq!(
        store.provider_write_protocol().unwrap().sequence,
        (LIMIT + 1) as u64
    );
}
