use super::*;
use local_agent_core::runtime::ServiceGuard;
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
};

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap()
}
fn fixture() -> (tempfile::TempDir, ServiceGuard, TcpListener, ServiceRecord) {
    let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let guard = ServiceGuard::acquire(root.path()).unwrap();
    let token = uuid::Uuid::new_v4().to_string();
    let identity = guard
        .publish_mode(
            listener.local_addr().unwrap().port(),
            token.clone(),
            env!("CARGO_PKG_VERSION"),
            true,
        )
        .unwrap();
    let record = ServiceRecord {
        identity,
        port: listener.local_addr().unwrap().port(),
        token,
    };
    (root, guard, listener, record)
}
fn headers(stream: &mut TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut bytes = vec![];
    while !bytes.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        assert_eq!(stream.read(&mut byte).unwrap(), 1);
        bytes.push(byte[0]);
        assert!(bytes.len() < 8192);
    }
    String::from_utf8(bytes).unwrap()
}
fn reply(stream: &mut TcpStream, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}
fn failed() -> Backend {
    Backend::new(client(), None, Some("启动未完成".into()))
}
fn value(backend: &Backend) -> serde_json::Value {
    serde_json::to_value(backend.diagnostics().unwrap()).unwrap()
}

#[test]
fn bounded_diagnostics_exclude_connection_and_error_payloads_and_ignore_old_exit_state() {
    let mut state = Lifecycle::new(None, Some("private-error-sentinel".into()));
    let generation = uuid::Uuid::new_v4().to_string();
    let operation = uuid::Uuid::new_v4().to_string();
    for _ in 0..40 {
        state.record(&operation, Phase::Process, Category::ChildError, None, None);
    }
    state
        .install(Connection {
            endpoint: "http://private-endpoint.invalid".into(),
            token: "private-token-sentinel".into(),
            persistent: true,
            generation: generation.clone(),
            instance_id: generation.clone(),
            iam_protocol: true,
            iam_epoch: 1,
            iam_session: Some(std::sync::Arc::new(zeroize::Zeroizing::new(
                "private-session-sentinel".into(),
            ))),
        })
        .unwrap();
    state.process_exit("old-generation", &operation, Some(7), Some(15));
    let encoded = serde_json::to_value(state.snapshot()).unwrap();
    assert_eq!(encoded["state"], "connected");
    assert_eq!(encoded["events"].as_array().unwrap().len(), 32);
    assert_eq!(encoded["events"][0]["exitCode"], 7);
    for secret in [
        "private-error-sentinel",
        "private-token-sentinel",
        "private-session-sentinel",
        "private-endpoint.invalid",
        "old-generation",
        &generation,
    ] {
        assert!(!encoded.to_string().contains(secret));
    }
    state.process_exit(&generation, &operation, None, Some(9));
    assert!(matches!(state.snapshot().state, Status::Disconnected));
    assert!(
        state.install(state.connection.clone().unwrap()).is_err(),
        "an observed dead instance must not become connected again"
    );
}

#[test]
fn reconnect_from_startup_failure_uses_only_verified_reads_and_preserves_persistent_owner() {
    let (root, _guard, listener, record) = fixture();
    let identity = serde_json::to_string(&record.identity).unwrap();
    let expected = record.token.clone();
    let server = std::thread::spawn(move || {
        for (path, body) in [
            ("/api/runtime", identity.as_str()),
            ("/api/snapshot", r#"{"ready":true}"#),
            ("/api/runtime", identity.as_str()),
        ] {
            let (mut stream, _) = listener.accept().unwrap();
            let request = headers(&mut stream);
            assert!(request.starts_with(&format!("GET {path} HTTP/1.1")));
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains(&format!("authorization: bearer {expected}"))
            );
            reply(&mut stream, body);
        }
    });
    let backend = failed();
    assert_eq!(value(&backend)["state"], "startup_failed");
    tauri::async_runtime::block_on(async {
        assert!(matches!(
            backend.reconnect(root.path()).await.unwrap().state,
            Status::Connected
        ));
        assert_eq!(
            backend
                .request(reqwest::Method::GET, "/api/snapshot", None)
                .await
                .unwrap()["ready"],
            true
        );
        let context = backend.context().unwrap();
        let first_epoch = backend
            .state()
            .unwrap()
            .connection
            .as_ref()
            .unwrap()
            .generation
            .clone();
        context.ensure_current().unwrap();
        backend.reconnect(root.path()).await.unwrap();
        assert!(
            context.ensure_current().is_err(),
            "a composite read cannot combine values across reconnects"
        );
        assert_ne!(
            first_epoch,
            backend
                .state()
                .unwrap()
                .connection
                .as_ref()
                .unwrap()
                .generation,
            "rechecking the same published instance must create a new connection epoch"
        );
    });
    server.join().unwrap();
    assert!(!value(&backend).to_string().contains(&record.token));
    backend.stop(root.path());
    assert!(
        runtime::read_service(root.path())
            .unwrap()
            .unwrap()
            .identity
            .persistent
    );
    assert!(!value(&backend)["canReconnect"].as_bool().unwrap());
}

#[test]
fn reconnect_rejects_publication_changes_and_oversized_identity_replies() {
    for oversized in [false, true] {
        let (root, _guard, listener, record) = fixture();
        let path = runtime::directory(root.path()).join("service.json");
        let identity = serde_json::to_string(&record.identity).unwrap();
        let token = record.token.clone();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            headers(&mut stream);
            if oversized {
                reply(&mut stream, &"X".repeat(8192));
            } else {
                let mut replacement = record;
                replacement.token = uuid::Uuid::new_v4().to_string();
                local_agent_core::providers::files::atomic_write(
                    &path,
                    &serde_json::to_vec(&replacement).unwrap(),
                    0o600,
                )
                .unwrap();
                reply(&mut stream, &identity);
            }
        });
        let backend = failed();
        let error = tauri::async_runtime::block_on(backend.reconnect(root.path()))
            .err()
            .unwrap();
        assert!(error.contains(if oversized {
            "超出限制"
        } else {
            "发布已变化"
        }));
        assert!(!error.contains(&token));
        assert_eq!(value(&backend)["state"], "disconnected");
        assert_eq!(value(&backend)["events"][0]["category"], "reconnect_failed");
        server.join().unwrap();
    }
}

#[test]
fn failed_or_missing_discovery_never_dispatches_the_original_write() {
    let root = tempfile::tempdir().unwrap();
    let backend = failed();
    tauri::async_runtime::block_on(async {
        assert!(backend.reconnect(root.path()).await.is_err());
        let error = backend
            .request(
                reqwest::Method::POST,
                "/api/provider-credentials",
                Some(serde_json::json!({"secret":"disposable-not-sent"})),
            )
            .await
            .unwrap_err();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&error).unwrap()["outcomeUnknown"],
            false
        );
    });
    assert_eq!(value(&backend)["events"].as_array().unwrap().len(), 2);
}

#[test]
fn reconnect_admission_cancellation_and_shutdown_do_not_publish_a_late_connection() {
    for stop in [false, true] {
        let (root, _guard, listener, record) = fixture();
        let identity = serde_json::to_string(&record.identity).unwrap();
        let entered = Arc::new(tokio::sync::Notify::new());
        let seen = entered.clone();
        let (release, wait) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            headers(&mut stream);
            seen.notify_one();
            wait.recv_timeout(Duration::from_secs(2)).unwrap();
            reply(&mut stream, &identity);
        });
        let backend = Arc::new(failed());
        tauri::async_runtime::block_on(async {
            let other = backend.clone();
            let directory = root.path().to_path_buf();
            let task = tokio::spawn(async move { other.reconnect(&directory).await });
            entered.notified().await;
            assert!(
                backend
                    .reconnect(root.path())
                    .await
                    .err()
                    .unwrap()
                    .contains("backend_reconnect_busy")
            );
            if stop {
                let mut state = backend.state().unwrap();
                state.stopping = true;
                state.status = Status::Stopping;
            } else {
                task.abort();
            }
            release.send(()).unwrap();
            let result = task.await;
            if stop {
                assert!(result.unwrap().is_err());
            } else {
                assert!(matches!(result, Err(error) if error.is_cancelled()));
            }
            assert!(backend.state().unwrap().connection.is_none());
            assert_eq!(
                value(&backend)["state"],
                if stop { "stopping" } else { "disconnected" }
            );
        });
        server.join().unwrap();
    }
}

#[test]
fn old_reads_and_failures_cannot_replace_a_new_connection_but_confirmed_writes_return_their_receipt()
 {
    for (method, failure, same_instance) in [
        (reqwest::Method::GET, false, false),
        (reqwest::Method::PUT, false, false),
        (reqwest::Method::GET, true, false),
        (reqwest::Method::GET, true, true),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let backend = Arc::new(Backend::test_connection(format!(
            "http://{}",
            listener.local_addr().unwrap()
        )));
        let entered = Arc::new(tokio::sync::Notify::new());
        let seen = entered.clone();
        let (release, wait) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            headers(&mut stream);
            seen.notify_one();
            wait.recv_timeout(Duration::from_secs(2)).unwrap();
            if !failure {
                reply(&mut stream, r#"{"committed":true}"#);
            }
        });
        tauri::async_runtime::block_on(async {
            let other = backend.clone();
            let operation = method.clone();
            let task =
                tokio::spawn(async move { other.request(operation, "/api/settings", None).await });
            entered.notified().await;
            let old_instance = backend
                .state()
                .unwrap()
                .connection
                .as_ref()
                .unwrap()
                .instance_id
                .clone();
            backend
                .state()
                .unwrap()
                .install(Connection {
                    endpoint: "http://127.0.0.1:1".into(),
                    token: "new-private-token".into(),
                    generation: uuid::Uuid::new_v4().to_string(),
                    persistent: true,
                    instance_id: if same_instance {
                        old_instance
                    } else {
                        uuid::Uuid::new_v4().to_string()
                    },
                    iam_protocol: true,
                    iam_epoch: 0,
                    iam_session: None,
                })
                .unwrap();
            release.send(()).unwrap();
            let result = task.await.unwrap();
            if method == reqwest::Method::PUT {
                assert_eq!(result.unwrap()["committed"], true);
            } else {
                assert!(result.is_err());
            }
            assert_eq!(value(&backend)["state"], "connected");
        });
        server.join().unwrap();
    }
}

#[test]
fn child_exit_metadata_and_typed_events_never_store_output_payloads() {
    let backend = Backend::test_connection("http://127.0.0.1:1".into());
    let generation = backend
        .state()
        .unwrap()
        .connection
        .as_ref()
        .unwrap()
        .instance_id
        .clone();
    let operation = uuid::Uuid::new_v4().to_string();
    let output = std::process::Command::new("/bin/sh")
        .args([
            "-c",
            "printf 'private-stdout-sentinel\\n'; printf 'private-stderr-sentinel\\n' >&2; exit 7",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(7));
    tauri::async_runtime::block_on(async {
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        tx.send(CommandEvent::Stdout(output.stdout)).await.unwrap();
        tx.send(CommandEvent::Stderr(output.stderr)).await.unwrap();
        tx.send(CommandEvent::Error("private-event-error-sentinel".into()))
            .await
            .unwrap();
        tx.send(CommandEvent::Terminated(
            tauri_plugin_shell::process::TerminatedPayload {
                code: output.status.code(),
                signal: None,
            },
        ))
        .await
        .unwrap();
        drop(tx);
        observe_process(
            rx,
            backend.lifecycle.clone(),
            backend.child.clone(),
            generation,
            operation,
            true,
        )
        .await;
    });
    let report = value(&backend);
    assert_eq!(report["state"], "disconnected");
    assert!(
        report["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["exitCode"] == 7)
    );
    assert!(!report.to_string().contains("sentinel"));
    assert!(backend.child.lock().unwrap().is_none());
    let mut restored = backend.state().unwrap().connection.clone().unwrap();
    restored.instance_id = uuid::Uuid::new_v4().to_string();
    restored.generation = uuid::Uuid::new_v4().to_string();
    backend.state().unwrap().install(restored).unwrap();
    tauri::async_runtime::block_on(async {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        tx.send(CommandEvent::Terminated(
            tauri_plugin_shell::process::TerminatedPayload {
                code: Some(8),
                signal: None,
            },
        ))
        .await
        .unwrap();
        drop(tx);
        observe_process(
            rx,
            backend.lifecycle.clone(),
            backend.child.clone(),
            String::new(),
            uuid::Uuid::new_v4().to_string(),
            false,
        )
        .await;
    });
    assert_eq!(
        value(&backend)["state"],
        "connected",
        "a discarded startup child must not disconnect an adopted service"
    );
    assert_eq!(value(&backend)["events"][0]["phase"], "startup");
}
