use crate::backend::{
    Backend,
    test_server::{serve, serve_status},
};
use std::sync::Arc;
use tokio::sync::Notify;

fn timeout_backend(endpoint: String, instance: &str) -> Backend {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_millis(20))
        .build()
        .unwrap();
    let backend = Backend::test_with_client(client, endpoint, "export-fixture-token".into(), false);
    backend
        .state()
        .unwrap()
        .connection
        .as_mut()
        .unwrap()
        .instance_id = instance.into();
    backend
}

fn runtime_reply(instance: &str) -> String {
    serde_json::json!({
        "protocol":local_agent_core::runtime::PROTOCOL,
        "version":env!("CARGO_PKG_VERSION"),
        "instanceId":instance,
        "pid":1,
        "persistent":false
    })
    .to_string()
}

#[tokio::test]
async fn read_timeout_keeps_the_same_confirmed_live_connection_without_replaying_the_read() {
    let instance = uuid::Uuid::new_v4().to_string();
    let identity = runtime_reply(&instance);
    let server = serve(move |request| {
        let identity = identity.clone();
        async move {
            assert_eq!(request.authorization, "Bearer export-fixture-token");
            if request.path == "/held-read" {
                tokio::time::sleep(std::time::Duration::from_millis(70)).await;
                "{}".into()
            } else if request.path == "/api/runtime" {
                identity
            } else {
                "{\"available\":true}".into()
            }
        }
    })
    .await;
    let backend = timeout_backend(server.endpoint.clone(), &instance);
    let generation = backend.connection().unwrap().generation;
    let error = backend
        .request(reqwest::Method::GET, "/held-read", None)
        .await
        .unwrap_err();
    let error: serde_json::Value = serde_json::from_str(&error).unwrap();
    assert_eq!(error["code"], "transport_timeout");
    assert_eq!(error["outcomeUnknown"], false);
    assert_eq!(backend.connection().unwrap().generation, generation);
    assert_eq!(
        backend
            .request(reqwest::Method::GET, "/unrelated", None)
            .await
            .unwrap()["available"],
        true
    );
    let requests = server.requests.lock().unwrap();
    assert_eq!(
        requests.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(),
        ["/held-read", "/api/runtime", "/unrelated"]
    );
}

#[tokio::test]
async fn domain_post_queries_keep_a_confirmed_connection_without_replay_or_write_uncertainty() {
    for path in [
        "/api/domain/requests",
        "/api/domain/activity",
        "/api/domain/facts",
        "/api/domain/sessions",
        "/api/domain/conversation",
        "/api/domain/fact-conversation",
    ] {
        let instance = uuid::Uuid::new_v4().to_string();
        let identity = runtime_reply(&instance);
        let body = serde_json::json!({"scope":{"sourceIds":[]}});
        let expected = body.clone();
        let server = serve(move |request| {
            let (identity, expected) = (identity.clone(), expected.clone());
            async move {
                assert_eq!(request.authorization, "Bearer export-fixture-token");
                if request.path == path {
                    assert_eq!(request.method, "POST");
                    assert_eq!(request.body, expected);
                    tokio::time::sleep(std::time::Duration::from_millis(70)).await;
                    "{}".into()
                } else if request.path == "/api/runtime" {
                    assert_eq!(request.method, "GET");
                    identity
                } else {
                    assert_eq!(request.path, "/unrelated");
                    "{\"available\":true}".into()
                }
            }
        })
        .await;
        let backend = timeout_backend(server.endpoint.clone(), &instance);
        let generation = backend.connection().unwrap().generation;
        let error = backend
            .request(reqwest::Method::POST, path, Some(body))
            .await
            .unwrap_err();
        let error: serde_json::Value = serde_json::from_str(&error).unwrap();
        assert_eq!(error["code"], "transport_timeout", "{path}");
        assert_eq!(error["outcomeUnknown"], false, "{path}");
        assert_eq!(backend.connection().unwrap().generation, generation);
        assert_eq!(
            backend
                .request(reqwest::Method::GET, "/unrelated", None)
                .await
                .unwrap()["available"],
            true
        );
        assert_eq!(
            server
                .requests
                .lock()
                .unwrap()
                .iter()
                .map(|r| r.path.as_str())
                .collect::<Vec<_>>(),
            [path, "/api/runtime", "/unrelated"]
        );
    }
}

#[tokio::test]
async fn domain_post_query_success_from_a_replaced_connection_cannot_publish() {
    for path in [
        "/api/domain/requests",
        "/api/domain/activity",
        "/api/domain/facts",
        "/api/domain/sessions",
        "/api/domain/conversation",
        "/api/domain/fact-conversation",
    ] {
        let entered = Arc::new(Notify::new());
        let released = Arc::new(Notify::new());
        let (hit, done) = (entered.clone(), released.clone());
        let original = serve(move |request| {
            let (hit, done) = (hit.clone(), done.clone());
            async move {
                assert_eq!(request.method, "POST");
                assert_eq!(request.path, path);
                hit.notify_one();
                done.notified().await;
                "{\"old\":true}".into()
            }
        })
        .await;
        let replacement = serve(|_| async { "{\"new\":true}".into() }).await;
        let backend = Backend::test_connection(original.endpoint.clone());
        let context = backend.context().unwrap();
        let task =
            tokio::spawn(async move { context.request(reqwest::Method::POST, path, None).await });
        tokio::time::timeout(std::time::Duration::from_secs(1), entered.notified())
            .await
            .unwrap();
        backend.test_install_connection(replacement.endpoint.clone());
        let generation = backend.connection().unwrap().generation;
        released.notify_one();
        assert!(
            task.await
                .unwrap()
                .unwrap_err()
                .contains("backend_connection_changed"),
            "{path}"
        );
        assert_eq!(backend.connection().unwrap().generation, generation);
        assert_eq!(original.requests.lock().unwrap().len(), 1);
        assert!(replacement.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn domain_writes_and_query_lookalikes_remain_uncertain_without_probe_or_replay() {
    for (method, path) in [
        (reqwest::Method::POST, "/api/domain/source-refresh"),
        (reqwest::Method::POST, "/api/domain/source-authorize"),
        (reqwest::Method::POST, "/api/domain/consent-review"),
        (reqwest::Method::POST, "/api/domain/consent"),
        (reqwest::Method::POST, "/api/domain/project-visibility"),
        (reqwest::Method::POST, "/api/domain/request-exports"),
        (reqwest::Method::POST, "/api/domain/requests/export"),
        (reqwest::Method::POST, "/api/domain/requests?write=true"),
        (reqwest::Method::POST, "/api/domain/requests/"),
        (reqwest::Method::POST, "/api/domain/future"),
        (reqwest::Method::PUT, "/api/domain/requests"),
        (reqwest::Method::DELETE, "/api/domain/sessions"),
    ] {
        let server = serve(|_| async {
            tokio::time::sleep(std::time::Duration::from_millis(70)).await;
            "{}".into()
        })
        .await;
        let backend = timeout_backend(server.endpoint.clone(), "owned-instance");
        let error = backend.request(method, path, None).await.unwrap_err();
        let error: serde_json::Value = serde_json::from_str(&error).unwrap();
        assert_eq!(error["code"], "transport_timeout", "{path}");
        assert_eq!(error["outcomeUnknown"], true, "{path}");
        assert!(backend.connection().is_err(), "{path}");
        assert_eq!(server.requests.lock().unwrap().len(), 1, "{path}");
    }
}

#[tokio::test]
async fn domain_post_query_timeout_cannot_keep_an_unverified_origin_connected() {
    for reply in [runtime_reply("different-instance"), "invalid-json".into()] {
        let server = serve(move |request| {
            let reply = reply.clone();
            async move {
                if request.path == "/api/domain/requests" {
                    tokio::time::sleep(std::time::Duration::from_millis(70)).await;
                    "{}".into()
                } else {
                    assert_eq!(request.path, "/api/runtime");
                    reply
                }
            }
        })
        .await;
        let backend = timeout_backend(server.endpoint.clone(), "owned-instance");
        let error = backend
            .request(reqwest::Method::POST, "/api/domain/requests", None)
            .await
            .unwrap_err();
        let error: serde_json::Value = serde_json::from_str(&error).unwrap();
        assert_eq!(error["code"], "transport_timeout");
        assert_eq!(error["outcomeUnknown"], false);
        assert!(backend.connection().is_err());
        assert_eq!(server.requests.lock().unwrap().len(), 2);
    }
}

#[tokio::test]
async fn write_timeout_stays_uncertain_and_does_not_probe_or_replay() {
    let server = serve(|_| async {
        tokio::time::sleep(std::time::Duration::from_millis(70)).await;
        "{\"committed\":true}".into()
    })
    .await;
    let backend = timeout_backend(server.endpoint.clone(), "owned-instance");
    let error = backend
        .request(reqwest::Method::POST, "/held-write", None)
        .await
        .unwrap_err();
    let error: serde_json::Value = serde_json::from_str(&error).unwrap();
    assert_eq!(error["code"], "transport_timeout");
    assert_eq!(error["outcomeUnknown"], true);
    assert!(backend.connection().is_err());
    assert_eq!(server.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn unverified_runtime_keeps_a_read_timeout_disconnected() {
    let instance = uuid::Uuid::new_v4().to_string();
    let mut incompatible: serde_json::Value =
        serde_json::from_str(&runtime_reply(&instance)).unwrap();
    let mut replies = vec![
        (200, runtime_reply("different-instance")),
        (200, "invalid-json".into()),
        (200, "x".repeat(4097)),
        (503, runtime_reply(&instance)),
    ];
    for (field, value) in [
        ("protocol", serde_json::json!(999)),
        ("version", serde_json::json!("future")),
        ("persistent", serde_json::json!(true)),
        ("pid", serde_json::json!(0)),
    ] {
        let before = incompatible[field].clone();
        incompatible[field] = value;
        replies.push((200, incompatible.to_string()));
        incompatible[field] = before;
    }
    for (status, body) in replies {
        let server = serve_status(move |request| {
            let body = body.clone();
            async move {
                if request.path == "/held-read" {
                    tokio::time::sleep(std::time::Duration::from_millis(70)).await;
                    (200, "{}".into())
                } else {
                    assert_eq!(request.path, "/api/runtime");
                    (status, body)
                }
            }
        })
        .await;
        let backend = timeout_backend(server.endpoint.clone(), &instance);
        let error = backend
            .request(reqwest::Method::GET, "/held-read", None)
            .await
            .unwrap_err();
        assert!(error.contains("transport_timeout"));
        assert!(backend.connection().is_err());
        assert_eq!(server.requests.lock().unwrap().len(), 2);
    }
}

#[tokio::test]
async fn liveness_check_has_its_own_bounded_budget() {
    let server = serve(|request| async move {
        tokio::time::sleep(if request.path == "/api/runtime" {
            std::time::Duration::from_secs(3)
        } else {
            std::time::Duration::from_millis(70)
        })
        .await;
        "{}".into()
    })
    .await;
    let backend = timeout_backend(server.endpoint.clone(), "owned-instance");
    let started = std::time::Instant::now();
    let error = backend
        .request(reqwest::Method::GET, "/held-read", None)
        .await
        .unwrap_err();
    assert!(error.contains("transport_timeout"));
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    assert!(backend.connection().is_err());
    assert_eq!(server.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn late_liveness_probe_does_not_install_or_disconnect_a_replacement_connection() {
    let instance = uuid::Uuid::new_v4().to_string();
    let identity = runtime_reply(&instance);
    let entered = Arc::new(Notify::new());
    let released = Arc::new(Notify::new());
    let (hit, done) = (entered.clone(), released.clone());
    let original = serve(move |request| {
        let (identity, hit, done) = (identity.clone(), hit.clone(), done.clone());
        async move {
            if request.path == "/held-read" {
                tokio::time::sleep(std::time::Duration::from_millis(70)).await;
                "{}".into()
            } else {
                assert_eq!(request.path, "/api/runtime");
                hit.notify_one();
                done.notified().await;
                identity
            }
        }
    })
    .await;
    let replacement = serve(|_| async { "{\"new\":true}".into() }).await;
    let backend = Arc::new(timeout_backend(original.endpoint.clone(), &instance));
    let pending = {
        let backend = backend.clone();
        tokio::spawn(async move {
            backend
                .request(reqwest::Method::GET, "/held-read", None)
                .await
        })
    };
    tokio::time::timeout(std::time::Duration::from_secs(1), entered.notified())
        .await
        .unwrap();
    backend.test_install_connection(replacement.endpoint.clone());
    let generation = backend.connection().unwrap().generation;
    released.notify_one();
    assert!(
        pending
            .await
            .unwrap()
            .unwrap_err()
            .contains("transport_timeout")
    );
    assert_eq!(backend.connection().unwrap().generation, generation);
    assert_eq!(
        backend
            .request(reqwest::Method::GET, "/current", None)
            .await
            .unwrap()["new"],
        true
    );
    assert_eq!(original.requests.lock().unwrap().len(), 2);
    assert_eq!(replacement.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn scoped_credential_replies_are_bounded_without_hiding_write_uncertainty() {
    let limit = local_agent_core::providers::credential_scope::MAX_RESPONSE_BYTES;
    for (method, unknown) in [(reqwest::Method::GET, false), (reqwest::Method::POST, true)] {
        let body = format!("\"{}\"", "x".repeat(limit));
        let server = serve(move |_| {
            let body = body.clone();
            async move { body }
        })
        .await;
        let backend = Backend::test_connection(server.endpoint.clone());
        let error = backend
            .request(method, "/api/provider-credentials/scoped", None)
            .await
            .unwrap_err();
        let value: serde_json::Value = serde_json::from_str(&error).unwrap();
        assert_eq!(value["code"], "transport_invalid_response");
        assert_eq!(value["outcomeUnknown"], unknown);
    }
}
#[tokio::test]
async fn old_backend_cannot_receive_a_scoped_credential_after_protocol_rejection() {
    let server = serve_status(|_| async { (404, "old service".into()) }).await;
    let backend = Backend::test_connection(server.endpoint.clone());
    let error = backend
        .request(
            reqwest::Method::GET,
            "/api/provider-credentials/protocol?storeId=fixture",
            None,
        )
        .await
        .unwrap_err();
    let value: serde_json::Value = serde_json::from_str(&error).unwrap();
    assert_eq!(value["code"], "credential_protocol_unavailable");
    assert_eq!(value["outcomeUnknown"], false);
    assert_eq!(server.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn provider_operation_response_budget_preserves_read_and_write_uncertainty() {
    let limit = local_agent_core::providers::operations::MAX_RESPONSE_BYTES;
    for (method, unknown) in [(reqwest::Method::GET, false), (reqwest::Method::POST, true)] {
        let body = format!("\"{}\"", "x".repeat(limit));
        let server = serve(move |_| {
            let body = body.clone();
            async move { body }
        })
        .await;
        let backend = Backend::test_connection(server.endpoint.clone());
        let error = backend
            .request(method, "/api/provider-operations", None)
            .await
            .unwrap_err();
        let error: serde_json::Value = serde_json::from_str(&error).unwrap();
        assert_eq!(error["code"], "transport_invalid_response");
        assert_eq!(error["outcomeUnknown"], unknown);
    }
    let body = format!("\"{}\"", "x".repeat(limit - 2));
    let server = serve(move |_| {
        let body = body.clone();
        async move { body }
    })
    .await;
    let backend = Backend::test_connection(server.endpoint.clone());
    assert_eq!(
        backend
            .request(reqwest::Method::GET, "/api/provider-operations", None)
            .await
            .unwrap()
            .as_str()
            .unwrap()
            .len(),
        limit - 2
    );
}

#[tokio::test]
async fn older_provider_protocol_is_explicitly_unavailable_even_with_a_non_json_404() {
    let server = serve_status(|_| async { (404, "<html>older service</html>".into()) }).await;
    let backend = Backend::test_connection(server.endpoint.clone());
    let error = backend
        .request(
            reqwest::Method::GET,
            "/api/provider-operations/protocol",
            None,
        )
        .await
        .unwrap_err();
    let error: serde_json::Value = serde_json::from_str(&error).unwrap();
    assert_eq!(error["code"], "write_protocol_unavailable");
    assert_eq!(error["outcomeUnknown"], false);
}

#[tokio::test]
async fn later_steps_reject_reconnect_but_compensation_uses_original_address_and_token() {
    let a = serve(|_| async { "{\"revoked\":true}".into() }).await;
    let b = serve(|_| async { "{}".into() }).await;
    let backend = Backend::test_connection(a.endpoint.clone());
    let context = backend.context().unwrap();
    context
        .request(reqwest::Method::GET, "/first", None)
        .await
        .unwrap();
    backend.test_install_connection(b.endpoint.clone());
    let error = context
        .request(reqwest::Method::POST, "/must-not-start", None)
        .await
        .unwrap_err();
    let error: serde_json::Value = serde_json::from_str(&error).unwrap();
    assert_eq!(error["code"], "backend_connection_changed");
    assert_eq!(error["outcomeUnknown"], false);
    context
        .origin_request(reqwest::Method::POST, "/revoke-original", None)
        .await
        .unwrap();
    let requests = a.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].path, "/revoke-original");
    assert!(
        requests
            .iter()
            .all(|r| r.authorization == "Bearer export-fixture-token")
    );
    assert!(b.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn late_reads_do_not_publish_but_completed_original_writes_are_not_lost() {
    for method in [reqwest::Method::GET, reqwest::Method::POST] {
        let entered = Arc::new(Notify::new());
        let released = Arc::new(Notify::new());
        let (hit, done) = (entered.clone(), released.clone());
        let a = serve(move |_| {
            let (hit, done) = (hit.clone(), done.clone());
            async move {
                hit.notify_one();
                done.notified().await;
                "{\"committed\":true}".into()
            }
        })
        .await;
        let b = serve(|_| async { "{}".into() }).await;
        let backend = Backend::test_connection(a.endpoint.clone());
        let context = backend.context().unwrap();
        let write = method == reqwest::Method::POST;
        let task = tokio::spawn(async move { context.request(method, "/held", None).await });
        tokio::time::timeout(std::time::Duration::from_secs(4), entered.notified())
            .await
            .unwrap();
        backend.test_install_connection(b.endpoint.clone());
        released.notify_one();
        let result = task.await.unwrap();
        if write {
            assert_eq!(result.unwrap()["committed"], true);
        } else {
            assert!(result.unwrap_err().contains("backend_connection_changed"));
        }
        assert!(b.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn failed_origin_cleanup_does_not_disconnect_the_replacement_backend() {
    let a = serve(|_| async { "invalid-json".into() }).await;
    let b = serve(|_| async { "{\"new\":true}".into() }).await;
    let backend = Backend::test_connection(a.endpoint.clone());
    let context = backend.context().unwrap();
    backend.test_install_connection(b.endpoint.clone());
    let error = context
        .origin_request(reqwest::Method::POST, "/cleanup", None)
        .await
        .unwrap_err();
    assert!(error.contains("transport_invalid_response"));
    assert_eq!(
        backend
            .request(reqwest::Method::GET, "/current", None)
            .await
            .unwrap()["new"],
        true
    );
    assert_eq!(
        b.requests.lock().unwrap()[0].authorization,
        "Bearer replacement-fixture-token"
    );
}

#[tokio::test]
async fn explicit_origin_read_can_reconcile_a_disconnected_original_without_installing_a_new_connection()
 {
    let a = serve(|r| async move {
        if r.path == "/invalid" {
            "invalid-json".into()
        } else {
            "{\"applied\":true}".into()
        }
    })
    .await;
    let backend = Backend::test_connection(a.endpoint.clone());
    let context = backend.context().unwrap();
    assert!(
        context
            .request(reqwest::Method::POST, "/invalid", None)
            .await
            .is_err()
    );
    assert!(context.ensure_current().is_err());
    assert_eq!(
        context
            .origin_request(reqwest::Method::GET, "/state", None)
            .await
            .unwrap()["applied"],
        true
    );
    assert!(
        context.ensure_current().is_err(),
        "readback must not silently reconnect"
    );
}
