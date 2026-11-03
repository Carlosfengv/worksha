use super::text::{content_text, is_context, safe_text, str_at};
use super::{Builder, MAX_TEXT, Usage};
use serde_json::Value;

impl Builder {
    pub(super) fn claude(&mut self, v: &Value, ts: Option<&str>, line: usize) {
        if v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            return;
        }
        if let Some(id) = str_at(v, "sessionId") {
            self.native_id.get_or_insert_with(|| id.into());
        }
        if let Some(version) = str_at(v, "version") {
            self.version = Some(safe_text(version, 100).0);
        }
        if let Some(cwd) = str_at(v, "cwd")
            && self.project.is_none()
        {
            self.project = Some(safe_text(cwd, 2048).0);
        }
        let kind = str_at(v, "type").unwrap_or("");
        if kind == "custom-title"
            && let Some(title) = str_at(v, "customTitle").or_else(|| str_at(v, "title"))
        {
            self.title = Some(safe_text(title, 200).0);
            self.title_source = "native_title".into();
        }
        if kind == "system" && str_at(v, "subtype") == Some("api_error") {
            let key = format!(
                "failure:{}",
                str_at(v, "uuid")
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("line:{line}"))
            );
            let failure = crate::sessions::failure::native(&v["error"], "claude_api_error");
            let index = self.failure_event(&key, failure, ts, line);
            self.events[index].model = str_at(v, "model")
                .filter(|s| *s != "<synthetic>")
                .map(|s| safe_text(s, 300).0);
            return;
        }
        if !["user", "assistant"].contains(&kind) {
            return;
        }
        let row_id = str_at(v, "uuid")
            .map(str::to_owned)
            .unwrap_or_else(|| format!("line:{line}"));
        if !self.seen_rows.insert(row_id.clone()) {
            return;
        }
        let message = &v["message"];
        let content = &message["content"];
        if kind == "assistant" && v["isApiErrorMessage"] == true {
            let text = content_text(content);
            let failure = crate::sessions::failure::from_parts(
                v["error"].as_str(),
                Some(&text),
                v["apiErrorStatus"]
                    .as_u64()
                    .and_then(|n| u16::try_from(n).ok())
                    .filter(|n| (100..=599).contains(n)),
                None,
                "claude_api_message",
            );
            let index = self.failure_event(&format!("failure:{row_id}"), failure, ts, line);
            self.events[index].model = str_at(message, "model")
                .filter(|s| *s != "<synthetic>")
                .map(|s| safe_text(s, 300).0);
            return;
        }
        let has_tool_result = content.as_array().is_some_and(|blocks| {
            blocks
                .iter()
                .any(|b| str_at(b, "type") == Some("tool_result"))
        });
        if kind == "user" && !has_tool_result {
            self.turn = str_at(v, "promptId").unwrap_or(&row_id).into();
            let text = content_text(content);
            self.title_from(&text);
            let index = self.event(
                &format!("message:{row_id}"),
                if is_context(&text) {
                    "event"
                } else {
                    "message"
                },
                if is_context(&text) {
                    "会话上下文"
                } else {
                    "用户"
                },
                ts,
                line,
            );
            self.events[index].role = Some("user".into());
            self.write_text(index, "text", &text);
        }
        if kind == "assistant" {
            self.model = str_at(message, "model").map(|s| safe_text(s, 200).0);
            let request_id = str_at(message, "id");
            let model_call =
                request_id.is_some() && self.model.as_deref().is_some_and(|m| m != "<synthetic>");
            let key = format!(
                "{}:{}",
                if model_call { "model" } else { "message" },
                request_id.unwrap_or(&row_id)
            );
            let index = self.event(
                &key,
                if model_call { "model" } else { "message" },
                "Claude 回复",
                ts,
                line,
            );
            self.events[index].role = Some("assistant".into());
            if message["usage"].is_object() {
                let usage = Usage::parse(&message["usage"], true);
                self.events[index]
                    .usage
                    .get_or_insert_with(Usage::default)
                    .merge_max(&usage);
                if self.content(index) {
                    self.events[index].raw_usage =
                        Some(safe_text(&message["usage"].to_string(), MAX_TEXT).0);
                }
            }
            if let Some(blocks) = content.as_array() {
                for block in blocks {
                    match str_at(block, "type").unwrap_or("") {
                        "text" => {
                            self.write_text(index, "text", str_at(block, "text").unwrap_or(""))
                        }
                        "tool_use" => {
                            if let (Some(id), Some(name)) =
                                (str_at(block, "id"), str_at(block, "name"))
                            {
                                let tool = self.tool(id, name, &block["input"], ts, line);
                                if model_call {
                                    self.events[tool].model_response_id = Some(key.clone());
                                }
                            }
                        }
                        // Thinking and encrypted reasoning are intentionally not reconstructed.
                        _ => {}
                    }
                }
            } else if let Some(text) = content.as_str() {
                self.write_text(index, "text", text);
            }
        }
        if let Some(blocks) = content.as_array() {
            for block in blocks {
                if str_at(block, "type") == Some("tool_result")
                    && let Some(id) = str_at(block, "tool_use_id")
                {
                    self.result(
                        id,
                        &block["content"],
                        block["is_error"].as_bool().unwrap_or(false),
                        ts,
                        line,
                    );
                }
            }
        }
    }
}
