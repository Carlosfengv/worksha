use super::*;
use crate::domain::{self as d, requests::RequestUnit};
use serde_json::json;

fn fixture() -> (Info, q::UnifiedRequestItem) {
    let item: q::UnifiedRequestItem = serde_json::from_value(json!({
        "record":{"id":"response-1","sourceId":"external-fixture","unit":"model-response",
            "sessionTitle":"  ＝HYPERLINK(\"owned,fixture\")\nsecond line", "providerKey":"external",
            "providerName":"External", "outcome":"success", "usageMissing":"0",
            "usage":{"inputTokens":"9007199254740993","outputTokens":"2","totalTokens":"9007199254740995"}},
        "evidence":"source-reported","reportedResponseId":"unverified-response","reportedDurationMs":"12",
        "evidenceDetail":{"matching":{"kind":"source-reported"},"billing":null,"overflowFields":[],"timingNote":null}
    })).unwrap();
    let info = Info {
        api_version: super::super::API_VERSION.into(),
        operation_id: uuid::Uuid::new_v4().to_string(),
        snapshot_token: "frozen-selection".into(),
        count: 1_u64.into(),
        as_of: chrono::Utc::now().to_rfc3339(),
        applied_query: q::UnifiedRequestQuery {
            scope: d::QueryScope {
                kind: d::ScopeKind::External,
                source_ids: vec!["external-fixture".into()],
            },
            unit: RequestUnit::ModelResponse,
            filter: Default::default(),
            descending: true,
            sort: None,
            include_evidence: Some(true),
            snapshot_token: None,
            cursor: None,
        },
        source_states: vec![d::SourceState {
            source_id: "external-fixture".into(),
            authorization: d::Authorization::Authorized,
            coverage: d::Coverage::Complete,
            freshness: d::Freshness::Current,
            problem: None,
        }],
        source_summaries: vec![q::RequestSourceSummary {
            source_id: "external-fixture".into(),
            unit: RequestUnit::ModelResponse,
            evidence: q::RequestEvidence::SourceReported,
            coverage: d::Coverage::Complete,
            records: Some(1_u64.into()),
            usage: item.record.usage.clone(),
            usage_missing: Some(0_u64.into()),
            attempts: None,
            unknown_time: Some(1_u64.into()),
            evidence_summary: None,
        }],
        overlap_unknown: true,
    };
    (info, item)
}

fn rows(csv: &str) -> Vec<Vec<String>> {
    let mut input = csv.trim_start_matches('\u{feff}').chars().peekable();
    let mut rows = vec![];
    while input.peek().is_some() {
        let mut row = vec![];
        loop {
            assert_eq!(input.next(), Some('"'));
            let mut cell = String::new();
            loop {
                let c = input.next().expect("complete quoted CSV cell");
                if c == '"' {
                    if input.peek() == Some(&'"') {
                        input.next();
                        cell.push('"');
                    } else {
                        break;
                    }
                } else {
                    cell.push(c);
                }
            }
            row.push(cell);
            match input.next() {
                Some(',') => {}
                Some('\r') => {
                    assert_eq!(input.next(), Some('\n'));
                    break;
                }
                other => panic!("bad CSV separator {other:?}"),
            }
        }
        rows.push(row);
    }
    rows
}

#[test]
fn unified_export_csv_retains_big_integers_unknowns_source_evidence_and_escaped_cells() {
    let (info, item) = fixture();
    let mut csv = start(&info).unwrap();
    append(&mut csv, &info, &item, 16 * 1024 * 1024).unwrap();
    let rows = rows(&csv);
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|r| r.len() == 37));
    assert_eq!(rows[1][0], "scope");
    let metadata: serde_json::Value = serde_json::from_str(&rows[1][1]).unwrap();
    assert_eq!(metadata["count"], "1");
    assert_eq!(metadata["overlapUnknown"], true);
    let field = |name: &str| &rows[2][rows[0].iter().position(|v| v == name).unwrap()];
    assert_eq!(field("输入Token"), "9007199254740993");
    assert_eq!(field("总Token"), "9007199254740995");
    assert_eq!(field("已记录尝试次数"), "未知");
    assert_eq!(field("宿主记录耗时ms"), "未知");
    assert_eq!(field("来源报告耗时ms"), "12");
    assert_eq!(field("证据来源"), "source-reported");
    assert_eq!(
        field("会话标题"),
        &format!("'\t{}", item.record.session_title.unwrap())
    );
    assert_eq!(field("冻结实际费用(JSON)"), "未知");
}

#[test]
fn unified_export_csv_empty_readable_range_keeps_unavailable_and_unknown_source_totals() {
    let (mut info, _) = fixture();
    info.count = 0_u64.into();
    info.source_states[0].authorization = d::Authorization::Required;
    info.source_states[0].coverage = d::Coverage::Unavailable;
    info.source_states[0].freshness = d::Freshness::Unread;
    info.source_summaries[0].coverage = d::Coverage::Unavailable;
    info.source_summaries[0].records = None;
    info.source_summaries[0].usage = serde_json::from_value(json!({})).unwrap();
    info.source_summaries[0].usage_missing = None;
    info.source_summaries[0].unknown_time = None;
    let parsed = rows(&start(&info).unwrap());
    assert_eq!(parsed.len(), 2);
    let m: serde_json::Value = serde_json::from_str(&parsed[1][1]).unwrap();
    assert_eq!(m["count"], "0");
    assert!(m["sourceSummaries"][0]["records"].is_null());
    assert_eq!(m["sourceStates"][0]["coverage"], "unavailable");
}

#[test]
fn unified_export_csv_does_not_expand_units_sources_or_host_evidence() {
    let (info, mut item) = fixture();
    let mut csv = String::from("retained-prefix");
    item.record.attempts = Some(1_u64.into());
    assert!(append(&mut csv, &info, &item, 4096).is_err());
    assert_eq!(csv, "retained-prefix");
    item.record.attempts = None;
    item.record.source_id = "foreign".into();
    assert!(append(&mut csv, &info, &item, 4096).is_err());
    item.record.source_id = "external-fixture".into();
    item.record.unit = RequestUnit::LogicalRequest;
    assert!(append(&mut csv, &info, &item, 4096).is_err());
    assert_eq!(csv, "retained-prefix");
}

#[test]
fn unified_export_csv_keeps_actual_estimated_currencies_and_recorded_timing_separate() {
    let (mut info, mut item) = fixture();
    info.applied_query.scope.kind = d::ScopeKind::Builtin;
    info.applied_query.scope.source_ids = vec!["claude".into()];
    info.applied_query.unit = RequestUnit::LogicalRequest;
    info.source_states[0].source_id = "claude".into();
    info.source_summaries[0].source_id = "claude".into();
    info.source_summaries[0].unit = RequestUnit::LogicalRequest;
    info.source_summaries[0].evidence = q::RequestEvidence::HostObserved;
    item.record.source_id = "claude".into();
    item.record.unit = RequestUnit::LogicalRequest;
    item.record.attempts = Some(1_u64.into());
    item.record.duration_ms = Some(1.25);
    item.evidence = q::RequestEvidence::HostObserved;
    item.reported_response_id = None;
    item.reported_duration_ms = None;
    item.evidence_detail = Some(serde_json::from_value(json!({
        "matching":{"kind":"gateway-request","matchedResponses":"0","links":[],"linksLimited":false},
        "billing":{"actual":[{"currency":"USD","amount":"0.10","parts":"1"},{"currency":"EUR","amount":"2.50","parts":"1"}],
            "estimated":[{"currency":"USD","amount":"0.20","parts":"1"}],"unknownParts":"2"},
        "overflowFields":[],"timingNote":"host timing"
    })).unwrap());
    let mut csv = start(&info).unwrap();
    append(&mut csv, &info, &item, 8192).unwrap();
    let parsed = rows(&csv);
    let cell = |name: &str| &parsed[2][parsed[0].iter().position(|v| v == name).unwrap()];
    let actual: serde_json::Value = serde_json::from_str(cell("冻结实际费用(JSON)")).unwrap();
    let estimated: serde_json::Value = serde_json::from_str(cell("冻结估算费用(JSON)")).unwrap();
    assert_eq!(actual[0]["amount"], "0.10");
    assert_eq!(actual[1]["currency"], "EUR");
    assert_eq!(estimated[0]["amount"], "0.20");
    assert_eq!(cell("费用未知部分数"), "2");
    assert_eq!(cell("宿主记录耗时ms"), "1.25");
    assert_eq!(cell("来源报告耗时ms"), "未知");
    item.record.duration_ms = Some(f64::NAN);
    let before = csv.clone();
    assert_eq!(
        append(&mut csv, &info, &item, 8192),
        Err(ExportError::Unavailable)
    );
    assert_eq!(csv, before);
}

#[test]
fn unified_export_csv_bounds_each_row_and_scope_metadata_without_partial_appends() {
    let (mut info, item) = fixture();
    let mut csv = String::from("retained-prefix");
    assert_eq!(
        append(&mut csv, &info, &item, 20),
        Err(ExportError::RecordTooLarge)
    );
    assert_eq!(csv, "retained-prefix");
    info.source_states[0].problem = Some("x".repeat(EXPORT_TARGET_BYTES));
    assert_eq!(start(&info), Err(ExportError::RecordTooLarge));
    for formula in [
        "=1+1",
        "+1",
        "-1",
        "@SUM(1)",
        "＝1",
        "＋1",
        "－1",
        "＠1",
        "\u{feff}=1",
        "\tvalue",
        "\rvalue",
        "\nvalue",
    ] {
        assert!(guarded(formula));
    }
    assert!(!guarded("9007199254740993"));
}
