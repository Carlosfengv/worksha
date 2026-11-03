use local_agent_core::sessions::Usage;
use local_agent_core::sessions::activity_input::tool_name;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Default)]
pub struct Update {
    pub tool_names: Vec<String>,
    pub response_id: Option<String>,
    pub reported_cost: Option<f64>,
    pub generated: bool,
    pub text: bool,
    pub estimated_tokens: f64,
    pub estimated_output_tokens: f64,
    pub tool: bool,
    pub terminal: Option<&'static str>,
    pub model: Option<String>,
    pub usage: Option<Usage>,
    pub issue: bool,
    pub stop_reason: Option<String>,
}
pub struct StreamDecoder {
    tools: BTreeMap<u64, String>,
    pending: Vec<u8>,
    data: Vec<u8>,
    discard: bool,
    protocol: String,
    stop_reason: Option<String>,
    terminal: bool,
    damaged: bool,
    event: Vec<u8>,
}
impl StreamDecoder {
    pub fn new(protocol: &str) -> Self {
        Self {
            tools: BTreeMap::new(),
            pending: vec![],
            data: vec![],
            discard: false,
            protocol: protocol.into(),
            stop_reason: None,
            terminal: false,
            damaged: false,
            event: vec![],
        }
    }
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Update> {
        let mut updates = vec![];
        for byte in bytes {
            if *byte == b'\n' {
                let line = std::mem::take(&mut self.pending);
                let line = line.strip_suffix(b"\r").unwrap_or(&line);
                if line.is_empty() {
                    if !self.discard
                        && let Some(update) = self.dispatch()
                    {
                        updates.push(update);
                    }
                    self.data.clear();
                    self.event.clear();
                    self.discard = false;
                } else if let Some(event) = line.strip_prefix(b"event:") {
                    self.event = event.strip_prefix(b" ").unwrap_or(event).to_vec();
                } else if !self.discard
                    && let Some(value) = line.strip_prefix(b"data:")
                {
                    if !self.data.is_empty() {
                        self.data.push(b'\n');
                    }
                    self.data
                        .extend_from_slice(value.strip_prefix(b" ").unwrap_or(value));
                    if self.data.len() > 8 * 1024 * 1024 {
                        self.data.clear();
                        self.discard = true;
                        self.damaged = true;
                        updates.push(Update {
                            issue: true,
                            ..Update::default()
                        });
                    }
                }
            } else if self.pending.len() < 8 * 1024 * 1024 {
                self.pending.push(*byte);
            } else {
                self.pending.clear();
                self.discard = true;
                self.damaged = true;
                updates.push(Update {
                    issue: true,
                    ..Update::default()
                });
            }
        }
        updates
    }
    fn dispatch(&mut self) -> Option<Update> {
        if self.terminal || (self.data.is_empty() && self.event != b"error") {
            return None;
        }
        let mut update = if !self.data.is_empty() {
            decode(&self.data, &self.protocol)
        } else {
            Update::default()
        };
        if update.generated && !update.tool {
            self.tools.clear();
        }
        if update.tool {
            if self.protocol == "chat-completions" {
                if let Ok(v) = serde_json::from_slice::<Value>(&self.data)
                    && let Some(tools) = v["choices"][0]["delta"]["tool_calls"].as_array()
                {
                    for tool in tools.iter().take(8) {
                        let Some(index) = tool["index"].as_u64().filter(|i| *i <= 127) else {
                            continue;
                        };
                        let Some(name) = tool["function"]["name"].as_str().and_then(tool_name)
                        else {
                            continue;
                        };
                        if self.tools.len() < 8 || self.tools.contains_key(&index) {
                            let entry = self.tools.entry(index).or_default();
                            entry.extend(
                                name.chars()
                                    .take(100_usize.saturating_sub(entry.chars().count())),
                            );
                        }
                    }
                }
            } else {
                for name in &update.tool_names {
                    if self.tools.len() < 8 && !self.tools.values().any(|old| old == name) {
                        self.tools.insert(self.tools.len() as u64, name.clone());
                    }
                }
            }
            update.tool_names = self.tools.values().cloned().collect();
        }
        if self.event == b"error" {
            update.terminal = Some("error");
        }
        self.damaged |= update.issue;
        if let Some(reason) = &update.stop_reason {
            self.stop_reason = Some(reason.clone());
        }
        if update.terminal == Some("success") {
            update.terminal = Some(if self.damaged {
                "interrupted"
            } else {
                end_status(self.stop_reason.as_deref())
            });
        }
        self.terminal |= update.terminal.is_some();
        Some(update)
    }
    /// EOF may omit the final newline. Only a valid protocol terminal proves completion.
    pub fn end(&mut self) -> Vec<Update> {
        let mut updates = self.feed(b"\n\n");
        if self.discard {
            updates.push(Update {
                issue: true,
                ..Default::default()
            });
        }
        updates
    }
}
fn end_status(reason: Option<&str>) -> &'static str {
    match reason {
        Some("max_tokens" | "model_context_window_exceeded" | "length" | "content_filter") => {
            "incomplete"
        }
        _ => "success",
    }
}
fn number(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(Value::as_u64)
}
fn usage(v: &Value, claude: bool) -> Usage {
    let cached = number(
        v,
        if claude {
            "cache_read_input_tokens"
        } else {
            "cached_input_tokens"
        },
    )
    .or_else(|| number(&v["input_tokens_details"], "cached_tokens"));
    let write = number(v, "cache_creation_input_tokens")
        .or_else(|| number(&v["input_tokens_details"], "cache_write_tokens"));
    let input = number(v, "input_tokens").map(|n| {
        if claude {
            n.saturating_add(cached.unwrap_or(0))
                .saturating_add(write.unwrap_or(0))
        } else {
            n
        }
    });
    let output = number(v, "output_tokens");
    Usage {
        input_tokens: input,
        output_tokens: output,
        cached_input_tokens: cached,
        cache_write_input_tokens: write,
        reasoning_output_tokens: number(&v["output_tokens_details"], "reasoning_tokens"),
        total_tokens: input.zip(output).map(|(a, b)| a.saturating_add(b)),
    }
}
pub fn decode(bytes: &[u8], protocol: &str) -> Update {
    crate::gateway::capabilities::builtin()
        .resolve(protocol)
        .map(|wire| wire.metrics(bytes))
        .unwrap_or_else(|_| Update {
            issue: true,
            ..Update::default()
        })
}
pub(crate) fn decode_builtin(bytes: &[u8], protocol: &str) -> Update {
    if bytes == b"[DONE]" {
        return Update {
            terminal: (protocol == "chat-completions").then_some("success"),
            issue: protocol != "chat-completions",
            ..Update::default()
        };
    }
    let Ok(v) = serde_json::from_slice::<Value>(bytes) else {
        return Update {
            issue: true,
            ..Update::default()
        };
    };
    let kind = v["type"].as_str().unwrap_or("");
    let mut u = Update {
        response_id: v["response"]["id"]
            .as_str()
            .or_else(|| v["message"]["id"].as_str())
            .or_else(|| {
                ((protocol == "chat-completions")
                    || v["object"] == "response"
                    || v["type"] == "message")
                    .then(|| v["id"].as_str())
                    .flatten()
            })
            .filter(|id| {
                !id.is_empty()
                    && id.len() <= 512
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
            })
            .map(str::to_owned),
        reported_cost: v["usage"]["cost"]
            .as_f64()
            .or_else(|| v["response"]["usage"]["cost"].as_f64())
            .filter(|n| n.is_finite() && *n >= 0.0),
        ..Default::default()
    };
    if kind == "error" || v.get("error").is_some_and(|error| !error.is_null()) {
        u.terminal = Some("error");
        if v["usage"].is_object() {
            let raw = &v["usage"];
            u.usage = Some(if protocol == "chat-completions" {
                usage(
                    &serde_json::json!({"input_tokens":raw["prompt_tokens"],"output_tokens":raw["completion_tokens"],"input_tokens_details":raw["prompt_tokens_details"],"output_tokens_details":raw["completion_tokens_details"]}),
                    false,
                )
            } else {
                usage(raw, protocol == "messages")
            });
        }
        return u;
    }
    let mut visible = "";
    let mut generated = "";
    if protocol == "messages" {
        match kind {
            "content_block_delta" => {
                let d = &v["delta"];
                match d["type"].as_str().unwrap_or("") {
                    "text_delta" => {
                        visible = d["text"].as_str().unwrap_or("");
                        generated = visible;
                    }
                    "input_json_delta" => {
                        generated = d["partial_json"].as_str().unwrap_or("");
                        u.tool = true;
                    }
                    "thinking_delta" => generated = d["thinking"].as_str().unwrap_or(""),
                    _ => {}
                }
            }
            "content_block_start" => {
                let b = &v["content_block"];
                if b["type"] == "text" {
                    visible = b["text"].as_str().unwrap_or("");
                    generated = visible;
                } else if b["type"] == "tool_use" {
                    u.tool = true;
                    u.tool_names.extend(b["name"].as_str().and_then(tool_name));
                }
            }
            "message_start" => {
                let mut initial = usage(&v["message"]["usage"], true);
                // Initial output_tokens is a partial counter, commonly zero.
                // Only message_delta reports final output accounting.
                initial.output_tokens = None;
                initial.total_tokens = None;
                u.usage = Some(initial);
                u.model = v["message"]["model"].as_str().map(str::to_owned);
            }
            "message_delta" => {
                u.usage = Some(usage(&v["usage"], true));
                u.stop_reason = v["delta"]["stop_reason"].as_str().map(str::to_owned);
            }
            "message_stop" => u.terminal = Some("success"),
            "error" => u.terminal = Some("error"),
            _ => {}
        }
        if (kind.is_empty() || kind == "message") && v["content"].is_array() {
            u.usage = Some(usage(&v["usage"], true));
            u.model = v["model"].as_str().map(str::to_owned);
            u.stop_reason = v["stop_reason"].as_str().map(str::to_owned);
            u.terminal = Some(end_status(u.stop_reason.as_deref()));
        }
    } else if protocol == "chat-completions" {
        let c = &v["choices"][0];
        let delta = &c["delta"];
        visible = delta["content"].as_str().unwrap_or("");
        generated = visible;
        let reasoning = delta["reasoning_content"].as_str().unwrap_or("");
        u.estimated_output_tokens += estimate(reasoning);
        u.generated |= !reasoning.is_empty();
        if let Some(tools) = delta["tool_calls"].as_array() {
            u.tool_names = tools
                .iter()
                .take(8)
                .filter_map(|t| t["function"]["name"].as_str().and_then(tool_name))
                .collect();
            u.generated |= tools.iter().any(|t| {
                t["function"]["arguments"]
                    .as_str()
                    .is_some_and(|s| !s.is_empty())
            });
            u.estimated_output_tokens += tools
                .iter()
                .filter_map(|t| t["function"]["arguments"].as_str())
                .map(estimate)
                .sum::<f64>();
            u.tool = true;
        }
        if let Some(raw) = v.get("usage").filter(|v| !v.is_null()) {
            u.usage = Some(usage(
                &serde_json::json!({"input_tokens":raw["prompt_tokens"],"output_tokens":raw["completion_tokens"],"cached_input_tokens":raw["prompt_cache_hit_tokens"],"input_tokens_details":raw["prompt_tokens_details"],"output_tokens_details":raw["completion_tokens_details"]}),
                false,
            ));
        }
        u.model = v["model"].as_str().map(str::to_owned);
        u.stop_reason = c["finish_reason"].as_str().map(str::to_owned);
        if !c["message"].is_null() {
            u.terminal = Some(end_status(u.stop_reason.as_deref()));
        }
    } else {
        match kind {
            "response.output_item.added" if v["item"]["type"] == "function_call" => {
                u.tool = true;
                u.tool_names
                    .extend(v["item"]["name"].as_str().and_then(tool_name));
            }
            "response.output_text.delta" => {
                visible = v["delta"].as_str().unwrap_or("");
                generated = visible;
            }
            "response.function_call_arguments.delta" => {
                generated = v["delta"].as_str().unwrap_or("");
                u.tool = true;
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                generated = v["delta"].as_str().unwrap_or("")
            }
            "response.created" | "response.in_progress" => {
                u.model = v["response"]["model"].as_str().map(str::to_owned)
            }
            "response.completed" | "response.incomplete" | "response.failed" => {
                u.usage = Some(usage(&v["response"]["usage"], false));
                u.model = v["response"]["model"].as_str().map(str::to_owned);
                u.terminal = Some(if kind == "response.completed" {
                    "success"
                } else if kind == "response.incomplete" {
                    "incomplete"
                } else {
                    "error"
                });
            }
            "error" => u.terminal = Some("error"),
            _ => {
                if v["object"] == "response" {
                    u.usage = Some(usage(&v["usage"], false));
                    u.model = v["model"].as_str().map(str::to_owned);
                    u.terminal = match v["status"].as_str() {
                        Some("completed") => Some("success"),
                        Some("failed") => Some("error"),
                        Some("incomplete") => Some("incomplete"),
                        _ => None,
                    };
                }
            }
        }
    }
    u.generated |= !generated.is_empty();
    u.text = !visible.is_empty();
    u.estimated_tokens = estimate(visible);
    u.estimated_output_tokens += estimate(generated);
    u
}
fn estimate(text: &str) -> f64 {
    text.chars()
        .map(|c| {
            if c.is_ascii() {
                0.25
            } else if ('\u{4e00}'..='\u{9fff}').contains(&c) {
                1.0
            } else {
                0.5
            }
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn output_sampling_includes_reasoning_and_tool_arguments_without_body_or_usage_inflation() {
        for (protocol, reasoning, tools) in [
            (
                "messages",
                r#"{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"abcd"}}"#,
                r#"{"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"12345678"}}"#,
            ),
            (
                "chat-completions",
                r#"{"choices":[{"delta":{"reasoning_content":"abcd"}}]}"#,
                r#"{"choices":[{"delta":{"tool_calls":[{"function":{"arguments":"1234"}},{"function":{"arguments":"5678"}}]}}]}"#,
            ),
            (
                "responses",
                r#"{"type":"response.reasoning_text.delta","delta":"abcd"}"#,
                r#"{"type":"response.function_call_arguments.delta","delta":"12345678"}"#,
            ),
        ] {
            for (wire, expected, tool) in [(reasoning, 1.0, false), (tools, 2.0, true)] {
                let u = decode(wire.as_bytes(), protocol);
                assert!(u.generated && !u.text);
                assert_eq!(u.tool, tool);
                assert_eq!(u.estimated_output_tokens, expected, "{protocol}");
                assert_eq!(u.estimated_tokens, 0.0);
                assert!(u.usage.is_none());
            }
        }
        let mixed = decode(br#"{"choices":[{"delta":{"content":"abcd","reasoning_content":"1234","tool_calls":[{"function":{"arguments":"5678"}}]}}]}"#, "chat-completions");
        assert_eq!(mixed.estimated_output_tokens, 3.0);
        assert_eq!(mixed.estimated_tokens, 1.0);
        for wire in [
            r#"{"choices":[{"delta":{"role":"assistant"}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"function":{"name":"Read","arguments":""}}]}}]}"#,
            r#"{"choices":[],"usage":{"completion_tokens":100}}"#,
        ] {
            let u = decode(wire.as_bytes(), "chat-completions");
            assert!(!u.generated);
            assert_eq!(u.estimated_output_tokens, 0.0);
        }
    }
    #[test]
    fn tool_names_survive_argument_deltas_without_counting_names_as_first_output() {
        let mut d = StreamDecoder::new("messages");
        let first = d.feed(b"data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"name\":\"Read\"}}\n\n");
        assert_eq!(first[0].tool_names, vec!["Read"]);
        assert!(!first[0].generated);
        let next = d.feed(b"data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"name\":\"Grep\"}}\n\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{}\"}}\n\n");
        assert_eq!(next[1].tool_names, vec!["Read", "Grep"]);
        assert!(next[1].generated && next[1].tool && !next[1].text);
        let mut d = StreamDecoder::new("chat-completions");
        d.feed(b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"name\":\"read_\"}}]}}]}\n\n");
        let u = d.feed(b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"name\":\"file\",\"arguments\":\"{}\"}}]}}]}\n\n");
        assert_eq!(u[0].tool_names, vec!["read_file"]);
        let mut d = StreamDecoder::new("responses");
        let u = d.feed(b"data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"function_call\",\"name\":\"mcp__files__read\"}}\n\n");
        assert_eq!(u[0].tool_names, vec!["mcp__files__read"]);
        assert!(!u[0].generated);
    }
    #[test]
    fn stream_end_preserves_truncation_and_errors_until_the_protocol_terminal() {
        for (protocol, wire, expected) in [
            (
                "messages",
                "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\"}}\n\ndata: {\"type\":\"message_stop\"}\n\n",
                "incomplete",
            ),
            (
                "chat-completions",
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\ndata: {\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":8},\"choices\":[]}\n\ndata: [DONE]\n\n",
                "incomplete",
            ),
            (
                "chat-completions",
                "data: {\"error\":{\"type\":\"server_error\"}}\n\ndata: [DONE]\n\n",
                "error",
            ),
        ] {
            let mut decoder = StreamDecoder::new(protocol);
            let updates = decoder.feed(wire.as_bytes());
            assert_eq!(
                updates.iter().filter_map(|u| u.terminal).next_back(),
                Some(expected),
                "{protocol}"
            );
            if protocol == "chat-completions" && expected == "incomplete" {
                assert_eq!(
                    updates
                        .iter()
                        .filter_map(|u| u.usage.as_ref())
                        .next_back()
                        .unwrap()
                        .output_tokens,
                    Some(8)
                );
            }
        }
        assert!(decode(b"[DONE]", "messages").terminal.is_none());
        assert_eq!(
            decode(
                br#"{"type":"message","content":[],"stop_reason":"max_tokens"}"#,
                "messages"
            )
            .terminal,
            Some("incomplete")
        );
        assert_eq!(
            decode(br#"{"object":"response","status":"failed"}"#, "responses").terminal,
            Some("error")
        );
    }
    #[test]
    fn malformed_and_unterminated_streams_never_create_a_success() {
        let mut decoder = StreamDecoder::new("chat-completions");
        let updates = decoder.feed(b"data: {bad}\n\ndata: [DONE]\n\n");
        assert_eq!(
            updates.iter().filter_map(|u| u.terminal).next_back(),
            Some("interrupted")
        );
        let mut decoder = StreamDecoder::new("messages");
        assert!(
            decoder
                .feed(b"event: error\ndata: {\"type\":\"error\"}")
                .is_empty()
        );
        assert_eq!(
            decoder.end().iter().filter_map(|u| u.terminal).next_back(),
            Some("error")
        );
    }
    #[test]
    fn explicit_cost_and_deepseek_cache_metadata_are_not_text_estimates() {
        let u = decode(br#"{"usage":{"prompt_tokens":100,"completion_tokens":10,"prompt_cache_hit_tokens":80,"cost":0.002}}"#, "chat-completions");
        assert_eq!(u.reported_cost, Some(0.002));
        assert_eq!(u.usage.unwrap().cached_input_tokens, Some(80));
        assert_eq!(
            decode(br#"{"usage":{"cost":0}}"#, "chat-completions").reported_cost,
            Some(0.0)
        );
        assert_eq!(
            decode(br#"{"usage":{"cost":-1}}"#, "chat-completions").reported_cost,
            None
        );
        assert_eq!(
            decode(
                br#"{"type":"response.completed","response":{"usage":{"cost":0.3}}}"#,
                "responses"
            )
            .reported_cost,
            Some(0.3)
        );
        assert_eq!(
            decode(
                br#"{"usage":{"prompt_tokens":0,"completion_tokens":0}}"#,
                "chat-completions"
            )
            .reported_cost,
            None
        );
    }
    #[test]
    fn response_identity_uses_protocol_evidence_not_tool_item_or_http_id() {
        let r = decode(
            br#"{"type":"response.created","response":{"id":"resp_123","model":"m"}}"#,
            "responses",
        );
        assert_eq!(r.response_id.as_deref(), Some("resp_123"));
        let m = decode(
            br#"{"type":"message_start","message":{"id":"msg_123"}}"#,
            "messages",
        );
        assert_eq!(m.response_id.as_deref(), Some("msg_123"));
        let c = decode(br#"{"id":"chatcmpl_123","choices":[]}"#, "chat-completions");
        assert_eq!(c.response_id.as_deref(), Some("chatcmpl_123"));
        assert!(
            decode(
                br#"{"type":"response.output_item.added","id":"tool-item"}"#,
                "responses"
            )
            .response_id
            .is_none()
        );
        assert!(
            decode(
                br#"{"type":"message","id":"has space","content":[]}"#,
                "messages"
            )
            .response_id
            .is_none()
        );
    }
    #[test]
    fn heartbeats_headers_empty_and_split_utf8_do_not_fabricate_tokens() {
        let mut d = StreamDecoder::new("responses");
        assert!(d.feed(b": keepalive\r\n\r\n").is_empty());
        assert!(
            !d.feed(b"data: {\"type\":\"response.created\",\"response\":{\"model\":\"test\"}}\n\n")
                [0]
            .generated
        );
        let event = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"你abc\"}\r\n\r\n"
            .as_bytes();
        let mut updates = vec![];
        for byte in event {
            updates.extend(d.feed(&[*byte]));
        }
        assert_eq!(updates.len(), 1);
        assert!(updates[0].generated);
        assert_eq!(updates[0].estimated_tokens, 1.75);
        let u = decode(
            br#"{"type":"response.function_call_arguments.delta","delta":"{}"}"#,
            "responses",
        );
        assert!(u.generated && u.tool && !u.text);
        assert_eq!(u.estimated_tokens, 0.0);
    }
    #[test]
    fn usage_cache_and_reasoning_are_subsets_and_terminal_errors_are_explicit() {
        let error = decode(br#"{"error":{"type":"server_error"},"usage":{"cost":0.002,"prompt_tokens":10,"completion_tokens":2}}"#, "chat-completions");
        assert_eq!(error.terminal, Some("error"));
        assert_eq!(error.reported_cost, Some(0.002));
        assert_eq!(error.usage.unwrap().total_tokens, Some(12));
        let c=decode(br#"{"type":"message_start","message":{"usage":{"input_tokens":10,"cache_read_input_tokens":20,"cache_creation_input_tokens":5}}}"#,"messages").usage.unwrap();
        assert_eq!(c.input_tokens, Some(35));
        assert_eq!(c.total_tokens, None);
        let o=decode(br#"{"type":"response.completed","response":{"usage":{"input_tokens":100,"output_tokens":20,"input_tokens_details":{"cached_tokens":50},"output_tokens_details":{"reasoning_tokens":10}}}}"#,"responses");
        assert_eq!(o.usage.unwrap().total_tokens, Some(120));
        assert_eq!(o.terminal, Some("success"));
        assert_eq!(
            decode(br#"{"type":"error"}"#, "messages").terminal,
            Some("error")
        );
    }
    #[test]
    fn initial_messages_output_counter_is_not_final_accounting() {
        let initial = decode(br#"{"type":"message_start","message":{"usage":{"input_tokens":10,"output_tokens":0,"cache_read_input_tokens":80}}}"#, "messages").usage.unwrap();
        assert_eq!(initial.input_tokens, Some(90));
        assert_eq!(initial.output_tokens, None);
        assert_eq!(initial.total_tokens, None);
        let final_usage = decode(br#"{"type":"message_delta","usage":{"output_tokens":0},"delta":{"stop_reason":"end_turn"}}"#, "messages").usage.unwrap();
        assert_eq!(final_usage.output_tokens, Some(0)); // A real final zero remains zero.
    }
}
