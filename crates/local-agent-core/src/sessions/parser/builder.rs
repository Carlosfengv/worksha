use super::text::{content_text, is_context, normalize_time, safe_text, str_at};
use super::{
    Builder, CallSummary, Event, MAX_TEXT, NativeResult, PAGE, Parsed, ProjectCache, SessionSummary,
};
use chrono::DateTime;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

impl Builder {
    pub(super) fn weight(&self) -> usize {
        self.events.len()
            + self.seen_rows.len()
            + self.lookup.len()
            + self.proposed_writes.values().map(Vec::len).sum::<usize>()
    }
    pub(super) fn new(agent: &str, fallback: &str, window: Option<usize>) -> Self {
        Self {
            activity_history: Default::default(),
            context: crate::sessions::context::Collector::new(window.is_some()),
            agent: agent.into(),
            product: (agent == "opencode").then(|| "opencode".into()),
            product_evidence: (agent == "opencode").then_some("native_record"),
            known_products: HashSet::new(),
            native_id: None,
            fallback: fallback.into(),
            version: None,
            title: None,
            title_source: "fallback".into(),
            project: None,
            remote: None,
            model: None,
            provider: None,
            created: None,
            updated: None,
            turn: "unknown".into(),
            events: vec![],
            lookup: HashMap::new(),
            seen_rows: HashSet::new(),
            window,
            total_usage: None,
            usage_priority: 0,
            foreign_records: 0,
            proposed_writes: HashMap::new(),
            confirmed_writes: HashSet::new(),
        }
    }
    pub(super) fn content(&self, index: usize) -> bool {
        self.window
            .is_some_and(|start| (start..start + PAGE).contains(&index))
    }
    pub(super) fn event(
        &mut self,
        key: &str,
        kind: &str,
        name: &str,
        ts: Option<&str>,
        line: usize,
    ) -> usize {
        if let Some(index) = self.lookup.get(key) {
            return *index;
        }
        let index = self.events.len();
        self.events.push(Event {
            agent_attribution: self.product_evidence,
            agent_product_id: self.product.clone(),
            id: key.into(),
            native_id: key.into(),
            turn_id: self.turn.clone(),
            kind: kind.into(),
            role: None,
            name: safe_text(name, 300).0,
            timestamp: ts.and_then(normalize_time),
            end_time: None,
            status: "recorded".into(),
            text: None,
            input: None,
            output: None,
            model: self.model.clone(),
            model_response_id: None,
            provider: self.provider.clone(),
            usage: None,
            raw_usage: None,
            call_id: None,
            server: None,
            tool_source: None,
            duration_ms: None,
            truncated: false,
            source_line: line,
            failure: None,
        });
        self.lookup.insert(key.into(), index);
        index
    }
    pub(super) fn failure_event(
        &mut self,
        key: &str,
        mut failure: crate::sessions::ModelFailure,
        ts: Option<&str>,
        line: usize,
    ) -> usize {
        let index = self.event(key, "diagnostic", "模型 / Agent 调用异常", ts, line);
        self.events[index].status = if failure.category == "cancelled" {
            "cancelled"
        } else {
            "error"
        }
        .into();
        if !self.content(index) {
            failure.message = None;
        }
        self.events[index].failure = Some(failure);
        index
    }
    pub(super) fn title_from(&mut self, text: &str) {
        let text = text
            .split_once("## My request:")
            .map_or(text, |(_, request)| request)
            .trim();
        if self.title.is_none() && !is_context(text) && !text.trim().is_empty() {
            self.title = Some(
                safe_text(
                    text.split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .as_str(),
                    100,
                )
                .0,
            );
            self.title_source = "first_user_message".into();
        }
    }
    pub(super) fn write_text(&mut self, index: usize, field: &str, text: &str) {
        if !self.content(index) || text.is_empty() {
            return;
        }
        let event = &mut self.events[index];
        let existing = match field {
            "input" => &mut event.input,
            "output" => &mut event.output,
            _ => &mut event.text,
        };
        let mut combined = existing.take().unwrap_or_default();
        if !combined.is_empty() {
            combined.push('\n');
        }
        let remaining = MAX_TEXT.saturating_sub(combined.chars().count());
        let (addition, truncated) = safe_text(text, remaining);
        combined.push_str(&addition);
        event.truncated |= truncated;
        *existing = Some(combined);
    }
    pub(super) fn tool(
        &mut self,
        id: &str,
        name: &str,
        input: &Value,
        ts: Option<&str>,
        line: usize,
    ) -> usize {
        let (kind, server) = if name == "Skill" {
            ("skill", None)
        } else if let Some(tail) = name
            .strip_prefix("mcp__")
            .or_else(|| name.strip_prefix("functions.mcp__"))
        {
            ("mcp", tail.split_once("__").map(|(s, _)| s.to_string()))
        } else {
            ("tool", None)
        };
        let index = self.event(&format!("call:{id}"), kind, name, ts, line);
        if let Some(cwd) = self.project.as_deref() {
            let files = crate::projects::writes::proposed(name, input, cwd);
            if !files.is_empty() {
                self.proposed_writes.entry(id.into()).or_insert(files);
            }
        }
        let call = &mut self.events[index];
        call.call_id = Some(id.into());
        call.server = server;
        if call.output.is_none() && call.end_time.is_none() {
            call.status = "missing_result".into();
        }
        if kind == "skill"
            && let Some(skill) = str_at(input, "skill")
        {
            call.name = format!("Skill · {}", safe_text(skill, 200).0);
        }
        if self.content(index) && !input.is_null() {
            let text = if let Some(s) = input.as_str() {
                s.into()
            } else {
                serde_json::to_string_pretty(input).unwrap_or_default()
            };
            // Repeated native begin/response records describe one invocation.
            if self.events[index].input.is_none() {
                self.write_text(index, "input", &text);
            }
        }
        index
    }
    pub(super) fn result(
        &mut self,
        id: &str,
        output: &Value,
        error: bool,
        ts: Option<&str>,
        line: usize,
    ) {
        let key = format!("call:{id}");
        let index = self.lookup.get(&key).copied().unwrap_or_else(|| {
            self.event(
                &format!("orphan:{id}"),
                "event",
                "未关联的工具结果",
                ts,
                line,
            )
        });
        self.events[index].status = if error { "error" } else { "returned" }.into();
        // Execution/result provenance can differ from the client that proposed a tool.
        self.events[index].agent_product_id = self.product.clone();
        self.events[index].agent_attribution = self.product_evidence;
        self.events[index].end_time = ts.and_then(normalize_time);
        if crate::projects::writes::explicit_success(&self.events[index].name, output, error) {
            self.confirmed_writes.insert(id.into());
        }
        if self.events[index].output.is_none() {
            self.write_text(index, "output", &content_text(output));
        }
    }
    pub(super) fn consume(&mut self, v: Value, line: usize) {
        let incoming = if self.agent == "claude" {
            str_at(&v, "sessionId")
        } else if str_at(&v, "type") == Some("session_meta") {
            str_at(&v["payload"], "id").or_else(|| str_at(&v["payload"], "session_id"))
        } else {
            None
        };
        if incoming
            .zip(self.native_id.as_deref())
            .is_some_and(|(incoming, current)| incoming != current)
        {
            self.foreign_records += 1;
            return;
        }
        let product = crate::agents::native_client(&self.agent, &v).flatten();
        if let Some(product) = &product {
            self.known_products.insert(product.clone());
        }
        // Session creation metadata identifies a participant, not later execution.
        self.product = if v["type"] == "session_meta" {
            None
        } else {
            product
        };
        self.product_evidence = if self.agent == "claude" {
            crate::agents::claude_client_evidence(&v)
        } else {
            self.product.as_ref().map(|_| "native_record")
        };
        let ts = str_at(&v, "timestamp");
        if let Some(time) = ts.and_then(normalize_time) {
            if self.created.as_ref().is_none_or(|t| &time < t) {
                self.created = Some(time.clone());
            }
            if self.updated.as_ref().is_none_or(|t| &time > t) {
                self.updated = Some(time);
            }
        }
        self.context
            .observe(&self.agent, &v, ts, line, self.product.as_deref());
        self.activity_history.observe(&self.agent, &v);
        if self.agent == "claude" {
            self.claude(&v, ts, line);
        } else {
            self.codex(&v, ts, line);
        }
    }
    pub(super) fn finish(
        mut self,
        path: &Path,
        bytes: u64,
        allocated: Option<u64>,
        warnings: Vec<String>,
        paged: bool,
        projects: Option<&mut ProjectCache>,
    ) -> NativeResult<Parsed> {
        let native = self.native_id.take().unwrap();
        let id = format!("{}:{}", self.agent, native);
        let title = self.title.unwrap_or_else(|| {
            format!(
                "未命名会话 · {}",
                self.fallback.chars().take(12).collect::<String>()
            )
        });
        let mut usage = self.total_usage.clone().unwrap_or_default();
        if self.total_usage.is_none() {
            for e in self.events.iter().filter(|e| e.kind == "model") {
                if let Some(u) = &e.usage {
                    usage.add(u);
                }
            }
        }
        let model_count = self.events.iter().filter(|e| e.kind == "model").count();
        let known_usage = self
            .events
            .iter()
            .filter(|e| {
                e.kind == "model" && e.usage.as_ref().is_some_and(|u| u.total_tokens.is_some())
            })
            .count();
        let project_identity = self.project.as_deref().and_then(|path| match projects {
            Some(cache) => cache.identity(path),
            None => crate::projects::identity(path),
        });
        let mut products: Vec<String> = self
            .events
            .iter()
            .filter_map(|event| event.agent_product_id.clone())
            .collect();
        if let Some(product) = &self.product {
            products.push(product.clone());
        }
        products.extend(self.known_products);
        products.sort();
        products.dedup();
        let has_unknown_client = self
            .events
            .iter()
            .any(|event| event.agent_product_id.is_none())
            || products.is_empty();
        let mut summary = SessionSummary {
            project_hidden: false,
            agent_product_id: (!has_unknown_client && products.len() == 1)
                .then(|| products[0].clone()),
            agent_products: products,
            has_unknown_client,
            id: id.clone(),
            native_id: native,
            agent_id: self.agent.clone(),
            agent_version: self.version,
            title: title.clone(),
            title_source: self.title_source,
            project_path: self.project.clone(),
            project_name: self
                .project
                .as_ref()
                .and_then(|p| Path::new(p).file_name())
                .map(|s| s.to_string_lossy().into()),
            project_id: project_identity.as_ref().map(|p| p.id.clone()),
            project_root: project_identity.map(|p| p.path),
            git_remote: self.remote,
            created_at: self.created,
            updated_at: self.updated,
            status: "imported",
            message_count: self
                .events
                .iter()
                .filter(|e| e.kind == "message" || (e.kind == "model" && e.role.is_some()))
                .count(),
            turn_count: {
                let turns: HashSet<_> = self
                    .events
                    .iter()
                    .map(|e| e.turn_id.as_str())
                    .filter(|id| *id != "unknown")
                    .collect();
                (!turns.is_empty()).then_some(turns.len())
            },
            model_calls: model_count,
            model_error_count: self.events.iter().filter(|e| e.failure.is_some()).count(),
            latest_failure_id: None,
            tool_calls: self.events.iter().filter(|e| e.kind == "tool").count(),
            mcp_calls: self.events.iter().filter(|e| e.kind == "mcp").count(),
            skill_calls: self.events.iter().filter(|e| e.kind == "skill").count(),
            usage,
            usage_quality: if self.total_usage.is_some() {
                "native_cumulative"
            } else if known_usage > 0 && known_usage == model_count {
                "native_requests"
            } else if known_usage > 0 {
                "partial"
            } else {
                "unknown"
            },
            disk_bytes: bytes,
            disk_allocated_bytes: allocated,
            disk_scope: "native_transcript_file",
            source_path: path.to_string_lossy().into(),
            partial: !warnings.is_empty(),
        };
        if summary.updated_at.is_none() {
            summary.partial = true;
        }
        let mut calls = vec![];
        for event in &mut self.events {
            event.id = format!("{id}:{}", event.id);
            event.model_response_id = event
                .model_response_id
                .take()
                .map(|response| format!("{id}:{response}"));
            event.turn_id = format!("{id}:turn:{}", event.turn_id);
            if ["model", "tool", "mcp", "skill", "diagnostic"].contains(&event.kind.as_str()) {
                calls.push(CallSummary {
                    // OpenCode message IDs are local transcript identities, not upstream response IDs.
                    response_id: (!matches!(self.agent.as_str(), "opencode" | "antigravity"))
                        .then(|| event.native_id.strip_prefix("model:").map(str::to_owned))
                        .flatten(),
                    native_agent_product_id: event.agent_product_id.clone(),
                    agent_attribution: event.agent_attribution,
                    agent_product_id: event.agent_product_id.clone(),
                    id: event.id.clone(),
                    session_id: id.clone(),
                    session_title: title.clone(),
                    agent_id: self.agent.clone(),
                    turn_id: event.turn_id.clone(),
                    kind: event.kind.clone(),
                    name: event.name.clone(),
                    status: event.status.clone(),
                    timestamp: event
                        .failure
                        .as_ref()
                        .and_then(|f| f.last_observed_at.clone())
                        .or_else(|| event.timestamp.clone()),
                    model: event.model.clone(),
                    provider: event.provider.clone(),
                    model_response_id: event.model_response_id.clone(),
                    usage: event.usage.clone(),
                    server: event.server.clone(),
                    tool_name: event
                        .tool_source
                        .as_ref()
                        .and_then(|s| s.tool_name.clone())
                        .or_else(|| {
                            event
                                .name
                                .strip_prefix("mcp__")
                                .or_else(|| event.name.strip_prefix("functions.mcp__"))
                                .and_then(|s| {
                                    event
                                        .server
                                        .as_ref()
                                        .and_then(|server| s.strip_prefix(&format!("{server}__")))
                                        .or_else(|| s.split_once("__").map(|(_, tool)| tool))
                                })
                                .map(str::to_owned)
                        }),
                    duration_ms: event.duration_ms,
                    tool_source: event.tool_source.clone(),
                    failure: event.failure.clone(),
                });
            }
        }
        summary.latest_failure_id = self
            .events
            .iter()
            .filter(|e| e.failure.is_some())
            .max_by_key(|e| {
                e.failure
                    .as_ref()
                    .and_then(|f| f.last_observed_at.as_ref())
                    .or(e.timestamp.as_ref())
                    .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
                    .map(|t| t.timestamp_millis())
            })
            .map(|e| e.id.clone());
        let total = self.events.len();
        let writes = self
            .events
            .iter()
            .filter_map(|event| {
                let call = event.call_id.as_ref()?;
                if !self.confirmed_writes.contains(call) || event.status == "error" {
                    return None;
                }
                let files = self.proposed_writes.get(call)?.clone();
                Some(crate::projects::writes::FileWrite {
                    id: event.id.clone(),
                    project_id: summary.project_id.clone()?,
                    session_id: id.clone(),
                    agent_id: self.agent.clone(),
                    agent_product_id: event.agent_product_id.clone(),
                    turn_id: event.turn_id.clone(),
                    call_id: call.clone(),
                    tool: event.name.clone(),
                    occurred_at: event.end_time.clone().or_else(|| event.timestamp.clone()),
                    source_line: event.source_line,
                    evidence:
                        "明确写入工具及同一 call_id 的成功结果；不代表当前文件仍由该 Agent 保持"
                            .into(),
                    files,
                })
            })
            .collect();
        let unknown_non_calls = self.events.iter().any(|e| {
            e.agent_product_id.is_none()
                && !["model", "tool", "mcp", "skill", "diagnostic"].contains(&e.kind.as_str())
        });
        let start = self.window.unwrap_or(0);
        let end = (start + PAGE).min(total);
        let events = if paged {
            self.events.into_iter().skip(start).take(PAGE).collect()
        } else {
            self.events
        };
        Ok(Parsed {
            activity_messages: self.activity_history.finish(&summary),
            context: self.context.finish(),
            unknown_non_calls,
            summary,
            calls,
            events,
            total,
            end,
            warnings,
            snapshot_bytes: 0,
            fingerprint: String::new(),
            writes,
        })
    }
}
