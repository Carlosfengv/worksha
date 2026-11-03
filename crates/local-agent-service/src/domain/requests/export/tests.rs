use super::super::tests::{Fixture, input};
use super::*;
use serde_json::json;

fn start(state: &AppState) -> (x::Begin, x::Info) {
    let mut query = input();
    query.include_evidence = Some(true);
    let page = super::super::query(state, query).unwrap();
    let begin = x::Begin {
        operation_id: uuid::Uuid::new_v4().to_string(),
        snapshot_token: page.snapshot_token,
    };
    let info = serde_json::from_value(
        run(
            state,
            &begin.operation_id,
            Action::Begin(begin.clone()),
            &|| {},
        )
        .unwrap(),
    )
    .unwrap();
    (begin, info)
}

#[tokio::test]
async fn unified_export_service_reads_metadata_without_refresh_or_body_and_finishes_exact_scope() {
    let fixture = Fixture::new().await;
    let state = &fixture.state;
    let (begin, info) = start(state);
    assert_eq!(info.count.value(), 1);
    let revision = state.sessions.domain_revision().unwrap();
    let part: x::Chunk = serde_json::from_value(
        run(
            state,
            &begin.operation_id,
            Action::Read(x::Read {
                offset: 0u64.into(),
            }),
            &|| {},
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(part.rows.value(), 1);
    assert_eq!(part.next_offset, None);
    assert!(part.csv.contains("\"13\""));
    assert!(!part.csv.contains("owned reply"));
    let receipt: x::Info = serde_json::from_value(
        run(
            state,
            &begin.operation_id,
            Action::Finish(x::Finish {
                next_offset: 1u64.into(),
                rows: 1u64.into(),
            }),
            &|| {},
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(receipt, info);
    assert_eq!(state.sessions.domain_revision().unwrap(), revision);
    assert!(
        run(
            state,
            &begin.operation_id,
            Action::Read(x::Read {
                offset: 0u64.into()
            }),
            &|| {}
        )
        .is_err()
    );
}

#[tokio::test]
async fn unified_export_service_rejects_revoke_even_after_rows_were_read_or_before_finish() {
    for finish in [false, true] {
        let fixture = Fixture::new().await;
        let state = &fixture.state;
        let (begin, _) = start(state);
        let action = if finish {
            Action::Finish(x::Finish {
                next_offset: 1u64.into(),
                rows: 1u64.into(),
            })
        } else {
            Action::Read(x::Read {
                offset: 0u64.into(),
            })
        };
        let error = run(state, &begin.operation_id, action, &|| {
            state.sessions.set_enabled("claude", false).unwrap();
            state.sessions.set_enabled("claude", true).unwrap();
        })
        .unwrap_err();
        assert_eq!(error.0, StatusCode::CONFLICT);
        assert!(
            sql(state, |h| h.unified_csv_export_query(
                &begin.operation_id,
                chrono::Utc::now()
            ))
            .is_err()
        );
    }
}

#[tokio::test]
async fn unified_export_service_bad_duplicate_begin_does_not_cancel_existing_operation() {
    let fixture = Fixture::new().await;
    let state = &fixture.state;
    let (begin, _) = start(state);
    let mut wrong = begin.clone();
    wrong.snapshot_token = "foreign".into();
    assert_eq!(
        run(state, &begin.operation_id, Action::Begin(wrong), &|| {})
            .unwrap_err()
            .0,
        StatusCode::BAD_REQUEST
    );
    assert!(
        run(
            state,
            &begin.operation_id,
            Action::Read(x::Read {
                offset: 0u64.into()
            }),
            &|| {}
        )
        .is_ok()
    );
    sql(state, |h| h.cancel_unified_csv_export(&begin.operation_id)).unwrap();
}

#[tokio::test]
async fn unified_export_http_parses_decimal_offsets_strictly_and_rejects_paths() {
    let fixture = Fixture::new().await;
    let (begin, _) = start(&fixture.state);
    let app = axum::Router::new()
        .route("/exports", axum::routing::post(super::begin))
        .route("/exports/{id}", axum::routing::get(chunk).delete(cancel))
        .with_state(fixture.state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    for suffix in ["?offset=-1", "?offset=01", "?offset=0&path=foreign"] {
        assert_eq!(
            client
                .get(format!("{base}/exports/{}{suffix}", begin.operation_id))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    let response = client
        .get(format!("{base}/exports/{}?offset=0", begin.operation_id))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let part: x::Chunk = response.json().await.unwrap();
    assert_eq!(part.offset.value(), 0);
    assert_eq!(part.rows.value(), 1);
    assert_eq!(client.post(format!("{base}/exports")).json(&json!({"operationId":begin.operation_id,"snapshotToken":begin.snapshot_token,"path":"foreign"})).send().await.unwrap().status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        client
            .delete(format!("{base}/exports/{}", begin.operation_id))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    task.abort();
}
