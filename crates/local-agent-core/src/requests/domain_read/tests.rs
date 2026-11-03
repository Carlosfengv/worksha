use super::*;
use crate::{
    observation::{ObservationStore, sql::History},
    requests::{self, NativeRecord},
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize};

fn gateway(id: &str) -> Request {
    let mut row = crate::requests::tests::row(id);
    row.client = Some(crate::observation::GatewayClient {
        id: "client".into(),
        name: "Fixture".into(),
        peer: "private-peer-sentinel".into(),
    });
    requests::logical_requests(vec![row], &HashMap::new())
        .pop()
        .unwrap()
}
fn native(id: &str) -> Request {
    let mut row = gateway(id);
    row.source = "native".into();
    row.native = Some(NativeRecord {
        failure: None,
        event_id: id.into(),
        turn_id: "private-turn".into(),
        response_id: Some("response".into()),
        reported_status: "recorded".into(),
        association: "unlinked".into(),
        gateway_request_id: None,
        matched_attempt_id: None,
        usage_conflict: false,
    });
    row.client = None;
    row.attempts.clear();
    row.session_id = Some("codex:session".into());
    row.session_title = Some("Native fixture".into());
    row.status = "unknown".into();
    row.duration_ms = None;
    row.ttft_ms = None;
    row.throughput = None;
    row.cost = crate::provider_cost::Totals::default();
    row.cost.add(&crate::provider_cost::Cost::unknown(
        "native_price_unconfirmed",
    ));
    row
}
#[test]
fn builtin_projection_preserves_complete_private_frames_and_decimal_precision() {
    for mut row in [gateway("gateway"), native("native")] {
        row.usage.input_tokens = Some(9_007_199_254_740_993);
        row.usage.total_tokens = None;
        let original = serde_json::to_value(&row).unwrap();
        let public = observation(
            &row,
            if row.source == "native" {
                r::RequestUnit::ModelResponse
            } else {
                r::RequestUnit::LogicalRequest
            },
        );
        let encoded = serde_json::to_string(&public).unwrap();
        assert!(encoded.contains("9007199254740993"));
        for private in [
            "endpoint",
            "routing",
            "private-peer-sentinel",
            "private-turn",
            "responseId",
            "httpStatus",
            "cost",
        ] {
            assert!(!encoded.contains(private), "{private}");
        }
        let actual = RequestProjection::default().project(row).unwrap();
        assert_eq!(serde_json::to_value(&actual).unwrap(), original);
        assert_eq!(
            public.usage.input_tokens.unwrap().value(),
            9_007_199_254_740_993
        );
        assert!(public.usage.total_tokens.is_none());
    }
}
#[test]
fn native_unit_retains_attached_host_attempts_and_confirmed_zero_timing_and_money() {
    let mut observed = crate::requests::tests::row("attached");
    observed.endpoint = "https://openrouter.ai/api/v1".into();
    observed.reported_cost = Some(
        serde_json::from_value(
            json!({"amount":0,"currency":"USD","source":"openrouter.usage.cost"}),
        )
        .unwrap(),
    );
    let gateway = requests::logical_requests(vec![observed], &HashMap::new())
        .pop()
        .unwrap();
    let mut row = native("native");
    row.attempts = gateway.attempts;
    row.cost = gateway.cost;
    row.ttft_ms = Some(0.0);
    row.duration_ms = Some(1000.0);
    let expected = serde_json::to_value(&row).unwrap();
    assert_eq!(expected["cost"]["actual"][0]["amount"], 0.0);
    let public = observation(&row, r::RequestUnit::ModelResponse);
    assert_eq!(public.attempts.value(), 1);
    assert_eq!(public.unit, r::RequestUnit::ModelResponse);
    assert_eq!(public.ttft_ms, Some(0.0));
    assert_eq!(
        serde_json::to_value(RequestProjection::default().project(row).unwrap()).unwrap(),
        expected
    );
}
struct Labels(Arc<AtomicUsize>);
impl RequestReader for Labels {
    fn identity(&self) -> &str {
        "test.replaceable-reader"
    }
    fn read_request(
        &self,
        source: &mut dyn r::RequestSource,
        read: &r::RequestRead,
    ) -> d::Result<r::RequestSnapshot> {
        assert!(read.implementation_identity.ends_with(self.identity()));
        let mut result = source.read_request(read)?;
        self.0.fetch_add(1, Ordering::SeqCst);
        result.record.provider_name = format!(
            "Public {} label",
            if read.unit == r::RequestUnit::ModelResponse {
                "native"
            } else {
                "gateway"
            }
        );
        Ok(result)
    }
}
#[test]
fn selected_public_readers_change_the_real_sql_search_detail_page_and_frozen_export() {
    let dir = tempfile::tempdir().unwrap();
    let reads = Arc::new(AtomicUsize::new(0));
    let projection = RequestProjection::with_readers(
        Arc::new(Labels(reads.clone())),
        Arc::new(Labels(reads.clone())),
    )
    .unwrap();
    let capabilities = projection.capabilities();
    assert!(capabilities.iter().all(|c| {
        c.provider
            .implementation_identity
            .ends_with("test.replaceable-reader")
    }));
    let mut history = History::open_with_request_projection(dir.path(), projection).unwrap();
    let mut a = crate::requests::tests::row("g1");
    a.logical_id = Some("logical-fixture".into());
    a.response_id = Some("response".into());
    a.session_id = Some("codex:session".into());
    let mut b = a.clone();
    b.id = "g2".into();
    b.response_id = Some("response2".into());
    b.started_at = "2026-10-19T01:00:02Z".into();
    b.ended_at = Some("2026-10-19T01:00:03Z".into());
    ObservationStore::open(dir.path())
        .unwrap()
        .save_batch(&[a, b])
        .unwrap();
    history.refresh(&[]).unwrap();
    history
        .sync_native(
            "native-generation".into(),
            &HashMap::new(),
            [native("native")],
        )
        .unwrap();
    let now = chrono::DateTime::parse_from_rfc3339("2026-10-19T02:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    for (source, label, attempts) in [
        ("gateway", "Public gateway label", 2),
        ("native", "Public native label", 0),
    ] {
        let q = requests::Query {
            source: Some(source.into()),
            search: Some(label.into()),
            ..Default::default()
        };
        let report = history.source_report(&q, now, vec![], None).unwrap();
        let page = history.request_page(&q, now, report, None).unwrap();
        assert_eq!(page["total"], 1, "{source}: {}", page["items"]);
        assert_eq!(page["summary"]["attempts"], attempts);
        assert_eq!(page["items"][0]["provider"], label);
        let id = page["items"][0]["id"].as_str().unwrap();
        let detail = history.request_detail(id).unwrap().unwrap();
        assert_eq!(detail["provider"], label);
        if source == "gateway" {
            assert_eq!(detail["attempts"].as_array().unwrap().len(), 2);
            assert_eq!(detail["usage"]["totalTokens"], 240);
            assert_eq!(detail["nativeLinks"][0]["eventId"], "native");
            assert_ne!(
                detail["attempts"][0]["provider"], label,
                "private attempt evidence retains original supplier"
            );
        } else {
            assert_eq!(detail["native"]["association"], "response_id");
            assert_eq!(detail["usage"]["totalTokens"], 120);
        }
        let export = history
            .frozen_csv(page["snapshot"].as_str().unwrap(), now)
            .unwrap();
        assert!(export.to_string().contains(label));
    }
    assert!(reads.load(Ordering::SeqCst) >= 2);
}
struct Tamper(usize);
impl RequestReader for Tamper {
    fn identity(&self) -> &str {
        "test.tamper"
    }
    fn read_request(
        &self,
        source: &mut dyn r::RequestSource,
        read: &r::RequestRead,
    ) -> d::Result<r::RequestSnapshot> {
        let mut s = source.read_request(read)?;
        match self.0 {
            0 => s.read.read_revision.push('x'),
            1 => s.record.id.push('x'),
            2 => s.record.source_id.push('x'),
            3 => s.record.product_id = Some("foreign".into()),
            4 => s.record.session_ref = Some("foreign".into()),
            5 => s.record.project_ref = Some("foreign".into()),
            6 => s.record.model = Some("foreign".into()),
            7 => s.record.provider_key = "foreign".into(),
            8 => s.record.outcome = "failed".into(),
            9 => s.record.usage.total_tokens = Some(0u64.into()),
            10 => s.record.usage_missing = 99u64.into(),
            11 => s.record.attempts = 99u64.into(),
            12 => s.record.duration_ms = Some(9.0),
            13 => s.record.ttft_ms = Some(9.0),
            14 => s.record.throughput = Some(9.0),
            15 => s.record.occurred_at = Some("2026-10-20T00:00:00Z".into()),
            _ => s.record.unit = r::RequestUnit::ModelResponse,
        };
        Ok(s)
    }
}
#[test]
fn a_public_result_cannot_move_private_billing_clocks_or_links_to_another_record() {
    for n in 0..17 {
        let reader = Arc::new(Tamper(n));
        let projection = RequestProjection::with_readers(reader.clone(), reader).unwrap();
        assert!(projection.project(gateway("one")).is_err(), "tamper {n}");
    }
}
#[test]
fn a_forged_read_does_not_consume_the_bound_source_and_replay_is_rejected() {
    use r::RequestSource;
    let row = gateway("one");
    let read = r::RequestRead {
        provider_key: "builtin:domain.request-source@1:gateway".into(),
        implementation_identity: "fixture".into(),
        read_revision: "one".into(),
        source_id: row.agent_id.clone(),
        record_id: row.id.clone(),
        unit: r::RequestUnit::LogicalRequest,
    };
    let mut source = PublishedRequest {
        expected: &read,
        row: &row,
        read: false,
    };
    let mut forged = read.clone();
    forged.record_id = "other".into();
    assert!(source.read_request(&forged).is_err());
    assert!(!source.read);
    let result = source.read_request(&read).unwrap();
    assert!(source.read);
    assert!(source.read_request(&read).is_err());
    let mut encoded = serde_json::to_value(result).unwrap();
    encoded["endpoint"] = json!("forbidden");
    assert!(serde_json::from_value::<r::RequestSnapshot>(encoded).is_err());
}
struct Failing;
impl RequestReader for Failing {
    fn identity(&self) -> &str {
        "test.failure"
    }
    fn read_request(
        &self,
        _source: &mut dyn r::RequestSource,
        _read: &r::RequestRead,
    ) -> d::Result<r::RequestSnapshot> {
        Err(d::Failure::new(d::ErrorCode::Unavailable))
    }
}
#[test]
fn failed_read_rolls_back_disposable_history_and_never_rewrites_original_observations() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let row = crate::requests::tests::row("persisted");
    store.save(&row).unwrap();
    let before = serde_json::to_value(store.load_all().unwrap()).unwrap();
    let projection =
        RequestProjection::with_readers(Arc::new(BuiltinReader), Arc::new(Failing)).unwrap();
    let mut history = History::open_with_request_projection(dir.path(), projection).unwrap();
    assert!(history.refresh(&[]).is_err());
    assert!(history.request_detail("persisted").unwrap().is_none());
    assert_eq!(
        serde_json::to_value(store.load_all().unwrap()).unwrap(),
        before
    );
    let mut recovered = History::open(dir.path()).unwrap();
    recovered.refresh(&[]).unwrap();
    assert_eq!(
        recovered.request_detail("persisted").unwrap().unwrap()["id"],
        "persisted"
    );
}
struct ChangesIdentity(AtomicBool);
impl RequestReader for ChangesIdentity {
    fn identity(&self) -> &str {
        if self.0.load(Ordering::SeqCst) {
            "test.replaced"
        } else {
            "test.original"
        }
    }
    fn read_request(
        &self,
        source: &mut dyn r::RequestSource,
        read: &r::RequestRead,
    ) -> d::Result<r::RequestSnapshot> {
        let result = source.read_request(read)?;
        self.0.store(true, Ordering::SeqCst);
        Ok(result)
    }
}
#[test]
fn implementation_changes_during_read_cannot_deliver_an_old_result() {
    let projection = RequestProjection::with_readers(
        Arc::new(BuiltinReader),
        Arc::new(ChangesIdentity(AtomicBool::new(false))),
    )
    .unwrap();
    assert!(projection.project(gateway("one")).is_err());
}
#[test]
fn unknown_native_time_is_not_replaced_with_host_time_or_zero_measurements() {
    let mut row = native("unknown");
    row.started_at = "unparseable legacy event".into();
    let expected = serde_json::to_value(&row).unwrap();
    let public = observation(&row, r::RequestUnit::ModelResponse);
    assert!(
        public.occurred_at.is_none() && public.ttft_ms.is_none() && public.duration_ms.is_none()
    );
    assert_eq!(
        serde_json::to_value(RequestProjection::default().project(row).unwrap()).unwrap(),
        expected
    );
    let catalog = d::CapabilityCatalog {
        api_version: d::API_VERSION.into(),
        revision: 1u64.into(),
        entries: builtin_capabilities(),
        external_problem: None,
    };
    catalog.validate().unwrap();
    assert_eq!(catalog.entries.len(), 2);
    assert_eq!(
        serde_json::to_value(&catalog).unwrap()["entries"][0]["operations"],
        json!(["read-requests"])
    );
    let _: Value = serde_json::from_str(&serde_json::to_string(&public).unwrap()).unwrap();
}
