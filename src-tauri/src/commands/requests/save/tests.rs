use super::*;
use crate::backend::Backend;
use std::sync::{Arc, Mutex};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt},
    net::TcpListener,
    sync::Notify,
};

#[derive(Clone, Copy)]
enum Mode {
    Good,
    Large,
    BadOffset,
    RejectFinish,
    HoldSecond,
    ChangedReceipt,
}
struct Fixture {
    mode: Mode,
    unified: bool,
    requests: Mutex<Vec<String>>,
    info: Mutex<Option<ExportInfo>>,
    blocked: Notify,
    release: Notify,
}
struct Server {
    endpoint: String,
    fixture: Arc<Fixture>,
    task: tokio::task::JoinHandle<()>,
}

#[tokio::test]
async fn captured_export_context_cannot_start_on_a_replacement_backend() {
    let a = server(Mode::Good).await;
    let b = server(Mode::Good).await;
    let backend = Backend::test_connection(a.endpoint.clone());
    let context = backend.context().unwrap();
    backend.test_install_connection(b.endpoint.clone());
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("requests.csv");
    std::fs::write(&target, "original").unwrap();
    let error: serde_json::Value = serde_json::from_str(
        &export_to(&context, "snapshot".into(), target.clone())
            .await
            .unwrap_err(),
    )
    .unwrap();
    assert_eq!(error["code"], "export_not_saved");
    assert_eq!(error["outcomeUnknown"], false);
    assert!(error["message"].as_str().unwrap().contains("连接已变化"));
    assert_eq!(std::fs::read_to_string(target).unwrap(), "original");
    assert!(a.fixture.requests.lock().unwrap().is_empty());
    assert!(b.fixture.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn reconnect_during_export_stops_publication_and_cancels_only_the_original_export() {
    let a = server(Mode::HoldSecond).await;
    let b = server(Mode::Good).await;
    let backend = Backend::test_connection(a.endpoint.clone());
    let context = backend.context().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("requests.csv");
    std::fs::write(&target, "original").unwrap();
    let copy = target.clone();
    let task = tokio::spawn(async move { export_to(&context, "snapshot".into(), copy).await });
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        a.fixture.blocked.notified(),
    )
    .await
    .unwrap();
    backend.test_install_connection(b.endpoint.clone());
    a.fixture.release.notify_one();
    let error: serde_json::Value = serde_json::from_str(&task.await.unwrap().unwrap_err()).unwrap();
    assert_eq!(error["code"], "export_not_saved");
    assert_eq!(error["outcomeUnknown"], false);
    assert!(error["message"].as_str().unwrap().contains("连接已变化"));
    cancelled(&a).await;
    assert_eq!(std::fs::read_to_string(target).unwrap(), "original");
    assert_only_target(dir.path());
    assert!(b.fixture.requests.lock().unwrap().is_empty());
}
impl Drop for Server {
    fn drop(&mut self) {
        self.fixture.release.notify_waiters();
        self.task.abort();
    }
}
async fn server(mode: Mode) -> Server {
    protocol_server(mode, false).await
}
async fn protocol_server(mode: Mode, unified: bool) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let fixture = Arc::new(Fixture {
        mode,
        unified,
        requests: Mutex::new(vec![]),
        info: Mutex::new(None),
        blocked: Notify::new(),
        release: Notify::new(),
    });
    let state = fixture.clone();
    let task = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let state = state.clone();
            tokio::spawn(async move {
                let mut reader = tokio::io::BufReader::new(&mut stream);
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                let first = line.trim().to_string();
                let mut length = 0;
                let mut authorized = false;
                loop {
                    line.clear();
                    reader.read_line(&mut line).await.unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap();
                    }
                    authorized |= line.to_ascii_lowercase().trim()
                        == "authorization: bearer export-fixture-token";
                }
                assert!(authorized);
                let mut bytes = vec![0; length];
                reader.read_exact(&mut bytes).await.unwrap();
                drop(reader);
                state.requests.lock().unwrap().push(first.clone());
                let mut status = "200 OK";
                let response = if first.starts_with("POST /api/request-exports HTTP")
                    || first.starts_with("POST /api/domain/request-exports HTTP")
                {
                    let input: BeginExport = if state.unified {
                        let input: x::Begin = serde_json::from_slice(&bytes).unwrap();
                        BeginExport {
                            operation_id: input.operation_id,
                            snapshot: input.snapshot_token,
                        }
                    } else {
                        serde_json::from_slice(&bytes).unwrap()
                    };
                    let info = ExportInfo {
                        operation_id: input.operation_id,
                        snapshot: input.snapshot,
                        count: 3,
                        as_of: "2026-10-27T00:00:00Z".into(),
                    };
                    *state.info.lock().unwrap() = Some(info.clone());
                    if state.unified {
                        serde_json::json!(unified_info(info))
                    } else {
                        serde_json::json!(info)
                    }
                } else if first.starts_with("DELETE ") {
                    serde_json::json!({"status":"cancelled"})
                } else if first.contains("/finish ") {
                    let input: FinishExport = if state.unified {
                        let input: x::Finish = serde_json::from_slice(&bytes).unwrap();
                        FinishExport {
                            after: input.next_offset.value() as i64 - 1,
                            rows: input.rows.value(),
                        }
                    } else {
                        serde_json::from_slice(&bytes).unwrap()
                    };
                    assert_eq!(input.rows, 3);
                    assert_eq!(input.after, 2);
                    if matches!(state.mode, Mode::RejectFinish) {
                        status = "409 Conflict";
                        serde_json::json!({"code":"stale_snapshot","message":"fixture revoked"})
                    } else {
                        let info = state.info.lock().unwrap().clone().unwrap();
                        if state.unified {
                            let mut receipt = unified_info(info);
                            if matches!(state.mode, Mode::ChangedReceipt) {
                                receipt.source_states[0].coverage =
                                    local_agent_core::domain::Coverage::Partial;
                            }
                            serde_json::json!(receipt)
                        } else {
                            serde_json::json!(info)
                        }
                    }
                } else {
                    let after = if first.contains("after=-1 ") || first.contains("offset=0 ") {
                        -1
                    } else {
                        1
                    };
                    if after == 1 && matches!(state.mode, Mode::HoldSecond) {
                        state.blocked.notify_one();
                        state.release.notified().await;
                    }
                    let operation = state
                        .info
                        .lock()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .operation_id
                        .clone();
                    let chunk = ExportChunk {
                        operation_id: operation,
                        after,
                        rows: if after == -1 { 2 } else { 1 },
                        next_after: if after == -1 {
                            Some(if matches!(state.mode, Mode::BadOffset) {
                                0
                            } else {
                                1
                            })
                        } else {
                            None
                        },
                        csv: if matches!(state.mode, Mode::Large) {
                            format!("{}\r\n", "x".repeat(2 * 1024 * 1024))
                        } else if after == -1 {
                            "\u{feff}id\r\nfirst\r\nsecond\r\n".into()
                        } else {
                            "third\r\n".into()
                        },
                    };
                    if state.unified {
                        serde_json::json!(x::Chunk {
                            operation_id: chunk.operation_id,
                            offset: ((after + 1) as u64).into(),
                            next_offset: chunk.next_after.map(|v| ((v + 1) as u64).into()),
                            rows: chunk.rows.into(),
                            csv: chunk.csv
                        })
                    } else {
                        serde_json::json!(chunk)
                    }
                };
                let body = serde_json::to_vec(&response).unwrap();
                let headers = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(headers.as_bytes()).await;
                let _ = stream.write_all(&body).await;
            });
        }
    });
    Server {
        endpoint,
        fixture,
        task,
    }
}
fn unified_info(info: ExportInfo) -> x::Info {
    serde_json::from_value(serde_json::json!({
        "apiVersion":"1.0.0", "operationId":info.operation_id, "snapshotToken":info.snapshot, "count":info.count.to_string(), "asOf":info.as_of,
        "appliedQuery":{"scope":{"kind":"builtin","sourceIds":["claude"]},"unit":"model-response","filter":{},"descending":true,"includeEvidence":true},
        "sourceStates":[{"sourceId":"claude","authorization":"authorized","freshness":"current","coverage":"complete","problem":null}],
        "sourceSummaries":[{"sourceId":"claude","unit":"model-response","evidence":"host-observed","coverage":"complete","records":info.count.to_string(),"usage":{}}], "overlapUnknown":true,
    })).unwrap()
}

#[tokio::test]
async fn unified_native_export_shares_safe_writer_and_returns_exact_frozen_receipt() {
    let server = protocol_server(Mode::Good, true).await;
    let backend = Backend::test_connection(server.endpoint.clone());
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("requests.csv");
    let result = export_unified_to(
        &backend.context().unwrap(),
        "snapshot".into(),
        target.clone(),
    )
    .await
    .unwrap();
    let reply: x::SaveReply = serde_json::from_value(result).unwrap();
    let x::SaveReply::Saved { info, file_name } = reply else {
        panic!("expected saved");
    };
    assert_eq!(
        *info,
        unified_info(server.fixture.info.lock().unwrap().clone().unwrap())
    );
    assert_eq!(file_name.as_deref(), Some("requests.csv"));
    assert_eq!(
        std::fs::read_to_string(target).unwrap(),
        "\u{feff}id\r\nfirst\r\nsecond\r\nthird\r\n"
    );
    assert_only_target(dir.path());
    assert!(
        server
            .fixture
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|p| p.contains("/api/domain/request-exports"))
    );
}

#[tokio::test]
async fn unified_native_export_rejects_invalid_order_revoke_and_changed_scope_without_replacing_target()
 {
    for mode in [Mode::BadOffset, Mode::RejectFinish, Mode::ChangedReceipt] {
        let server = protocol_server(mode, true).await;
        let backend = Backend::test_connection(server.endpoint.clone());
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("requests.csv");
        std::fs::write(&target, "original").unwrap();
        let error: serde_json::Value = serde_json::from_str(
            &export_unified_to(
                &backend.context().unwrap(),
                "snapshot".into(),
                target.clone(),
            )
            .await
            .unwrap_err(),
        )
        .unwrap();
        assert_eq!(error["code"], "export_not_saved");
        assert_eq!(error["outcomeUnknown"], false);
        assert_eq!(std::fs::read_to_string(target).unwrap(), "original");
        assert_only_target(dir.path());
        cancelled(&server).await;
        assert!(
            server
                .fixture
                .requests
                .lock()
                .unwrap()
                .iter()
                .any(|p| p.starts_with("DELETE /api/domain/request-exports/"))
        );
    }
}

#[tokio::test]
async fn unified_native_export_cancels_only_original_connection_when_backend_changes() {
    let a = protocol_server(Mode::HoldSecond, true).await;
    let b = protocol_server(Mode::Good, true).await;
    let backend = Backend::test_connection(a.endpoint.clone());
    let context = backend.context().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("requests.csv");
    std::fs::write(&target, "original").unwrap();
    let copy = target.clone();
    let task =
        tokio::spawn(async move { export_unified_to(&context, "snapshot".into(), copy).await });
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        a.fixture.blocked.notified(),
    )
    .await
    .unwrap();
    backend.test_install_connection(b.endpoint.clone());
    a.fixture.release.notify_one();
    assert!(task.await.unwrap().is_err());
    cancelled(&a).await;
    assert_eq!(std::fs::read_to_string(target).unwrap(), "original");
    assert_only_target(dir.path());
    assert!(b.fixture.requests.lock().unwrap().is_empty());
}
fn assert_only_target(directory: &std::path::Path) {
    let files: Vec<_> = std::fs::read_dir(directory)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(files, vec![std::ffi::OsString::from("requests.csv")]);
}
async fn cancelled(server: &Server) {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while !server
            .fixture
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.starts_with("DELETE "))
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn native_export_writes_all_batches_and_only_publishes_after_final_confirmation() {
    let server = server(Mode::Good).await;
    let backend = Backend::test_connection(server.endpoint.clone());
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("requests.csv");
    std::fs::write(&target, "original").unwrap();
    let result = export_to(
        &backend.context().unwrap(),
        "snapshot".into(),
        target.clone(),
    )
    .await
    .unwrap();
    assert_eq!(result["status"], "saved");
    assert_eq!(result["count"], 3);
    assert_eq!(result["fileName"], "requests.csv");
    assert_eq!(
        std::fs::read_to_string(target).unwrap(),
        "\u{feff}id\r\nfirst\r\nsecond\r\nthird\r\n"
    );
    assert_only_target(dir.path());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(dir.path().join("requests.csv"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    assert_eq!(server.fixture.requests.lock().unwrap().len(), 4);
}
#[tokio::test]
async fn invalid_batch_or_revoked_final_confirmation_preserves_target_and_removes_temporary_data() {
    for mode in [Mode::BadOffset, Mode::RejectFinish] {
        let server = server(mode).await;
        let backend = Backend::test_connection(server.endpoint.clone());
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("requests.csv");
        std::fs::write(&target, "original").unwrap();
        let error = export_to(
            &backend.context().unwrap(),
            "snapshot".into(),
            target.clone(),
        )
        .await
        .unwrap_err();
        let problem: serde_json::Value = serde_json::from_str(&error).unwrap();
        assert_eq!(problem["code"], "export_not_saved");
        assert_eq!(problem["outcomeUnknown"], false);
        assert_eq!(std::fs::read_to_string(target).unwrap(), "original");
        assert_only_target(dir.path());
        cancelled(&server).await;
    }
}
#[tokio::test]
async fn publication_failure_and_unwritable_destination_release_the_export() {
    for missing in [false, true] {
        let server = server(Mode::Good).await;
        let backend = Backend::test_connection(server.endpoint.clone());
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(if missing {
            "missing/requests.csv"
        } else {
            "requests.csv"
        });
        if !missing {
            std::fs::create_dir(&target).unwrap();
        }
        assert!(
            export_to(&backend.context().unwrap(), "snapshot".into(), target)
                .await
                .is_err()
        );
        if missing {
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
            cancelled(&server).await;
        } else {
            assert_only_target(dir.path());
        }
    }
}
#[tokio::test]
async fn cancellation_during_transfer_removes_partial_data_and_cancels_the_service_resource() {
    let server = server(Mode::HoldSecond).await;
    let backend = Arc::new(Backend::test_connection(server.endpoint.clone()));
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("requests.csv");
    std::fs::write(&target, "original").unwrap();
    let path = target.clone();
    let task = tokio::spawn(async move {
        export_to(&backend.context().unwrap(), "snapshot".into(), path).await
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        server.fixture.blocked.notified(),
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(std::fs::read_to_string(target).unwrap(), "original");
    assert_only_target(dir.path());
    cancelled(&server).await;
    server.fixture.release.notify_one();
}

#[tokio::test]
#[ignore = "requires the disposable filesystem created by scripts/test-export-disk-full.py"]
async fn real_disk_full_removes_partial_data_and_preserves_the_selected_target() {
    let directory = PathBuf::from(
        std::env::var_os("EXPORT_TEST_FULL_DIR").expect("run the isolated disk-full script"),
    );
    let marker = std::env::var("EXPORT_TEST_FULL_MARKER").unwrap();
    assert_eq!(
        std::fs::read_to_string(directory.join(".local-agent-export-fixture")).unwrap(),
        marker
    );
    let server = server(Mode::Large).await;
    let backend = Backend::test_connection(server.endpoint.clone());
    let target = directory.join("requests.csv");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "original");
    let error = export_to(
        &backend.context().unwrap(),
        "snapshot".into(),
        target.clone(),
    )
    .await
    .unwrap_err();
    assert!(error.contains("写入失败"), "{error}");
    assert_eq!(std::fs::read_to_string(target).unwrap(), "original");
    assert!(!std::fs::read_dir(directory).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".requests-")
    }));
    assert!(
        server
            .fixture
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.contains("after=-1")),
        "the test must reach the file write, not fail while creating a temporary file"
    );
    assert!(
        !server
            .fixture
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.contains("/finish"))
    );
    cancelled(&server).await;
}
