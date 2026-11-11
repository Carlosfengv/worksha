//! Client-facing text only. Upstream accounting and requests remain untouched.
use crate::AppState;
use axum::{
    body::{Body, Bytes},
    response::Response,
};
use futures_util::StreamExt;
use local_agent_core::routing::{Selection, Turn};
use serde_json::{Value, json};

const FRAME_LIMIT: usize = 8 * 1024 * 1024;
const PREFIX_LIMIT: usize = 4096;

/// Remove only gateway-owned status prefixes from assistant history. User messages,
/// signed thinking, tool inputs/results and unmatched model text are never edited.
pub(super) fn strip_history(body: &mut Value, issued: &[String]) {
    fn strip(text: &str, issued: &[String]) -> Option<String> {
        let s = text.trim_start();
        if !s.starts_with("【路由提示】") {
            return None;
        }
        let end = s.find('\n').unwrap_or(s.len());
        let line = s[..end].trim();
        if !issued.contains(&local_agent_core::providers::files::hash(line.as_bytes())) {
            return None;
        }
        Some(s[end..].trim_start_matches(['\r', '\n']).to_string())
    }
    if issued.is_empty() {
        return;
    }
    let Some(messages) = body["messages"].as_array_mut() else {
        return;
    };
    messages.retain_mut(|message| {
        if message["role"] != "assistant" {
            return true;
        }
        if let Some(text) = message["content"].as_str() {
            if let Some(rest) = strip(text, issued) {
                message["content"] = json!(rest);
                return !rest.is_empty();
            }
        } else if let Some(parts) = message["content"].as_array_mut() {
            let mut removed = false;
            parts.retain_mut(|part| {
                if part["type"] == "text"
                    && let Some(text) = part["text"].as_str()
                    && let Some(rest) = strip(text, issued)
                {
                    removed = true;
                    part["text"] = json!(rest);
                    return !rest.is_empty();
                }
                true
            });
            if removed && parts.is_empty() {
                return false;
            }
        }
        true
    });
}

fn output_contract(body: &Value) -> bool {
    let non_text = |v: &Value| !v.is_null() && v["type"] != "text";
    non_text(&body["response_format"])
        || non_text(&body["output_config"]["format"])
        || non_text(&body["text"]["format"])
        || [
            "json_schema",
            "output_schema",
            "outputSchema",
            "structured_output",
            "format",
        ]
        .iter()
        .any(|key| body.get(key).is_some_and(|v| !v.is_null()))
        || matches!(body["tool_choice"]["type"].as_str(), Some("any" | "tool"))
        || body["tool_choice"] == "required"
        || body["tools"]
            .as_array()
            .is_some_and(|tools| tools.iter().any(structured_tool))
        || json_only_prompt(body)
}

fn json_only_prompt(body: &Value) -> bool {
    let Some(user) = body["messages"]
        .as_array()
        .and_then(|messages| messages.iter().rev().find(|m| m["role"] == "user"))
    else {
        return false;
    };
    let text = user["content"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| {
            user["content"]
                .as_array()
                .map(|parts| {
                    parts
                        .iter()
                        .filter(|p| p["type"] == "text")
                        .filter_map(|p| p["text"].as_str())
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default()
        })
        .to_ascii_lowercase();
    text.contains("json")
        && [
            "只",
            "仅",
            "only",
            "valid json",
            "json格式",
            "json 格式",
            "json object",
            "json array",
        ]
        .iter()
        .any(|word| text.contains(word))
}

// Conservative guard for JSON-only prompts even when a client did not send a format flag.
// None means the first token is ambiguous; buffer a bounded prefix before deciding.
fn plain_prefix(text: &str) -> Option<bool> {
    let s = text.trim_start();
    let first = s.chars().next()?;
    if "{[\"`0123456789".contains(first) {
        return Some(false);
    }
    if first == '-' {
        let second = s.chars().nth(1)?;
        return Some(!second.is_ascii_digit());
    }
    for word in ["true", "false", "null"] {
        if word.starts_with(s) {
            return None;
        }
        if s.starts_with(word) {
            return Some(false);
        }
    }
    Some(true)
}
fn structured_tool(v: &Value) -> bool {
    let name = v["name"].as_str().unwrap_or("").to_ascii_lowercase();
    name.contains("structuredoutput")
        || name.contains("structured_output")
        || name.contains("json_output")
}

struct Claim {
    state: AppState,
    binding: String,
    turn: Turn,
    selected: Selection,
    allowed: bool,
}
impl Claim {
    fn take(&self, allow: bool) -> Option<String> {
        self.state
            .store
            .lock()
            .ok()?
            .model_service_route_notice(
                &self.binding,
                &self.turn,
                &self.selected,
                self.allowed && allow,
            )
            .ok()
            .flatten()
    }
}

pub(super) fn apply(
    response: Response,
    state: AppState,
    binding: String,
    turn: Turn,
    selected: Selection,
    request: &Bytes,
) -> Response {
    if !response.status().is_success()
        || !selected.fresh
        || selected.service.is_none()
        || turn.session.is_none()
        || turn.continuation
        || !turn.portable
        || response
            .headers()
            .get("content-encoding")
            .is_some_and(|v| v != "identity")
    {
        return response;
    }
    let Ok(request) = serde_json::from_slice::<Value>(request) else {
        return response;
    };
    if request.get("input").is_some() {
        return response;
    }
    let claim = Claim {
        state,
        binding,
        turn,
        selected,
        allowed: !output_contract(&request),
    };
    let sse = super::stream::is_event_stream(response.headers());
    let (mut parts, body) = response.into_parts();
    parts.headers.remove("content-length");
    let mut input = body.into_data_stream();
    let stream = async_stream::stream! {
        if sse {
            let mut rewrite = Rewriter::default();
            while let Some(chunk) = input.next().await {
                match chunk {
                    Ok(bytes) => { for output in rewrite.feed(&bytes, &|allow|claim.take(allow)) { yield Ok::<_,axum::Error>(Bytes::from(output)); } },
                    Err(error) => { let tail=rewrite.end();if !tail.is_empty(){yield Ok(Bytes::from(tail));}yield Err(error);return; }
                }
                if rewrite.terminated { return; }
            }
            let tail=rewrite.end();if !tail.is_empty(){yield Ok(Bytes::from(tail));}
        } else {
            let mut bytes=Vec::new();let mut oversized=false;
            while let Some(chunk)=input.next().await {
                match chunk {
                    Ok(chunk) if !oversized && bytes.len()+chunk.len()<=FRAME_LIMIT => bytes.extend_from_slice(&chunk),
                    Ok(chunk) => {if !bytes.is_empty(){yield Ok(Bytes::from(std::mem::take(&mut bytes)));}oversized=true;yield Ok(chunk);},
                    Err(error)=>{if !bytes.is_empty(){yield Ok(Bytes::from(bytes));}yield Err(error);return;}
                }
            }
            if !oversized { yield Ok(Bytes::from(rewrite_json(bytes,&|allow|claim.take(allow)))); }
        }
    };
    Response::from_parts(parts, Body::from_stream(stream))
}

fn rewrite_json(bytes: Vec<u8>, claim: &impl Fn(bool) -> Option<String>) -> Vec<u8> {
    let Ok(mut value) = serde_json::from_slice::<Value>(&bytes) else {
        return bytes;
    };
    if value["type"] != "message" || value["role"] != "assistant" || value.get("error").is_some() {
        return bytes;
    }
    let Some(content) = value["content"].as_array_mut() else {
        return bytes;
    };
    let Some(index) = content
        .iter()
        .position(|v| matches!(v["type"].as_str(), Some("text" | "tool_use")))
    else {
        return bytes;
    };
    let first = &content[index];
    let allow = if first["type"] == "tool_use" {
        !structured_tool(first)
    } else {
        plain_prefix(first["text"].as_str().unwrap_or("")) == Some(true)
    };
    if let Some(notice) = claim(allow) {
        content.insert(index, json!({"type":"text","text":notice}));
        serde_json::to_vec(&value).unwrap_or(bytes)
    } else {
        bytes
    }
}

#[derive(Default)]
struct Rewriter {
    pending: Vec<u8>,
    line_start: usize,
    held: Vec<Vec<u8>>,
    held_size: usize,
    prefix: String,
    index: Option<u64>,
    shift: Option<u64>,
    decided: bool,
    started: bool,
    passthrough: bool,
    terminated: bool,
}
impl Rewriter {
    fn feed(&mut self, bytes: &[u8], claim: &impl Fn(bool) -> Option<String>) -> Vec<Vec<u8>> {
        if self.passthrough {
            return vec![bytes.to_vec()];
        }
        let mut out = vec![];
        for (position, byte) in bytes.iter().enumerate() {
            self.pending.push(*byte);
            if self.pending.len() > FRAME_LIMIT {
                if self.shift.is_some() {
                    out.push(event(&json!({"type":"error","error":{"type":"api_error","message":"响应事件超过路由提示处理上限，未拼接其他模型输出"}})));
                    self.terminated = true;
                    break;
                }
                // Never corrupt or truncate an oversized/unsupported frame. No notice is injected.
                out.append(&mut self.held);
                out.push(std::mem::take(&mut self.pending));
                out.push(bytes[position + 1..].to_vec());
                self.passthrough = true;
                break;
            }
            if *byte == b'\n' {
                let line = &self.pending[self.line_start..self.pending.len() - 1];
                if line.is_empty() || line == b"\r" {
                    let frame = std::mem::take(&mut self.pending);
                    self.line_start = 0;
                    out.extend(self.frame(frame, claim));
                } else {
                    self.line_start = self.pending.len();
                }
            }
        }
        out
    }
    fn frame(&mut self, raw: Vec<u8>, claim: &impl Fn(bool) -> Option<String>) -> Vec<Vec<u8>> {
        let Some(mut value) = parse_frame(&raw) else {
            return vec![raw];
        };
        if value["type"] == "message_start" {
            self.started = value["message"]["role"] == "assistant";
        }
        if self.decided {
            return vec![self.shifted(raw, &mut value)];
        }
        if !self.started {
            return vec![raw];
        }
        let start = value["type"] == "content_block_start";
        if start && value["content_block"]["type"] == "tool_use" && self.index.is_none() {
            self.index = value["index"].as_u64().filter(|v| *v < 1024);
            self.held.push(raw);
            return self.decide(!structured_tool(&value["content_block"]), claim);
        }
        if start && value["content_block"]["type"] == "text" && self.index.is_none() {
            self.index = value["index"].as_u64().filter(|v| *v < 1024);
            self.prefix
                .push_str(value["content_block"]["text"].as_str().unwrap_or(""));
        }
        if self.index.is_some() {
            if value["type"] == "content_block_delta" && value["delta"]["type"] == "text_delta" {
                self.prefix
                    .push_str(value["delta"]["text"].as_str().unwrap_or(""));
            }
            self.held_size += raw.len();
            self.held.push(raw);
            if let Some(allow) = plain_prefix(&self.prefix) {
                return self.decide(allow, claim);
            }
            if self.prefix.len() >= PREFIX_LIMIT
                || self.held_size > 64 * 1024
                || matches!(
                    value["type"].as_str(),
                    Some("content_block_stop" | "message_stop" | "error")
                )
            {
                return self.decide(false, claim);
            }
            vec![]
        } else {
            vec![raw]
        }
    }
    fn decide(&mut self, allow: bool, claim: &impl Fn(bool) -> Option<String>) -> Vec<Vec<u8>> {
        self.decided = true;
        let mut out = vec![];
        if let Some(index) = self.index
            && let Some(notice) = claim(allow)
        {
            self.shift = Some(index);
            for value in [
                json!({"type":"content_block_start","index":index,"content_block":{"type":"text","text":""}}),
                json!({"type":"content_block_delta","index":index,"delta":{"type":"text_delta","text":notice}}),
                json!({"type":"content_block_stop","index":index}),
            ] {
                out.push(event(&value));
            }
        }
        for raw in std::mem::take(&mut self.held) {
            let rewritten = if let Some(mut value) = parse_frame(&raw) {
                self.shifted(raw, &mut value)
            } else {
                raw
            };
            out.push(rewritten);
        }
        self.prefix.clear();
        out
    }
    fn shifted(&self, raw: Vec<u8>, value: &mut Value) -> Vec<u8> {
        if let Some(first) = self.shift
            && value["type"]
                .as_str()
                .is_some_and(|s| s.starts_with("content_block_"))
            && let Some(index) = value["index"].as_u64().filter(|i| *i >= first)
        {
            value["index"] = json!(index + 1);
            // Preserve SSE id/retry/comment fields while replacing only the JSON data lines.
            let mut output = Vec::new();
            for line in raw.split(|b| *b == b'\n') {
                let line = line.strip_suffix(b"\r").unwrap_or(line);
                if !line.is_empty() && !line.starts_with(b"data:") {
                    output.extend_from_slice(line);
                    output.push(b'\n');
                }
            }
            output.extend_from_slice(format!("data: {value}\n\n").as_bytes());
            output
        } else {
            raw
        }
    }
    fn end(&mut self) -> Vec<u8> {
        self.held
            .drain(..)
            .flatten()
            .chain(std::mem::take(&mut self.pending))
            .collect()
    }
}
fn event(value: &Value) -> Vec<u8> {
    format!(
        "event: {}\ndata: {value}\n\n",
        value["type"].as_str().unwrap_or("")
    )
    .into_bytes()
}
fn parse_frame(bytes: &[u8]) -> Option<Value> {
    let mut data = Vec::new();
    for line in bytes.split(|b| *b == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if let Some(value) = line.strip_prefix(b"data:") {
            if !data.is_empty() {
                data.push(b'\n');
            }
            data.extend_from_slice(value.strip_prefix(b" ").unwrap_or(value));
        }
    }
    serde_json::from_slice(&data).ok()
}

#[cfg(test)]
mod tests;
