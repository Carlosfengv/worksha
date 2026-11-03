use super::text::{
    content_text, duration, is_context, is_error, normalize_time, safe_text, str_at,
};
use super::{Builder, MAX_TEXT, Usage};
use serde_json::Value;

impl Builder {
    pub(super) fn codex(&mut self, v: &Value, ts: Option<&str>, line: usize) {
        let p = &v["payload"];
        let row_kind = str_at(v, "type").unwrap_or("");
        match row_kind {
            "session_meta" => {
                self.native_id = str_at(p, "id")
                    .or_else(|| str_at(p, "session_id"))
                    .map(str::to_owned);
                self.version = str_at(p, "cli_version").map(|s| safe_text(s, 100).0);
                self.project = str_at(p, "cwd").map(|s| safe_text(s, 2048).0);
                self.provider = str_at(p, "model_provider").map(|s| safe_text(s, 200).0);
                self.remote = str_at(&p["git"], "repository_url").map(|s| safe_text(s, 2048).0);
            }
            "turn_context" => {
                if let Some(turn) = str_at(p, "turn_id") {
                    self.turn = turn.into();
                }
                self.model = str_at(p, "model").map(|s| safe_text(s, 200).0);
            }
            "response_item" => {
                let kind = str_at(p, "type").unwrap_or("");
                let id = str_at(p, "id")
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("line:{line}"));
                if !self.seen_rows.insert(format!("response:{kind}:{id}")) {
                    return;
                }
                match kind {
                    "message" => {
                        let role = str_at(p, "role").unwrap_or("");
                        if !["user", "assistant"].contains(&role) {
                            return;
                        }
                        let text = content_text(&p["content"]);
                        if text.is_empty() {
                            return;
                        }
                        if role == "user" {
                            self.title_from(&text);
                        }
                        let index = self.event(
                            &format!("message:{id}"),
                            if is_context(&text) {
                                "event"
                            } else {
                                "message"
                            },
                            if is_context(&text) {
                                "会话上下文"
                            } else if role == "user" {
                                "用户"
                            } else {
                                "Codex 回复"
                            },
                            ts,
                            line,
                        );
                        self.events[index].role = Some(role.into());
                        self.write_text(index, "text", &text);
                    }
                    "function_call" | "custom_tool_call" => {
                        if let (Some(call), Some(name)) = (str_at(p, "call_id"), str_at(p, "name"))
                        {
                            let server = str_at(p, "namespace")
                                .and_then(|ns| ns.strip_prefix("mcp__"))
                                .filter(|server| !server.is_empty());
                            let qualified = server.map(|server| format!("mcp__{server}__{name}"));
                            let index = self.tool(
                                call,
                                qualified.as_deref().unwrap_or(name),
                                if kind == "function_call" {
                                    &p["arguments"]
                                } else {
                                    &p["input"]
                                },
                                ts,
                                line,
                            );
                            if let Some(server) = server {
                                self.events[index].server = Some(safe_text(server, 200).0);
                            }
                        }
                    }
                    "function_call_output" | "custom_tool_call_output" => {
                        if let Some(call) = str_at(p, "call_id") {
                            self.result(call, &p["output"], is_error(p), ts, line);
                        }
                    }
                    _ => {}
                }
            }
            "token_usage_record" => {
                if p["thread_token_usage"].is_object() {
                    self.total_usage = Some(Usage::parse(&p["thread_token_usage"], false));
                    self.usage_priority = 2;
                }
                if let Some(response) = str_at(p, "response_id") {
                    let index = self.event(
                        &format!("model:{response}"),
                        "model",
                        "模型响应用量",
                        ts,
                        line,
                    );
                    if let Some(turn) = str_at(p, "turn_id") {
                        self.events[index].turn_id = turn.into();
                    }
                    let usage = Usage::parse(&p["usage"], false);
                    self.events[index]
                        .usage
                        .get_or_insert_with(Usage::default)
                        .merge_max(&usage);
                    if self.content(index) {
                        self.events[index].raw_usage =
                            Some(safe_text(&p["usage"].to_string(), MAX_TEXT).0);
                    }
                }
            }
            "event_msg" => self.codex_event(p, ts, line),
            _ => {}
        }
    }
    pub(super) fn codex_event(&mut self, p: &Value, ts: Option<&str>, line: usize) {
        let kind = str_at(p, "type").unwrap_or("");
        match kind {
            "error" | "stream_error" | "turn_failed" => {
                if let Some(turn) = str_at(p, "turn_id") {
                    self.turn = turn.into();
                }
                let info = &p["codex_error_info"];
                let code = info.as_str().or_else(|| {
                    info.as_object()
                        .and_then(|v| v.keys().next())
                        .map(String::as_str)
                });
                let http = info
                    .as_object()
                    .and_then(|v| v.values().next())
                    .and_then(|v| v["http_status_code"].as_u64())
                    .and_then(|n| u16::try_from(n).ok());
                let detail = [p["message"].as_str(), p["additional_details"].as_str()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join("\n");
                let failure = crate::sessions::failure::from_parts(
                    code,
                    (!detail.is_empty()).then_some(detail.as_str()),
                    http,
                    None,
                    "codex_event",
                );
                self.failure_event(&format!("failure:{line}"), failure, ts, line);
            }
            "task_started" => {
                if let Some(turn) = str_at(p, "turn_id") {
                    self.turn = turn.into();
                }
            }
            "token_count"
                if self.usage_priority < 2 && p["info"]["total_token_usage"].is_object() =>
            {
                self.total_usage = Some(Usage::parse(&p["info"]["total_token_usage"], false));
                self.usage_priority = 1;
            }
            "item_completed" => {
                let item = &p["item"];
                let item_kind = str_at(item, "type").unwrap_or("");
                let Some(id) = str_at(item, "id") else {
                    return;
                };
                // Typed item completion carries execution status. It enriches the same call ID.
                let key = format!("call:{id}");
                let index = if let Some(index) = self.lookup.get(&key) {
                    Some(*index)
                } else if item_kind == "McpToolCall" {
                    let name = format!(
                        "mcp__{}__{}",
                        str_at(item, "server").unwrap_or("unknown"),
                        str_at(item, "tool").unwrap_or("unknown")
                    );
                    Some(self.tool(id, &name, &item["arguments"], ts, line))
                } else if item_kind == "CommandExecution" {
                    Some(self.tool(
                        id,
                        "命令执行",
                        &serde_json::json!({"command": item["command"], "cwd":item["cwd"]}),
                        ts,
                        line,
                    ))
                } else if item_kind == "FileChange" {
                    Some(self.tool(id, "文件修改", &item["changes"], ts, line))
                } else {
                    None
                };
                if let Some(index) = index {
                    self.events[index].agent_product_id = self.product.clone();
                    self.events[index].agent_attribution =
                        self.product.as_ref().map(|_| "native_record");
                    if let Some(turn) = str_at(p, "turn_id") {
                        self.events[index].turn_id = turn.into();
                    }
                    self.events[index].status = if item_kind == "McpToolCall"
                        && matches!(str_at(item, "status"), Some("cancelled" | "canceled"))
                    {
                        "cancelled"
                    } else if item_kind == "McpToolCall"
                        && matches!(str_at(item, "status"), Some("timed_out" | "timeout"))
                    {
                        "timeout"
                    } else if item_kind == "McpToolCall"
                        && str_at(item, "status") == Some("declined")
                    {
                        "declined"
                    } else if is_error(item) || is_error(&item["result"]) {
                        "error"
                    } else if item_kind == "McpToolCall"
                        && str_at(item, "status") != Some("completed")
                        && item["result"].is_null()
                    {
                        "missing_result"
                    } else {
                        "returned"
                    }
                    .into();
                    self.events[index].end_time = ts.and_then(normalize_time);
                    self.events[index].duration_ms = duration(&item["duration"]);
                    if item_kind == "FileChange"
                        && str_at(item, "status") == Some("completed")
                        && self.events[index].status != "error"
                    {
                        self.confirmed_writes.insert(id.into());
                    }
                    if !item["result"].is_null()
                        && self.events[index].output.is_none()
                        && (item_kind != "McpToolCall" || item["error"].is_null())
                    {
                        self.write_text(index, "output", &content_text(&item["result"]));
                    }
                    if item_kind == "McpToolCall" && !item["error"].is_null() {
                        // Codex typed completions can carry a separate RPC error
                        // instead of a tool result. Keep its readable details.
                        let output = if item["result"].is_null() {
                            item["error"].clone()
                        } else {
                            serde_json::json!({"result": item["result"], "error": item["error"]})
                        };
                        self.write_text(index, "output", &content_text(&output));
                    }
                    if let Some(output) =
                        str_at(item, "aggregated_output").or_else(|| str_at(item, "stdout"))
                        && self.events[index].output.is_none()
                    {
                        self.write_text(index, "output", output);
                    }
                }
            }
            "mcp_tool_call_end" => {
                if let Some(id) = str_at(p, "call_id") {
                    let output = if !p["error"].is_null() {
                        if p["result"].is_null() {
                            p["error"].clone()
                        } else {
                            serde_json::json!({"result": p["result"], "error": p["error"]})
                        }
                    } else {
                        p["result"].clone()
                    };
                    self.result(id, &output, is_error(p) || is_error(&p["result"]), ts, line);
                    if let Some(index) = self.lookup.get(&format!("call:{id}")) {
                        self.events[*index].duration_ms = duration(&p["duration"]);
                    }
                }
            }
            "task_complete" | "turn_aborted" => {
                let failed = !p["error"].is_null();
                if failed {
                    let failure = crate::sessions::failure::native(&p["error"], "codex_turn");
                    self.failure_event(&format!("failure:{line}"), failure, ts, line);
                }
                let index = self.event(
                    &format!("event:{line}"),
                    "event",
                    if failed {
                        "本轮未成功完成"
                    } else if kind == "task_complete" {
                        "本轮已完成，等待后续输入"
                    } else {
                        "本轮已中止"
                    },
                    ts,
                    line,
                );
                self.events[index].status = if failed {
                    "error"
                } else if kind == "task_complete" {
                    "recorded"
                } else {
                    "cancelled"
                }
                .into();
            }
            // agent_message/user_message are mirrors of response_item, not extra messages.
            _ => {}
        }
    }
}
