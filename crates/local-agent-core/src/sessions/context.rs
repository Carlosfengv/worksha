//! Evidence from the same read-only native snapshot as the conversation.
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionContext {
    pub input_tokens: Option<u64>,
    pub window_tokens: Option<u64>,
    pub used_percentage: Option<f64>,
    pub sample_at: Option<String>,
    pub model: Option<String>,
    pub source: Option<String>,
    pub sample_agent_product_id: Option<String>,
    pub compactions: Vec<ContextRecord>,
    pub loaded: Vec<ContextRecord>,
    pub compaction_count: usize,
    pub loaded_count: usize,
    pub limited: bool,
}
/// Bounded metadata for live cards. Never includes conversation or loaded text.
#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsage {
    pub input_tokens: Option<u64>,
    pub window_tokens: Option<u64>,
    pub used_percentage: Option<f64>,
    pub sample_at: Option<String>,
    pub model: Option<String>,
    pub source: Option<String>,
    pub sample_agent_product_id: Option<String>,
}
impl SessionContext {
    pub fn usage_summary(&self) -> ContextUsage {
        ContextUsage {
            input_tokens: self.input_tokens,
            window_tokens: self.window_tokens,
            used_percentage: self
                .input_tokens
                .zip(self.window_tokens.filter(|n| *n > 0))
                .map(|(input, window)| input as f64 / window as f64 * 100.0),
            sample_at: self.sample_at.clone(),
            model: self.model.clone(),
            source: self.source.clone(),
            sample_agent_product_id: self.sample_agent_product_id.clone(),
        }
    }
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextRecord {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub timestamp: Option<String>,
    pub source_line: usize,
    pub agent_product_id: Option<String>,
    pub text: Option<String>,
    pub truncated: bool,
}
#[derive(Default, Clone)]
pub(super) struct Collector {
    pub data: SessionContext,
    pub capture: bool,
    reads: HashMap<String, String>,
}
impl Collector {
    pub fn new(capture: bool) -> Self {
        Self {
            capture,
            ..Default::default()
        }
    }
    fn record(
        &mut self,
        kind: &str,
        label: &str,
        text: Option<&str>,
        ts: Option<&str>,
        line: usize,
        product: Option<&str>,
    ) {
        let compression = kind == "compaction";
        if compression {
            self.data.compaction_count += 1;
        } else {
            self.data.loaded_count += 1;
        }
        if !self.capture {
            return;
        }
        let target = if compression {
            &mut self.data.compactions
        } else {
            &mut self.data.loaded
        };
        let (preview, truncated) = text
            .map(|s| super::parser::safe_text(s, 4096))
            .unwrap_or_default();
        target.push(ContextRecord {
            id: format!(
                "{kind}:{line}:{}",
                self.data.loaded_count + self.data.compaction_count
            ),
            kind: kind.into(),
            label: super::sanitize_label(label, 500),
            timestamp: ts.map(str::to_owned),
            source_line: line,
            agent_product_id: product.map(str::to_owned),
            text: (!preview.is_empty()).then_some(preview),
            truncated,
        });
        if target.len() > 50 {
            target.remove(0);
            self.data.limited = true;
        }
    }
    fn compact(
        &mut self,
        label: &str,
        text: Option<&str>,
        ts: Option<&str>,
        line: usize,
        product: Option<&str>,
    ) {
        self.data.input_tokens = None;
        self.data.window_tokens = None;
        self.data.sample_at = None;
        self.data.source = None;
        self.data.sample_agent_product_id = None;
        self.record("compaction", label, text, ts, line, product);
    }
    fn usage(
        &mut self,
        input: Option<u64>,
        window: Option<u64>,
        ts: Option<&str>,
        model: Option<&str>,
        source: &str,
    ) {
        // Never substitute cumulative usage, an older sample or a guessed model limit.
        self.data.input_tokens = input;
        self.data.window_tokens = window.filter(|v| *v > 0);
        self.data.sample_at = ts.map(str::to_owned);
        self.data.model = model
            .map(|s| super::sanitize_label(s, 200))
            .or(self.data.model.take());
        self.data.source = Some(source.into());
    }
    pub fn observe(
        &mut self,
        agent: &str,
        v: &Value,
        ts: Option<&str>,
        line: usize,
        product: Option<&str>,
    ) {
        if v["isSidechain"] == true {
            return;
        }
        if agent == "codex" {
            let p = &v["payload"];
            match v["type"].as_str().unwrap_or("") {
                "session_meta" => {
                    if let Some(text) = p["base_instructions"]["text"].as_str() {
                        self.record(
                            "injected",
                            "基础指令 · 原生注入记录",
                            Some(text),
                            ts,
                            line,
                            product,
                        );
                    }
                }
                "turn_context" => {
                    self.data.model = p["model"].as_str().map(str::to_owned);
                    for key in ["developer_instructions", "user_instructions"] {
                        if let Some(text) = p[key].as_str() {
                            self.record("injected", key, Some(text), ts, line, product);
                        }
                    }
                }
                "compacted" => {
                    self.compact("原生压缩记录", p["message"].as_str(), ts, line, product)
                }
                "event_msg" if p["type"] == "token_count" => {
                    self.data.sample_agent_product_id = product.map(str::to_owned);
                    self.usage(
                        p["info"]["last_token_usage"]["input_tokens"].as_u64(),
                        p["info"]["model_context_window"].as_u64(),
                        ts,
                        None,
                        "Codex last_token_usage",
                    );
                }
                "event_msg" if p["type"] == "context_compacted" => {
                    self.compact("上下文已压缩 · 原生事件", None, ts, line, product)
                }
                "response_item" if p["type"] == "message" => {
                    let role = p["role"].as_str().unwrap_or("");
                    let prefix = p["content"]
                        .as_array()
                        .and_then(|parts| parts.first())
                        .and_then(|part| part["text"].as_str())
                        .unwrap_or("");
                    if ["system", "developer"].contains(&role)
                        || (role == "user" && prefix.starts_with("# AGENTS.md instructions"))
                    {
                        let text = if self.capture {
                            super::parser::content_text(&p["content"])
                        } else {
                            String::new()
                        };
                        self.record(
                            "injected",
                            "会话指令 · 原生消息",
                            Some(&text),
                            ts,
                            line,
                            product,
                        );
                    }
                }
                _ => {}
            }
            return;
        }
        if v["type"] == "system" && v["subtype"] == "compact_boundary" {
            self.compact("上下文压缩边界", v["content"].as_str(), ts, line, product);
        }
        if v["isCompactSummary"] == true {
            let text = super::parser::content_text(&v["message"]["content"]);
            self.compact("压缩摘要 · 原生标记", Some(&text), ts, line, product);
        }
        let message = &v["message"];
        if v["type"] == "assistant"
            && !v["isApiErrorMessage"].as_bool().unwrap_or(false)
            && message["model"] != "<synthetic>"
        {
            self.data.sample_agent_product_id = product.map(str::to_owned);
            self.usage(
                super::parser::Usage::parse(&message["usage"], true).input_tokens,
                None,
                ts,
                message["model"].as_str(),
                "Claude 原生响应 usage",
            );
        }
        for block in message["content"].as_array().into_iter().flatten() {
            if block["type"] == "tool_use" {
                let name = block["name"].as_str().unwrap_or("");
                let path = block["input"]["file_path"].as_str().unwrap_or("");
                if self.reads.len() < 256
                    && (name == "Skill" || (name == "Read" && memory_path(path)))
                    && let Some(id) = block["id"].as_str()
                {
                    self.reads.insert(
                        id.into(),
                        if name == "Skill" {
                            "Skill 加载结果".into()
                        } else {
                            format!("读取记忆 / 规则：{path}")
                        },
                    );
                }
            }
            if block["type"] == "tool_result"
                && let Some(label) = block["tool_use_id"]
                    .as_str()
                    .and_then(|id| self.reads.remove(id))
                && block["is_error"] != true
            {
                let text = super::parser::content_text(&block["content"]);
                self.record("read", &label, Some(&text), ts, line, product);
            }
        }
    }
    pub fn opencode_message(&mut self, v: &Value, ts: Option<&str>, line: usize) {
        if v["summary"] == true {
            self.compact("压缩摘要消息 · 原生记录", None, ts, line, None);
        } else if v["role"] == "assistant" {
            let t = &v["tokens"];
            let input = t["input"].as_u64().map(|n| {
                n.saturating_add(t["cache"]["read"].as_u64().unwrap_or(0))
                    .saturating_add(t["cache"]["write"].as_u64().unwrap_or(0))
            });
            // OpenCode initializes an in-flight assistant with zero counters.
            // Keep the previous observed sample until usage is reported; a
            // completed, non-error response can still establish a real zero.
            if input.is_none()
                || input == Some(0)
                    && (v["time"]["completed"].as_u64().is_none() || !v["error"].is_null())
            {
                return;
            }
            self.data.sample_agent_product_id = None;
            self.usage(
                input,
                None,
                ts,
                v["modelID"].as_str(),
                "OpenCode 原生响应 tokens",
            );
        }
    }
    pub fn opencode_part(&mut self, v: &Value, ts: Option<&str>, line: usize) {
        if v["type"] == "compaction" {
            self.compact("压缩触发记录 · 完成状态未知", None, ts, line, None);
        }
        if v["type"] == "text" && v["synthetic"] == true {
            self.record(
                "injected",
                "补充上下文 · 原生片段",
                v["text"].as_str(),
                ts,
                line,
                None,
            );
        }
        let path = v["state"]["input"]["filePath"].as_str().unwrap_or("");
        if v["type"] == "tool"
            && v["state"]["status"] == "completed"
            && (v["tool"] == "skill" || (v["tool"] == "read" && memory_path(path)))
        {
            self.record(
                "read",
                if v["tool"] == "skill" {
                    "Skill 加载结果"
                } else {
                    path
                },
                v["state"]["output"].as_str(),
                ts,
                line,
                None,
            );
        }
    }
    pub fn summary_text(&mut self, text: &str) {
        if let Some(record) = self.data.compactions.last_mut() {
            let combined = format!(
                "{}{}",
                record.text.as_deref().unwrap_or(""),
                text.chars().take(4097).collect::<String>()
            );
            let (preview, truncated) = super::parser::safe_text(&combined, 4096);
            record.text = Some(preview);
            record.truncated |= truncated || text.chars().count() > 4096;
        }
    }
    pub fn finish(mut self) -> SessionContext {
        self.data.used_percentage = self
            .data
            .input_tokens
            .zip(self.data.window_tokens)
            .map(|(input, window)| input as f64 / window as f64 * 100.0);
        self.data
    }
}
fn memory_path(path: &str) -> bool {
    ["CLAUDE.md", "AGENTS.md", "AGENTS.override.md", "MEMORY.md"].contains(
        &std::path::Path::new(path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(""),
    ) || path.contains("/.claude/rules/")
        || path.contains("/memory/")
        || path.contains("/.codex/memories/")
}
