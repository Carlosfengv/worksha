use super::protocol::{Answer, Result};
use serde_json::{Value, json};
use std::collections::HashMap;

pub enum Delta {
    Text(String),
    Refusal(String),
    Reasoning(String),
    Tool(usize, Option<String>, Option<String>, String),
    ToolDone(usize, String),
    Usage(Option<u64>, Option<u64>, Option<u64>),
    Finish(String),
    Snapshot(Answer),
    End,
}
pub struct Decoder {
    protocol: String,
    reasoning_scope: Option<String>,
    hide_intrinsic_reasoning: bool,
    pending: Vec<u8>,
    data: Vec<u8>,
    ended: bool,
    rejection: Option<Value>,
}
impl Decoder {
    pub fn new(protocol: &str) -> Self {
        Self {
            protocol: protocol.into(),
            reasoning_scope: None,
            hide_intrinsic_reasoning: false,
            pending: vec![],
            data: vec![],
            ended: false,
            rejection: None,
        }
    }
    pub fn with_provider(mut self, provider: &local_agent_core::Provider) -> Self {
        self.reasoning_scope = super::deepseek::scope(provider);
        self
    }
    pub(super) fn hide_intrinsic_reasoning(mut self, hide: bool) -> Self {
        self.hide_intrinsic_reasoning = hide;
        self
    }
    pub(crate) fn take_rejection(&mut self) -> Option<Value> {
        self.rejection.take()
    }
    pub fn feed(&mut self, bytes: &[u8]) -> Result<Vec<Delta>> {
        let mut out = vec![];
        self.feed_each(bytes, |delta| {
            out.push(delta);
            Ok(())
        })?;
        Ok(out)
    }
    /// Deliver each decoded event in wire order even if a later event fails.
    pub(crate) fn feed_each(
        &mut self,
        bytes: &[u8],
        mut emit: impl FnMut(Delta) -> Result<()>,
    ) -> Result<()> {
        for byte in bytes {
            if *byte == b'\n' {
                let line = std::mem::take(&mut self.pending);
                let line = line.strip_suffix(b"\r").unwrap_or(&line);
                if line.is_empty() {
                    if !self.data.is_empty() {
                        let data = std::mem::take(&mut self.data);
                        if self.ended {
                            return Err("协议结束后仍返回额外输出");
                        }
                        if data == b"[DONE]" {
                            if self.protocol != "chat-completions" {
                                return Err("上游结束标记不匹配");
                            };
                            self.ended = true;
                            emit(Delta::End)?;
                        } else {
                            let v: Value =
                                serde_json::from_slice(&data).map_err(|_| "上游流不是有效 JSON")?;
                            if super::protocol::reply_error(&v).is_some()
                                || v["type"] == "response.failed"
                            {
                                self.rejection = Some(v);
                                return Err("上游返回协议错误，转换流已停止");
                            }
                            let next = self.event(&v)?;
                            if next.iter().any(|d| matches!(d, Delta::End)) {
                                self.ended = true;
                            }
                            for delta in next {
                                emit(delta)?;
                            }
                        }
                    }
                } else if let Some(data) = line.strip_prefix(b"data:") {
                    if !self.data.is_empty() {
                        self.data.push(b'\n')
                    }
                    self.data
                        .extend_from_slice(data.strip_prefix(b" ").unwrap_or(data));
                }
            } else {
                self.pending.push(*byte);
            }
            if self.pending.len() + self.data.len() > 8 * 1024 * 1024 {
                return Err("上游单个流事件超过 8 MiB");
            }
        }
        Ok(())
    }
    pub fn end(&mut self) -> Result<Vec<Delta>> {
        self.feed(b"\n\n")
    }
    fn event(&self, v: &Value) -> Result<Vec<Delta>> {
        let mut out = vec![];
        if self.protocol == "chat-completions" {
            if let Some(usage) = v.get("usage").filter(|x| !x.is_null()) {
                out.push(Delta::Usage(
                    usage["prompt_tokens"].as_u64(),
                    usage["completion_tokens"].as_u64(),
                    usage["prompt_cache_hit_tokens"]
                        .as_u64()
                        .or_else(|| usage["prompt_tokens_details"]["cached_tokens"].as_u64()),
                ));
            }
            if v["choices"].as_array().is_some_and(|c| c.len() > 1) {
                return Err("转换流只支持单个候选");
            }
            if let Some(choice) = v["choices"].as_array().and_then(|c| c.first()) {
                let d = &choice["delta"];
                if !self.hide_intrinsic_reasoning
                    && let Some(reasoning) = d.get("reasoning_content").filter(|v| !v.is_null())
                {
                    let text = reasoning.as_str().ok_or("上游思考增量需要字符串")?;
                    if !text.is_empty() || self.reasoning_scope.is_some() {
                        if self.reasoning_scope.is_none() {
                            return Err("上游返回当前不可转换的思考内容，请使用同协议连接");
                        }
                        out.push(Delta::Reasoning(text.into()));
                    }
                }
                if let Some(text) = d["content"].as_str().filter(|s| !s.is_empty()) {
                    out.push(Delta::Text(text.into()));
                }
                if let Some(refusal) = d.get("refusal").filter(|v| !v.is_null()) {
                    let text = refusal.as_str().ok_or("上游拒绝增量需要字符串")?;
                    if !text.is_empty() {
                        out.push(Delta::Refusal(text.into()));
                    }
                }
                for tool in d["tool_calls"].as_array().into_iter().flatten() {
                    out.push(Delta::Tool(
                        index(&tool["index"])?,
                        tool["id"].as_str().map(str::to_owned),
                        tool["function"]["name"].as_str().map(str::to_owned),
                        tool["function"]["arguments"].as_str().unwrap_or("").into(),
                    ));
                }
                if let Some(reason) = choice["finish_reason"].as_str() {
                    out.push(Delta::Finish(reason.into()));
                }
            }
        } else if self.protocol == "messages" {
            match v["type"].as_str() {
                Some("message_start") => {
                    let u = &v["message"]["usage"];
                    out.push(Delta::Usage(
                        u["input_tokens"].as_u64().map(|n| {
                            n.saturating_add(u["cache_read_input_tokens"].as_u64().unwrap_or(0))
                                .saturating_add(
                                    u["cache_creation_input_tokens"].as_u64().unwrap_or(0),
                                )
                        }),
                        None,
                        u["cache_read_input_tokens"].as_u64(),
                    ));
                }
                Some("content_block_start") => {
                    let p = &v["content_block"];
                    match p["type"].as_str() {
                        Some("text") => {
                            if let Some(t) = p["text"].as_str().filter(|s| !s.is_empty()) {
                                out.push(Delta::Text(t.into()));
                            }
                        }
                        Some("tool_use") => {
                            out.push(Delta::Tool(
                                index(&v["index"])?,
                                p["id"].as_str().map(str::to_owned),
                                p["name"].as_str().map(str::to_owned),
                                if p["input"].as_object().is_some_and(|m| !m.is_empty()) {
                                    p["input"].to_string()
                                } else {
                                    String::new()
                                },
                            ));
                        }
                        Some("thinking" | "redacted_thinking") if self.hide_intrinsic_reasoning => {
                        }
                        _ => return Err("上游内容块尚不能跨协议转换"),
                    }
                }
                Some("content_block_delta") => match v["delta"]["type"].as_str() {
                    Some("text_delta") => out.push(Delta::Text(
                        v["delta"]["text"].as_str().ok_or("文本增量无效")?.into(),
                    )),
                    Some("input_json_delta") => out.push(Delta::Tool(
                        index(&v["index"])?,
                        None,
                        None,
                        v["delta"]["partial_json"]
                            .as_str()
                            .ok_or("工具参数增量无效")?
                            .into(),
                    )),
                    Some("thinking_delta" | "signature_delta") if self.hide_intrinsic_reasoning => {
                    }
                    _ => return Err("思考签名与此增量类型尚不能跨协议转换"),
                },
                Some("message_delta") => {
                    let reason = match v["delta"]["stop_reason"].as_str() {
                        Some("end_turn" | "stop_sequence") => "stop",
                        Some("tool_use") => "tool_calls",
                        Some("max_tokens") => "length",
                        _ => return Err("上游结束原因不可转换"),
                    };
                    out.push(Delta::Finish(reason.into()));
                    out.push(Delta::Usage(
                        None,
                        v["usage"]["output_tokens"].as_u64(),
                        None,
                    ));
                }
                Some("message_stop") => out.push(Delta::End),
                _ => {}
            }
        } else {
            match v["type"].as_str() {
                Some("response.output_text.delta") => out.push(Delta::Text(
                    v["delta"].as_str().ok_or("Responses 文本增量无效")?.into(),
                )),
                Some("response.output_item.added") => {
                    let item = &v["item"];
                    if item["type"] == "function_call" {
                        out.push(Delta::Tool(
                            index(&v["output_index"])?,
                            item["call_id"].as_str().map(str::to_owned),
                            item["name"].as_str().map(str::to_owned),
                            item["arguments"].as_str().unwrap_or("").into(),
                        ));
                    } else if item["type"] != "message" && item["type"] != "reasoning" {
                        return Err("Responses 原生工具尚不能转换");
                    }
                }
                Some("response.function_call_arguments.delta") => out.push(Delta::Tool(
                    index(&v["output_index"])?,
                    None,
                    None,
                    v["delta"].as_str().ok_or("工具参数增量无效")?.into(),
                )),
                Some("response.function_call_arguments.done") => out.push(Delta::ToolDone(
                    index(&v["output_index"])?,
                    v["arguments"].as_str().ok_or("工具参数无效")?.into(),
                )),
                Some("response.completed" | "response.incomplete") => {
                    let answer = if self.hide_intrinsic_reasoning {
                        let mut response = v["response"].clone();
                        super::protocol::remove_intrinsic_reasoning("responses", &mut response);
                        Answer::parse("responses", &response)?
                    } else {
                        Answer::parse("responses", &v["response"])?
                    };
                    out.push(Delta::Snapshot(answer));
                    out.push(Delta::End);
                }
                Some("response.refusal.delta") => out.push(Delta::Refusal(
                    v["delta"].as_str().ok_or("上游拒绝增量需要字符串")?.into(),
                )),
                Some("response.reasoning_summary_text.delta" | "response.reasoning_text.delta")
                    if !self.hide_intrinsic_reasoning =>
                {
                    return Err("思考增量尚不能跨协议转换");
                }
                _ => {}
            }
        }
        Ok(out)
    }
}
fn index(v: &Value) -> Result<usize> {
    v.as_u64()
        .filter(|n| *n < 1024)
        .map(|n| n as usize)
        .ok_or("工具索引无效或超过限制")
}
pub struct Encoder {
    buffered: usize,
    tool_calls_allowed: bool,
    registry: super::tools::Tools,
    protocol: String,
    id: String,
    model: String,
    answer: Answer,
    tools: HashMap<usize, usize>,
    opened: HashMap<usize, usize>,
    order: Vec<Block>,
    text_index: Option<usize>,
    refusal_index: Option<usize>,
    reasoning_started: bool,
    reasoning_closed: bool,
    sequence: u64,
    finished: bool,
}
#[derive(Clone)]
enum Block {
    Text,
    Refusal,
    Tool(usize),
}
impl Encoder {
    pub fn response_id(&self) -> &str {
        &self.id
    }
    pub fn new(protocol: &str, model: &str) -> Self {
        Self {
            buffered: 0,
            tool_calls_allowed: true,
            registry: super::tools::Tools::default(),
            protocol: protocol.into(),
            id: format!("resp_{}", uuid::Uuid::new_v4().simple()),
            model: model.into(),
            answer: Answer::default(),
            tools: HashMap::new(),
            opened: HashMap::new(),
            order: vec![],
            text_index: None,
            refusal_index: None,
            reasoning_started: false,
            reasoning_closed: false,
            sequence: 0,
            finished: false,
        }
    }
    pub fn with_provider(mut self, provider: &local_agent_core::Provider) -> Self {
        self.answer.reasoning_scope = super::deepseek::scope(provider);
        self
    }
    pub fn with_tool_calls(mut self, allowed: bool) -> Self {
        self.tool_calls_allowed = allowed;
        self
    }
    fn event(&mut self, kind: &str, mut v: Value) -> String {
        v["type"] = json!(kind);
        if self.protocol == "responses" {
            v["sequence_number"] = json!(self.sequence);
            self.sequence += 1;
        }
        format!("event: {kind}\ndata: {v}\n\n")
    }
    pub fn with_tools(mut self, tools: super::tools::Tools) -> Self {
        self.registry = tools;
        self
    }
    pub fn start(&mut self) -> String {
        if self.protocol == "messages" {
            self.event("message_start",json!({"message":{"id":self.id,"type":"message","role":"assistant","model":self.model,"content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":0,"output_tokens":0}}}))
        } else {
            let mut v = self
                .answer
                .render("responses", &self.id, &self.model)
                .unwrap_or_default();
            v["status"] = json!("in_progress");
            v["usage"] = Value::Null;
            let a = self.event("response.created", json!({"response":v}));
            a + &self.event("response.in_progress", json!({"response":v}))
        }
    }
    fn reasoning(&mut self, text: &str) -> Result<String> {
        if text.is_empty() && self.reasoning_closed {
            return Ok(String::new());
        }
        let scope = self
            .answer
            .reasoning_scope
            .as_deref()
            .ok_or("思考内容缺少来源")?;
        if self.reasoning_closed || !self.order.is_empty() {
            return Err("思考内容出现在正文或工具调用之后");
        }
        let mut out = String::new();
        if !self.reasoning_started {
            let item = super::deepseek::item(scope, &self.id, "");
            self.reasoning_started = true;
            if self.protocol == "messages" {
                out += &self.event("content_block_start", json!({"index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}));
            } else {
                out += &self.event(
                    "response.output_item.added",
                    json!({"output_index":0,"item":item}),
                );
                out += &self.event("response.reasoning_summary_part.added", json!({"output_index":0,"item_id":item["id"],"summary_index":0,"part":{"type":"summary_text","text":""}}));
            }
        }
        self.answer.reasoning.push_str(text);
        self.answer.reasoning_reported = true;
        out += &if self.protocol == "messages" {
            self.event(
                "content_block_delta",
                json!({"index":0,"delta":{"type":"thinking_delta","thinking":text}}),
            )
        } else {
            let item = super::deepseek::item(
                self.answer.reasoning_scope.as_deref().unwrap(),
                &self.id,
                "",
            );
            self.event(
                "response.reasoning_summary_text.delta",
                json!({"output_index":0,"item_id":item["id"],"summary_index":0,"delta":text}),
            )
        };
        Ok(out)
    }
    fn close_reasoning(&mut self) -> String {
        if !self.reasoning_started || self.reasoning_closed {
            return String::new();
        }
        self.reasoning_closed = true;
        let scope = self.answer.reasoning_scope.as_deref().unwrap();
        if self.protocol == "messages" {
            let marker = super::deepseek::marker(scope);
            self.event(
                "content_block_delta",
                json!({"index":0,"delta":{"type":"signature_delta","signature":marker}}),
            ) + &self.event("content_block_stop", json!({"index":0}))
        } else {
            let item = super::deepseek::item(scope, &self.id, &self.answer.reasoning);
            self.event("response.reasoning_summary_text.done", json!({"output_index":0,"item_id":item["id"],"summary_index":0,"text":self.answer.reasoning}))
                + &self.event("response.reasoning_summary_part.done", json!({"output_index":0,"item_id":item["id"],"summary_index":0,"part":item["summary"][0]}))
                + &self.event("response.output_item.done", json!({"output_index":0,"item":item}))
        }
    }
    fn text(&mut self, text: &str) -> String {
        let mut out = String::new();
        let i = if let Some(i) = self.text_index {
            i
        } else {
            let i = self.order.len() + usize::from(self.reasoning_started);
            self.order.push(Block::Text);
            self.text_index = Some(i);
            if self.protocol == "messages" {
                out += &self.event(
                    "content_block_start",
                    json!({"index":i,"content_block":{"type":"text","text":""}}),
                );
            } else {
                out+=&self.event("response.output_item.added",json!({"output_index":i,"item":{"id":format!("msg_{}",self.id),"type":"message","role":"assistant","status":"in_progress","content":[]}}));
                out+=&self.event("response.content_part.added",json!({"output_index":i,"item_id":format!("msg_{}",self.id),"content_index":0,"part":{"type":"output_text","text":"","annotations":[]}}));
            }
            i
        };
        self.answer.text.push_str(text);
        out += &if self.protocol == "messages" {
            self.event(
                "content_block_delta",
                json!({"index":i,"delta":{"type":"text_delta","text":text}}),
            )
        } else {
            self.event("response.output_text.delta",json!({"output_index":i,"item_id":format!("msg_{}",self.id),"content_index":0,"delta":text}))
        };
        out
    }
    fn refusal(&mut self, text: &str) -> String {
        let mut out = String::new();
        let i = if let Some(i) = self.refusal_index {
            i
        } else {
            let i = self.order.len() + usize::from(self.reasoning_started);
            self.order.push(Block::Refusal);
            self.refusal_index = Some(i);
            if self.protocol == "messages" {
                out += &self.event(
                    "content_block_start",
                    json!({"index":i,"content_block":{"type":"text","text":""}}),
                );
            } else {
                out += &self.event("response.output_item.added",json!({"output_index":i,"item":{"id":format!("refusal_{}",self.id),"type":"message","role":"assistant","status":"in_progress","content":[]}}));
                out += &self.event("response.content_part.added",json!({"output_index":i,"item_id":format!("refusal_{}",self.id),"content_index":0,"part":{"type":"refusal","refusal":""}}));
            }
            i
        };
        self.answer.refusal.push_str(text);
        out += &if self.protocol == "messages" {
            self.event(
                "content_block_delta",
                json!({"index":i,"delta":{"type":"text_delta","text":text}}),
            )
        } else {
            self.event("response.refusal.delta",json!({"output_index":i,"item_id":format!("refusal_{}",self.id),"content_index":0,"delta":text}))
        };
        out
    }
    fn tool(
        &mut self,
        slot: usize,
        id: Option<String>,
        name: Option<String>,
        args: String,
    ) -> Result<String> {
        let pos = if let Some(pos) = self.tools.get(&slot) {
            *pos
        } else {
            let pos = self.answer.tools.len();
            self.answer
                .tools
                .push(json!({"id":"","type":"function","function":{"name":"","arguments":""}}));
            self.tools.insert(slot, pos);
            pos
        };
        let t = &mut self.answer.tools[pos];
        let mut id = id.filter(|s| !s.is_empty());
        let mut name = name.filter(|s| !s.is_empty());
        if self.opened.contains_key(&pos) {
            if id.as_ref().is_some_and(|s| t["id"] != s.as_str())
                || name
                    .as_ref()
                    .is_some_and(|s| t["function"]["name"] != s.as_str())
            {
                return Err("工具元数据在参数流开始后发生变化");
            }
            id = None;
            name = None;
        }
        for (pointer, value) in [
            ("/id", id),
            ("/function/name", name),
            ("/function/arguments", Some(args.clone())),
        ] {
            if let Some(value) = value {
                let field = t.pointer_mut(pointer).ok_or("工具字段无效")?;
                if let Value::String(text) = field {
                    text.push_str(&value)
                } else {
                    return Err("工具字段不是字符串");
                }
            }
        }
        if t["id"] == "" || t["function"]["name"] == "" {
            return Ok(String::new());
        }
        let mut out = String::new();
        let new_tool = !self.opened.contains_key(&pos);
        let i = if let Some(i) = self.opened.get(&pos) {
            *i
        } else {
            let i = self.order.len() + usize::from(self.reasoning_started);
            self.order.push(Block::Tool(pos));
            self.opened.insert(pos, i);
            let t = self.answer.tools[pos].clone();
            if self.protocol == "messages" {
                out+=&self.event("content_block_start",json!({"index":i,"content_block":{"type":"tool_use","id":t["id"],"name":self.registry.name(t["function"]["name"].as_str().ok_or("工具名称无效")?),"input":{}}}));
            } else {
                let item=self.registry.response_item(json!({"type":"function_call","id":format!("fc_{}_{}",self.id,pos),"call_id":t["id"],"name":t["function"]["name"],"arguments":"","status":"in_progress"}))?;
                out += &self.event(
                    "response.output_item.added",
                    json!({"output_index":i,"item":item}),
                );
            }
            i
        };
        let args = if new_tool {
            self.answer.tools[pos]["function"]["arguments"]
                .as_str()
                .unwrap_or("")
                .to_string()
        } else {
            args
        };
        if !args.is_empty() {
            out += &if self.protocol == "messages" {
                self.event(
                    "content_block_delta",
                    json!({"index":i,"delta":{"type":"input_json_delta","partial_json":args}}),
                )
            } else if !self.registry.is_custom(
                self.answer.tools[pos]["function"]["name"]
                    .as_str()
                    .unwrap_or(""),
            ) {
                self.event("response.function_call_arguments.delta",json!({"output_index":i,"item_id":format!("fc_{}_{}",self.id,pos),"delta":args}))
            } else {
                String::new()
            };
        }
        Ok(out)
    }
    pub fn push(&mut self, delta: Delta) -> Result<String> {
        if self.finished {
            return Err("已结束的转换流收到额外输出");
        }
        if !self.tool_calls_allowed
            && match &delta {
                Delta::Tool(..) | Delta::ToolDone(..) => true,
                Delta::Finish(reason) => reason == "tool_calls",
                Delta::Snapshot(answer) => !answer.tools.is_empty(),
                _ => false,
            }
        {
            return Err("上游返回了此文本入口未发布的工具调用");
        }
        self.buffered = self.buffered.saturating_add(match &delta {
            Delta::Text(text) | Delta::Refusal(text) | Delta::Reasoning(text) => text.len(),
            Delta::Tool(_, id, name, args) => {
                id.as_ref().map_or(0, String::len)
                    + name.as_ref().map_or(0, String::len)
                    + args.len()
            }
            Delta::ToolDone(slot, args) => {
                if self
                    .tools
                    .get(slot)
                    .is_none_or(|pos| self.answer.tools[*pos]["function"]["arguments"] == "")
                {
                    args.len()
                } else {
                    0
                }
            }
            _ => 0,
        });
        if self.buffered > 16 * 1024 * 1024 {
            return Err("转换输出累计超过 16 MiB");
        }
        let prefix = if matches!(
            &delta,
            Delta::Text(_) | Delta::Refusal(_) | Delta::Tool(..) | Delta::End
        ) {
            self.close_reasoning()
        } else {
            String::new()
        };
        let out = match delta {
            Delta::Text(text) => self.text(&text),
            Delta::Refusal(text) => self.refusal(&text),
            Delta::Reasoning(text) => self.reasoning(&text)?,
            Delta::Tool(slot, id, name, args) => self.tool(slot, id, name, args)?,
            Delta::ToolDone(slot, args) => {
                let pos = *self.tools.get(&slot).ok_or("工具参数先于调用定义")?;
                let current = self.answer.tools[pos]["function"]["arguments"]
                    .as_str()
                    .unwrap_or("");
                if current.is_empty() {
                    self.tool(slot, None, None, args)?
                } else if current != args {
                    return Err("工具最终参数与流式参数不一致");
                } else {
                    String::new()
                }
            }
            Delta::Usage(i, o, c) => {
                if let Some(i) = i {
                    self.answer.input = Some(i)
                }
                if let Some(o) = o {
                    self.answer.output = Some(o)
                }
                if let Some(c) = c {
                    self.answer.cached = Some(c)
                }
                String::new()
            }
            Delta::Finish(reason) => {
                self.answer.finish = reason;
                String::new()
            }
            Delta::Snapshot(answer) => {
                if self.answer.text != answer.text
                    || self.answer.refusal != answer.refusal
                    || self.answer.tools.len() != answer.tools.len()
                {
                    return Err("Responses 最终结果与已输出增量不一致");
                };
                for (a, b) in self.answer.tools.iter().zip(&answer.tools) {
                    if a != b {
                        return Err("Responses 最终工具参数不一致");
                    }
                }
                self.answer = answer;
                String::new()
            }
            Delta::End => self.finish()?,
        };
        Ok(prefix + &out)
    }
    fn finish(&mut self) -> Result<String> {
        self.answer.validate()?;
        let result = self.answer.render(&self.protocol, &self.id, &self.model)?;
        let mut out = String::new();
        for (i, kind) in self.order.clone().iter().enumerate() {
            let i = i + usize::from(self.reasoning_started);
            if self.protocol == "messages" {
                out += &self.event("content_block_stop", json!({"index":i}));
            } else if let Block::Tool(pos) = kind {
                let t = &self.answer.tools[*pos];
                let item = self.registry.response_item(json!({"type":"function_call","id":format!("fc_{}_{}",self.id,pos),"call_id":t["id"],"name":t["function"]["name"],"arguments":t["function"]["arguments"],"status":"completed"}))?;
                if item["type"] == "custom_tool_call" {
                    out += &self.event(
                        "response.custom_tool_call_input.delta",
                        json!({"output_index":i,"item_id":item["id"],"delta":item["input"]}),
                    );
                    out += &self.event(
                        "response.custom_tool_call_input.done",
                        json!({"output_index":i,"item_id":item["id"],"input":item["input"]}),
                    );
                } else {
                    out += &self.event(
                    "response.function_call_arguments.done",
                    json!({"output_index":i,"item_id":item["id"],"arguments":item["arguments"]}),
                );
                }
                out += &self.event(
                    "response.output_item.done",
                    json!({"output_index":i,"item":item}),
                );
            } else if matches!(kind, Block::Refusal) {
                let item = json!({"type":"message","id":format!("refusal_{}",self.id),"role":"assistant","status":"completed","content":[{"type":"refusal","refusal":self.answer.refusal}]});
                out += &self.event("response.refusal.done",json!({"output_index":i,"item_id":item["id"],"content_index":0,"refusal":self.answer.refusal}));
                out += &self.event("response.content_part.done",json!({"output_index":i,"item_id":item["id"],"content_index":0,"part":item["content"][0]}));
                out += &self.event(
                    "response.output_item.done",
                    json!({"output_index":i,"item":item}),
                );
            } else {
                let item = json!({"type":"message","id":format!("msg_{}",self.id),"role":"assistant","status":"completed","content":[{"type":"output_text","text":self.answer.text,"annotations":[]}]});
                out+=&self.event("response.output_text.done",json!({"output_index":i,"item_id":item["id"],"content_index":0,"text":self.answer.text}));
                out+=&self.event("response.content_part.done",json!({"output_index":i,"item_id":item["id"],"content_index":0,"part":item["content"][0]}));
                out += &self.event(
                    "response.output_item.done",
                    json!({"output_index":i,"item":item}),
                );
            }
        }
        if self.protocol == "messages" {
            out+=&self.event("message_delta",json!({"delta":{"stop_reason":result["stop_reason"],"stop_sequence":null},"usage":result["usage"]}));
            out += &self.event("message_stop", json!({}));
        } else {
            let mut result = result;
            let output = result["output"].as_array().ok_or("响应输出无效")?.clone();
            result["output"] = json!(
                output
                    .iter()
                    .filter(|v| v["type"] == "reasoning")
                    .cloned()
                    .chain(self.order.iter().map(|k| {
                        match k {
                            Block::Text => output
                                .iter()
                                .find(|v| {
                                    v["type"] == "message"
                                        && v["content"][0]["type"] == "output_text"
                                })
                                .cloned()
                                .unwrap_or(Value::Null),
                            Block::Refusal => output
                                .iter()
                                .find(|v| {
                                    v["type"] == "message" && v["content"][0]["type"] == "refusal"
                                })
                                .cloned()
                                .unwrap_or(Value::Null),
                            Block::Tool(pos) => output
                                .iter()
                                .filter(|v| v["type"] == "function_call")
                                .nth(*pos)
                                .cloned()
                                .unwrap_or(Value::Null),
                        }
                    }))
                    .collect::<Vec<_>>()
            );
            let result = self.registry.restore("responses", result)?;
            out += &self.event(
                if self.answer.end_status() == "incomplete" {
                    "response.incomplete"
                } else {
                    "response.completed"
                },
                json!({"response":result}),
            );
        }
        self.finished = true;
        Ok(out)
    }
    pub fn is_finished(&self) -> bool {
        self.finished
    }
    pub fn end_status(&self) -> &'static str {
        self.answer.end_status()
    }
    pub fn error(&mut self) -> String {
        if self.protocol == "messages" {
            self.event("error",json!({"error":{"type":"api_error","message":"上游中断或返回尚不可转换的内容；本次输出不完整，请查看请求日志"}}))
        } else {
            self.event("response.failed",json!({"response":{"id":self.id,"object":"response","status":"failed","error":{"code":"protocol_conversion_failed","message":"上游中断或内容尚不可转换"}}}))
        }
    }
}
