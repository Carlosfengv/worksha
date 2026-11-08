//! Native response records retain their transcript identity. An upstream response
//! ID is the only cross-source join key; timestamps, names and token counts are not.
use super::*;
use crate::sessions::{CallSummary, SourceStatus};
use std::collections::HashSet;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeRecord {
    pub failure: Option<crate::sessions::ModelFailure>,
    pub event_id: String,
    pub turn_id: String,
    pub response_id: Option<String>,
    pub reported_status: String,
    pub association: String,
    pub gateway_request_id: Option<String>,
    pub matched_attempt_id: Option<String>,
    pub usage_conflict: bool,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeLink {
    pub event_id: String,
    pub session_id: String,
    pub response_id: String,
    pub attempt_id: String,
    pub usage_conflict: bool,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Collector {
    pub authorization_revision: u64,
    pub agent_id: String,
    pub enabled: bool,
    pub status: String,
    pub as_of: Option<String>,
    pub last_successful_scan: Option<String>,
    pub error: Option<String>,
    pub skipped_files: usize,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceReport {
    pub gateway: Summary,
    pub native: Summary,
    pub linked: Summary,
    pub matched_native: usize,
    pub unlinked_native: usize,
    pub ambiguous_native: usize,
    pub usage_conflicts: usize,
    pub native_missing_time: usize,
    pub source_as_of: Option<String>,
    pub collectors: Vec<Collector>,
    pub scope: String,
}
pub fn native_records(
    calls: impl IntoIterator<Item = CallSummary>,
    contexts: &HashMap<String, Context>,
) -> Vec<Request> {
    let mut seen = HashSet::new();
    calls
        .into_iter()
        .filter(|c| c.kind == "model" && seen.insert(c.id.clone()))
        .map(|c| native_record(c, contexts))
        .collect()
}
pub fn native_records_iter<'a>(
    calls: impl Iterator<Item = &'a CallSummary> + 'a,
    contexts: &'a HashMap<String, Context>,
) -> impl Iterator<Item = Request> + 'a {
    let mut seen = HashSet::new();
    calls
        .filter(move |c| c.kind == "model" && seen.insert(c.id.as_str()))
        .map(|c| native_record(c.clone(), contexts))
}
fn native_record(c: CallSummary, contexts: &HashMap<String, Context>) -> Request {
    let context = contexts.get(&c.session_id);
    let usage = c.usage.unwrap_or_default();
    let provider = c.provider.unwrap_or("Provider 未报告".into());
    let provider_key = format!(
        "native-provider-{:x}",
        Sha256::digest(format!("{}\0{provider}", c.agent_id))
    );
    let status = match c.status.as_str() {
        "error" | "failed" => "failed",
        "cancelled" => "cancelled",
        // A recorded response does not establish the final HTTP result.
        _ => "unknown",
    };
    Request {
        client: None,
            source: "native".into(),
            native: Some(NativeRecord {
                failure: c.failure.map(|mut failure| { failure.message = None; failure }), event_id: c.id.clone(), turn_id: c.turn_id,
                response_id: c.response_id, reported_status: c.status,
                association: "unlinked".into(), gateway_request_id: None,
                matched_attempt_id: None, usage_conflict: false }),
            native_links: vec![], id: c.id, agent_id: c.agent_id,
            agent_product_id: c.agent_product_id, session_id: Some(c.session_id),
            session_title: Some(c.session_title), project_id: context.and_then(|s| s.project_id.clone()),
            project_name: context.and_then(|s| s.project_name.clone()),
            started_at: c.timestamp.unwrap_or_default(), ended_at: None, status: status.into(),
            model: c.model, provider, provider_key, usage_missing: usize::from(usage.total_tokens.is_none()), usage,
            cost: { let mut totals = provider_cost::Totals::default(); totals.add(&provider_cost::Cost::unknown("native_price_unconfirmed")); totals },
            duration_ms: None, ttft_ms: None, throughput: None,
            timing_note: Some("原生记录时间不等于请求开始时间；未提供同一单调时钟的首出与完整耗时证据，HTTP 结果、路由及费用不据此推断。".into()),
            attempts: vec![],
        }
}
fn different_usage(a: &Usage, b: &Usage) -> bool {
    [
        a.input_tokens.zip(b.input_tokens),
        a.output_tokens.zip(b.output_tokens),
        a.cached_input_tokens.zip(b.cached_input_tokens),
        a.cache_write_input_tokens.zip(b.cache_write_input_tokens),
        a.reasoning_output_tokens.zip(b.reasoning_output_tokens),
        a.total_tokens.zip(b.total_tokens),
    ]
    .into_iter()
    .flatten()
    .any(|(a, b)| a != b)
}
pub(super) fn native_issue(r: &Request) -> Option<&str> {
    r.native
        .as_ref()?
        .failure
        .as_ref()
        .and_then(|f| match f.category.as_str() {
            "authentication" => Some("authentication"),
            "rate_limit" => Some("rate_limit"),
            "upstream" => Some("upstream_error"),
            "timeout" => Some("timeout"),
            "connection" => Some("connection"),
            "protocol" => Some("protocol"),
            _ => None,
        })
}
pub fn correlate(gateway: &mut [Request], native: &mut [Request]) {
    let mut net: HashMap<(String, String), Vec<(usize, usize)>> = HashMap::new();
    let mut local: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (ri, r) in gateway.iter().enumerate() {
        for (ai, a) in r.attempts.iter().enumerate() {
            if let Some(id) = a.observation.native_response_id() {
                net.entry((r.agent_id.clone(), id.to_owned()))
                    .or_default()
                    .push((ri, ai));
            }
        }
    }
    for (i, r) in native.iter().enumerate() {
        if let Some(id) = r.native.as_ref().and_then(|n| n.response_id.clone()) {
            local.entry((r.agent_id.clone(), id)).or_default().push(i);
        }
    }
    for (key, indices) in local {
        let Some(candidates) = net.get(&key) else {
            continue;
        };
        if indices.len() != 1 || candidates.len() != 1 {
            for i in indices {
                native[i].native.as_mut().unwrap().association = "ambiguous".into();
            }
            continue;
        }
        let n = &mut native[indices[0]];
        let (ri, ai) = candidates[0];
        let r = &mut gateway[ri];
        let product_conflict = r
            .agent_product_id
            .as_ref()
            .zip(n.agent_product_id.as_ref())
            .is_some_and(|(a, b)| a != b);
        let session_conflict = r
            .session_id
            .as_ref()
            .zip(n.session_id.as_ref())
            .is_some_and(|(a, b)| a != b);
        let record = n.native.as_mut().unwrap();
        if product_conflict || session_conflict {
            record.association = "identity_conflict".into();
            continue;
        }
        let attempt = &r.attempts[ai];
        record.association = "response_id".into();
        record.gateway_request_id = Some(r.id.clone());
        record.matched_attempt_id = Some(attempt.observation.id.clone());
        record.usage_conflict = different_usage(&attempt.observation.usage, &n.usage);
        r.native_links.push(NativeLink {
            event_id: n.id.clone(),
            session_id: n.session_id.clone().unwrap(),
            response_id: key.1,
            attempt_id: attempt.observation.id.clone(),
            usage_conflict: record.usage_conflict,
        });
    }
}
impl SourceReport {
    pub fn new(
        gateway: &[Request],
        native: &[Request],
        q: &Query,
        now: DateTime<Utc>,
        sources: Vec<SourceStatus>,
        source_as_of: Option<String>,
    ) -> Self {
        Self::new_with_directory(
            gateway,
            native,
            q,
            now,
            sources,
            source_as_of,
            crate::agents::builtin_directory(),
        )
    }
    pub fn new_with_directory(
        gateway: &[Request],
        native: &[Request],
        q: &Query,
        now: DateTime<Utc>,
        sources: Vec<SourceStatus>,
        source_as_of: Option<String>,
        directory: &crate::agents::AgentDirectory,
    ) -> Self {
        let in_scope = |r: &&Request| matches(r, q) && time(&r.started_at).is_none_or(|t| t < now);
        let g: Vec<_> = gateway.iter().filter(in_scope).collect();
        let n: Vec<_> = native.iter().filter(in_scope).collect();
        let linked: Vec<_> = g
            .iter()
            .copied()
            .filter(|r| !r.native_links.is_empty())
            .collect();
        let matched_native = n
            .iter()
            .filter(|r| {
                r.native
                    .as_ref()
                    .is_some_and(|n| n.association == "response_id")
            })
            .count();
        Self {
            gateway: Summary::of(g), native: Summary::of(n.iter().copied()), linked: Summary::of(linked),
            matched_native, unlinked_native: n.len().saturating_sub(matched_native),
            ambiguous_native: n.iter().filter(|r| r.native.as_ref().is_some_and(|n| ["ambiguous", "identity_conflict"].contains(&n.association.as_str()))).count(),
            usage_conflicts: n.iter().filter(|r| r.native.as_ref().is_some_and(|n| n.usage_conflict)).count(),
            native_missing_time: native.iter().filter(|r| time(&r.started_at).is_none() && matches(r, &Query { start: None, end: None, ..q.clone() })).count(),
            source_as_of,
            collectors: sources.into_iter().filter(|s| q.agent.as_deref().is_none_or(|a| directory.family(a).is_none_or(|f| f==s.agent_id))).map(|s| Collector {authorization_revision:s.authorization_revision, agent_id:s.agent_id, enabled:s.enabled, status:s.status, as_of:s.last_scan, last_successful_scan:s.last_successful_scan, error:s.error, skipped_files:s.skipped_files}).collect(),
            scope: "各来源按当前筛选分别统计，单位为网关逻辑请求和原生模型响应记录。跨来源精确匹配按响应 ID 一对一配对，消费与计时采用网关记录，原生字段保留为独立证据。其余原生记录可能与网关重叠，各来源独立计数和计费。".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::row;
    use super::*;
    fn call(id: &str, response: Option<&str>) -> CallSummary {
        CallSummary {
            response_id: response.map(str::to_owned),
            native_agent_product_id: None,
            agent_attribution: None,
            agent_product_id: Some("codex-cli".into()),
            id: id.into(),
            session_id: "session".into(),
            session_title: "原生记录".into(),
            agent_id: "codex".into(),
            turn_id: "turn".into(),
            kind: "model".into(),
            name: "模型响应".into(),
            status: "recorded".into(),
            timestamp: Some("2026-10-19T01:00:00Z".into()),
            model: Some("deepseek-flash".into()),
            provider: None,
            model_response_id: None,
            usage: Some(row("g").usage),
            server: None,
            tool_name: None,
            duration_ms: Some(1234),
            tool_source: None,
            failure: None,
        }
    }
    fn g(ids: &[&str]) -> Vec<Request> {
        logical_requests(
            ids.iter()
                .enumerate()
                .map(|(i, id)| {
                    let mut r = row(&format!("g{i}"));
                    r.response_id = Some((*id).into());
                    r
                })
                .collect(),
            &HashMap::new(),
        )
    }
    fn n(calls: Vec<CallSummary>) -> Vec<Request> {
        native_records(calls, &HashMap::new())
    }
    #[test]
    fn native_response_is_not_a_http_request_or_tool_count() {
        let mut tool = call("tool", None);
        tool.kind = "tool".into();
        let n = n(vec![
            call("native", Some("r")),
            call("native", Some("r")),
            tool,
        ]);
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].id, "native");
        assert!(n[0].attempts.is_empty());
        assert_eq!(n[0].status, "unknown");
        assert!(n[0].ttft_ms.is_none());
        assert!(n[0].duration_ms.is_none());
        let s = Summary::of(&n);
        assert_eq!(s.requests, 1);
        assert_eq!(s.attempts, 0);
        assert_eq!(s.cost.unknown_calls, 1);
        assert!(s.success_rate.is_none());
    }
    #[test]
    fn explicit_identity_links_once_without_adding_consumption() {
        let mut g = g(&["r"]);
        let mut n = n(vec![call("native", Some("r"))]);
        correlate(&mut g, &mut n);
        assert_eq!(g[0].native_links.len(), 1);
        assert_eq!(n[0].native.as_ref().unwrap().association, "response_id");
        assert_eq!(Summary::of(&g).usage.total_tokens, Some(120));
    }
    #[test]
    fn converted_responses_join_only_the_id_the_client_received() {
        let mut gateway = g(&["upstream"]);
        let observation = &mut gateway[0].attempts[0].observation;
        observation.protocol = "chat-completions".into();
        observation.client_protocol = Some("responses".into());
        observation.upstream_response_id = Some("upstream".into());
        observation.client_response_id = Some("client".into());
        let mut native = n(vec![
            call("correct", Some("client")),
            call("wrong", Some("upstream")),
        ]);
        correlate(&mut gateway, &mut native);
        assert_eq!(
            native[0].native.as_ref().unwrap().association,
            "response_id"
        );
        assert_eq!(native[1].native.as_ref().unwrap().association, "unlinked");
        assert_eq!(gateway[0].native_links.len(), 1);
        assert_eq!(Summary::of(&gateway).usage.total_tokens, Some(120));
        gateway[0].attempts[0].observation.client_response_id = None;
        let mut legacy = n(vec![call("legacy", Some("upstream"))]);
        correlate(&mut gateway, &mut legacy);
        assert_eq!(legacy[0].native.as_ref().unwrap().association, "unlinked");
    }
    #[test]
    fn similar_time_model_and_tokens_cannot_establish_a_link() {
        let mut g = g(&["r"]);
        let mut n = n(vec![call("native", None)]);
        correlate(&mut g, &mut n);
        assert!(g[0].native_links.is_empty());
        assert_eq!(n[0].native.as_ref().unwrap().association, "unlinked");
    }
    #[test]
    fn reused_id_multiple_sessions_or_attempts_are_ambiguous() {
        for (mut g, mut n) in [
            (g(&["r", "r"]), n(vec![call("n", Some("r"))])),
            (
                g(&["r"]),
                n(vec![call("n1", Some("r")), call("n2", Some("r"))]),
            ),
        ] {
            correlate(&mut g, &mut n);
            assert!(g.iter().all(|r| r.native_links.is_empty()));
            assert!(
                n.iter()
                    .all(|r| r.native.as_ref().unwrap().association == "ambiguous")
            );
        }
    }
    #[test]
    fn client_and_session_conflict_never_force_attribution() {
        let mut g = g(&["r"]);
        g[0].agent_product_id = Some("codex-desktop".into());
        let mut n = n(vec![call("native", Some("r"))]);
        correlate(&mut g, &mut n);
        assert_eq!(
            n[0].native.as_ref().unwrap().association,
            "identity_conflict"
        );
        assert!(g[0].native_links.is_empty());
    }
    #[test]
    fn conflicting_usage_retains_both_evidence_and_gateway_value() {
        let mut g = g(&["r"]);
        let mut call = call("native", Some("r"));
        call.usage.as_mut().unwrap().input_tokens = Some(999);
        let mut n = n(vec![call]);
        correlate(&mut g, &mut n);
        assert!(g[0].native_links[0].usage_conflict);
        assert_eq!(g[0].usage.input_tokens, Some(100));
        assert_eq!(n[0].usage.input_tokens, Some(999));
    }
    #[test]
    fn unknown_time_is_only_in_all_and_never_drawn_as_zero() {
        let mut call = call("native", None);
        call.timestamp = None;
        let rows = n(vec![call]);
        let now = time("2026-10-19T12:00:00Z").unwrap();
        let s = Snapshot::new(Query::default(), rows.clone(), now, "all".into(), None).unwrap();
        assert_eq!(s.rows.len(), 1);
        assert_eq!(s.dashboard.missing_time, 1);
        assert_eq!(s.dashboard.heatmap.missing_time, 1);
        assert!(
            s.dashboard
                .heatmap
                .days
                .iter()
                .all(|d| d.records == 0 && d.tokens.is_none())
        );
        let s = Snapshot::new(
            Query {
                start: Some("2026-10-19T00:00:00+08:00".into()),
                ..Default::default()
            },
            rows,
            now,
            "day".into(),
            None,
        )
        .unwrap();
        assert!(s.rows.is_empty());
        assert_eq!(s.dashboard.missing_time, 1);
    }
    #[test]
    fn heatmap_shanghai_day_boundaries_and_unknown_gaps() {
        let mut rows = g(&["a", "b"]);
        rows[0].started_at = "2026-10-18T16:00:00Z".into();
        rows[1].started_at = "2026-10-18T15:59:59Z".into();
        let now = time("2026-10-19T12:00:00Z").unwrap();
        let h = Heatmap::new(&rows, now, &Query::default());
        assert_eq!(h.days.len(), 371);
        assert_eq!(h.days.iter().map(|d| d.records).sum::<usize>(), 2);
        assert_eq!(
            h.days
                .iter()
                .find(|d| d.date == "2026-10-19")
                .unwrap()
                .records,
            1
        );
        assert_eq!(
            h.days
                .iter()
                .find(|d| d.date == "2026-10-18")
                .unwrap()
                .records,
            1
        );
        assert!(
            h.days
                .iter()
                .any(|d| d.state == "unrecorded" && d.tokens.is_none())
        );
        assert!(h.days.iter().any(|d| d.state == "future"));
    }
    #[test]
    fn native_csv_retains_identity_type_and_unknown_time() {
        let mut call = call("native", Some("response"));
        call.timestamp = None;
        let s = Snapshot::new(
            Query {
                source: Some("native".into()),
                ..Default::default()
            },
            n(vec![call]),
            time("2026-10-19T12:00:00Z").unwrap(),
            "n".into(),
            None,
        )
        .unwrap();
        let csv = s.csv();
        assert!(csv.contains("模型响应记录"));
        assert!(csv.contains("\"response\""));
        assert!(csv.contains("时间未知"));
        assert!(csv.contains("非请求开始时间"));
    }
}
