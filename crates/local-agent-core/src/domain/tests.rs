use super::*;
use serde_json::json;

fn catalog() -> CatalogSnapshot {
    CatalogSnapshot {
        api_version: API_VERSION.into(),
        revision: 1u64.into(),
        providers: vec![Provider {
            provider_key: "org.example.reader:history".into(),
            plugin_id: "org.example.reader".into(),
            implementation_version: "1.0.0".into(),
            implementation_identity: "a".repeat(64),
            capability: Capability {
                name: "domain.session-source".into(),
                major: 1,
                instance_id: "org.example.reader:history".into(),
            },
            execution_kind: ExecutionKind::WasmComponent,
        }],
        products: vec![Product {
            id: "org.example.reader:cli".into(),
            display_name: "Example".into(),
            source_id: "org.example.reader:history".into(),
            config_id: None,
        }],
        sources: vec![Source {
            id: "org.example.reader:history".into(),
            provider_key: "org.example.reader:history".into(),
            product_ids: vec!["org.example.reader:cli".into()],
            status: ProviderStatus {
                enabled: false,
                authorization: Authorization::Required,
                grant_revision: 0u64.into(),
                runtime_active: false,
                freshness: Freshness::Unread,
            },
        }],
    }
}
fn record(id: &str) -> SessionRecord {
    SessionRecord {
        id: id.into(),
        native_id: id.into(),
        product_ids: vec![],
        unknown_client: true,
        title: "record".into(),
        created_at: None,
        updated_at: None,
        project_ref: None,
        usage: (&crate::usage::Usage::default()).into(),
        partial: false,
    }
}

#[test]
fn query_window_is_bounded_and_paginates_all_rows_with_whole_scope_totals() {
    let mut after = None;
    let mut seen = std::collections::BTreeSet::new();
    for page in 0..3 {
        let mut window = SessionWindow::new("source", after.clone());
        for n in (0..111).rev() {
            let mut s = record(&format!("record:{n:03}"));
            s.created_at = Some(
                if n == 0 {
                    "2026-10-31T08:00:00+08:00"
                } else {
                    "2026-10-31T00:00:00Z"
                }
                .into(),
            );
            s.usage.total_tokens = Some(9007199254740993u64.into());
            window.observe(s).unwrap();
        }
        let (mut rows, summary) = window.finish();
        assert!(rows.len() <= 41);
        assert_eq!(summary.sessions.value(), 111);
        assert_eq!(
            summary.usage.total_tokens.unwrap().value(),
            9007199254740993 * 111
        );
        rows.truncate(40);
        assert_eq!(rows.len(), if page < 2 { 40 } else { 31 });
        after = rows.last().map(|r| SessionOrder::of(&r.record));
        for row in rows {
            assert!(seen.insert(row.record.id));
        }
    }
    assert_eq!(seen.len(), 111);
}

#[test]
fn query_unknown_timestamps_and_usage_are_distinct_from_known_zero() {
    let unknown = record("a-unknown");
    let mut known = record("z-known");
    known.created_at = Some("2026-10-31T00:00:00Z".into());
    known.usage.total_tokens = Some(0u64.into());
    assert!(SessionOrder::of(&known) < SessionOrder::of(&unknown));
    let mut window = SessionWindow::new("source", None);
    window.observe(unknown).unwrap();
    let (_, summary) = window.finish();
    assert_eq!(summary.usage.total_tokens, None);
    assert_eq!(summary.known_usage_sessions.value(), 0);
    let mut window = SessionWindow::new("source", None);
    window.observe(known).unwrap();
    let (_, summary) = window.finish();
    assert_eq!(summary.usage.total_tokens.unwrap().value(), 0);
    assert_eq!(summary.known_usage_sessions.value(), 1);
}

#[test]
fn query_rejects_unknown_fields_numeric_counts_overflow_and_invalid_filters() {
    let mut s = record("one");
    s.usage.total_tokens = Some(u64::MAX.into());
    let mut window = SessionWindow::new("source", None);
    window.observe(s.clone()).unwrap();
    s.id = "two".into();
    assert_eq!(
        window.observe(s).unwrap_err().code,
        ErrorCode::ResourceLimit
    );
    assert!(
        SessionFilter {
            days: Some(2),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    assert!(serde_json::from_value::<UnifiedSessionQuery>(json!({"scope":{"kind":"external","sourceIds":[]},"filter":{},"snapshotToken":null,"cursor":null,"method":"raw"})).is_err());
}
fn batch(n: u64, id: &str) -> SourceBatch {
    SourceBatch {
        source_id: "source".into(),
        source_revision: "revision".into(),
        batch_id: n.into(),
        records: vec![record(id)],
    }
}
fn checkpoint(batches: u64, records: u64) -> Checkpoint {
    Checkpoint {
        source_revision: "revision".into(),
        batches: batches.into(),
        records: records.into(),
        complete: true,
        coverage: Coverage::Complete,
    }
}

#[test]
fn decimals_preserve_u64_and_reject_noncanonical_or_numeric_input() {
    for value in [0, 9_007_199_254_740_993, u64::MAX] {
        let encoded = serde_json::to_string(&Decimal::from(value)).unwrap();
        assert_eq!(encoded, format!("\"{value}\""));
        assert_eq!(
            serde_json::from_str::<Decimal>(&encoded).unwrap().value(),
            value
        );
    }
    for value in [
        json!(1),
        json!(null),
        json!("-1"),
        json!("01"),
        json!("+1"),
        json!("1.0"),
        json!("18446744073709551616"),
    ] {
        assert!(serde_json::from_value::<Decimal>(value).is_err());
    }
    let usage = Usage::from(&crate::usage::Usage {
        input_tokens: Some(0),
        output_tokens: None,
        ..Default::default()
    });
    assert_eq!(serde_json::to_value(usage).unwrap()["inputTokens"], "0");
}
#[test]
fn wire_dtos_roundtrip_and_reject_unknown_fields() {
    let s = catalog();
    assert_eq!(
        serde_json::from_value::<CatalogSnapshot>(serde_json::to_value(&s).unwrap()).unwrap(),
        s
    );
    let mut value = serde_json::to_value(s).unwrap();
    value["secret"] = json!("must-not-cross");
    assert!(serde_json::from_value::<CatalogSnapshot>(value).is_err());
    let b = batch(0, "one");
    assert!(
        serde_json::from_value::<crate::plugin::external::SourceBatch>(
            serde_json::to_value(&b).unwrap()
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<SourceBatch>(json!({
            "schemaVersion":1,"sourceRef":"history","sourceRevision":"revision",
            "batchId":"one","upserts":[],"nextCursor":null,"complete":true
        }))
        .is_err()
    );
    assert_eq!(
        serde_json::from_value::<SourceBatch>(serde_json::to_value(&b).unwrap()).unwrap(),
        b
    );
    let mut v = serde_json::to_value(&b).unwrap();
    v["records"][0]["path"] = json!("/private");
    assert!(serde_json::from_value::<SourceBatch>(v).is_err());
}
#[test]
fn catalog_publication_is_validated_immutable_and_compare_and_swap() {
    let registry = Catalog::new(catalog()).unwrap();
    let old = registry.snapshot().unwrap();
    let mut invalid = (*old).clone();
    invalid.revision = 2u64.into();
    invalid.products[0].id = "claude-code-cli".into();
    assert!(registry.replace(&old.revision, invalid).is_err());
    assert_eq!(registry.snapshot().unwrap().revision.value(), 1);
    let mut next = (*old).clone();
    next.revision = 2u64.into();
    next.sources[0].status.enabled = true;
    registry.replace(&old.revision, next.clone()).unwrap();
    assert!(!old.sources[0].status.enabled);
    assert!(registry.replace(&old.revision, next).is_err());
    assert!(registry.snapshot().unwrap().sources[0].status.enabled);
}
#[test]
fn namespace_and_capability_collisions_cannot_claim_builtin_identity() {
    let mut c = catalog();
    c.providers.push(c.providers[0].clone());
    c.providers[1].provider_key = "different-key".into();
    assert_eq!(
        c.validate().unwrap_err().code,
        ErrorCode::CapabilityConflict
    );
    let mut c = catalog();
    c.sources[0].id = "claude".into();
    assert!(c.validate().is_err());
    let mut c = catalog();
    c.api_version = "2.0.0".into();
    assert_eq!(c.validate().unwrap_err().code, ErrorCode::IncompatibleApi);
}
#[test]
fn incomplete_duplicate_changed_or_oversized_batches_never_publish() {
    let mut p = source::ScanPublication::new("source");
    p.accept(batch(0, "one")).unwrap();
    assert!(p.finish(&checkpoint(2, 2)).is_err());
    let mut p = source::ScanPublication::new("source");
    p.accept(batch(0, "one")).unwrap();
    assert!(p.accept(batch(1, "one")).is_err());
    assert!(p.finish(&checkpoint(1, 1)).is_err());
    let mut p = source::ScanPublication::new("source");
    p.accept(batch(0, "one")).unwrap();
    let mut wrong = batch(1, "two");
    wrong.source_revision = "changed".into();
    assert!(p.accept(wrong).is_err());
    assert!(p.finish(&checkpoint(1, 1)).is_err());
    let mut p = source::ScanPublication::with_limit("source", 0);
    assert_eq!(
        p.accept(batch(0, "one")).unwrap_err().code,
        ErrorCode::ResourceLimit
    );
    let mut p = source::ScanPublication::new("source");
    let mut large = batch(0, "one");
    large.records = (0..257).map(|n| record(&n.to_string())).collect();
    assert!(p.accept(large).is_err());
}
#[test]
fn confirmed_empty_and_partial_are_separate_from_an_unfinished_scan() {
    source::ScanPublication::new("source")
        .finish(&checkpoint(0, 0))
        .unwrap();
    let mut c = checkpoint(0, 0);
    c.complete = false;
    assert!(source::ScanPublication::new("source").finish(&c).is_err());
    c.complete = true;
    c.coverage = Coverage::Partial;
    source::ScanPublication::new("source").finish(&c).unwrap();
    c.coverage = Coverage::Unavailable;
    assert_eq!(
        source::ScanPublication::new("source")
            .finish(&c)
            .unwrap_err()
            .code,
        ErrorCode::Unavailable
    );
}
#[test]
fn generated_schemas_match_the_checked_in_contract() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contracts/domain");
    for (name, value) in schemas().as_object().unwrap() {
        let actual: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.join(format!("{name}.schema.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(&actual, value, "domain schema drift: {name}");
    }
}
