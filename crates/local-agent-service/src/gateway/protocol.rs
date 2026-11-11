//! Protocol adaptation keeps text, image parts and standard function tool relationships.
//! Unsupported stateful or provider-hosted operations fail instead of disappearing.
use serde_json::{Value, json};

pub type Result<T> = std::result::Result<T, &'static str>;

pub(crate) fn reply_error(value: &Value) -> Option<&Value> {
    let envelope = if value["type"] == "response.failed" {
        &value["response"]
    } else {
        value
    };
    envelope
        .get("error")
        .filter(|error| !error.is_null())
        .or_else(|| {
            let [choice] = value["choices"].as_array()?.as_slice() else {
                return None;
            };
            (choice["finish_reason"] == "error")
                .then(|| choice.get("error").filter(|error| !error.is_null()))
                .flatten()
        })
        .or_else(|| (value["type"] == "error").then_some(value))
}

fn list(v: &Value) -> Result<&Vec<Value>> {
    v.as_array().ok_or("协议字段需要数组")
}
fn string(v: &Value) -> Result<&str> {
    v.as_str().ok_or("协议字段需要字符串")
}
fn plain(v: &Value) -> Result<String> {
    if let Some(s) = v.as_str() {
        return Ok(s.into());
    }
    let mut text = String::new();
    for part in list(v)? {
        if matches!(
            part["type"].as_str(),
            Some("text" | "input_text" | "output_text")
        ) {
            text.push_str(string(&part["text"])?)
        } else {
            return Err("此工具结果包含尚不可跨协议转换的内容，请使用同协议连接");
        }
    }
    Ok(text)
}
fn content(v: &Value, source: &str, target: &str) -> Result<Value> {
    if v.is_string() || v.is_null() {
        return Ok(v.clone());
    }
    let mut out = vec![];
    for p in list(v)? {
        let kind = p["type"].as_str().unwrap_or("");
        if matches!(kind, "text" | "input_text" | "output_text" | "refusal") {
            let text = if kind == "refusal" {
                string(&p["refusal"])?
            } else {
                string(&p["text"])?
            };
            out.push(json!({"type":if target=="responses"{"input_text"}else{"text"},"text":text}));
        } else if matches!(kind, "image" | "input_image" | "image_url") {
            let url = if source == "messages" {
                match p["source"]["type"].as_str() {
                    Some("url") => string(&p["source"]["url"])?.to_string(),
                    Some("base64") => format!(
                        "data:{};base64,{}",
                        string(&p["source"]["media_type"])?,
                        string(&p["source"]["data"])?
                    ),
                    _ => return Err("图片来源尚不可转换"),
                }
            } else if source == "responses" {
                string(&p["image_url"])?.to_string()
            } else {
                string(&p["image_url"]["url"])?.to_string()
            };
            out.push(if target=="messages" {
                if let Some(data)=url.strip_prefix("data:"){let (media,data)=data.split_once(";base64,").ok_or("图片 data URL 无效")?;json!({"type":"image","source":{"type":"base64","media_type":media,"data":data}})}else{json!({"type":"image","source":{"type":"url","url":url}})}
            }else if target=="responses"{json!({"type":"input_image","image_url":url})}else{json!({"type":"image_url","image_url":{"url":url}})});
        } else {
            return Err("当前跨协议路由只支持文本、图片和标准函数工具；此内容类型请使用同协议连接");
        }
    }
    Ok(Value::Array(out))
}
fn call(id: &Value, name: &Value, args: Value) -> Value {
    json!({"id":id,"type":"function","function":{"name":name,"arguments":args}})
}

pub fn provider_request(
    source: &str,
    provider: &local_agent_core::Provider,
    v: &Value,
) -> Result<Value> {
    if (provider.vendor == "ollama" || provider.id == "ollama")
        && provider.protocol == "chat-completions"
        && v["thinking"]["type"] == "disabled"
    {
        // Ollama's OpenAI-compatible endpoint uses reasoning_effort=none to
        // disable thinking; dropping the Messages flag leaves its default on.
        let mut out = request(source, &provider.protocol, v, &provider.model)?;
        out.as_object_mut().unwrap().remove("reasoning");
        out["reasoning_effort"] = json!("none");
        return Ok(out);
    }
    let Some(scope) = super::deepseek::scope(provider).filter(|_| source != provider.protocol)
    else {
        return request(source, &provider.protocol, v, &provider.model);
    };
    super::capabilities::builtin().resolve(source)?;
    super::capabilities::builtin().resolve(&provider.protocol)?;
    let mut out =
        request_with_reasoning(source, &provider.protocol, v, &provider.model, Some(&scope))?;
    let enabled = matches!(v["thinking"]["type"].as_str(), Some("enabled" | "adaptive"))
        || v["reasoning"]["effort"]
            .as_str()
            .is_some_and(|e| e != "none");
    out["thinking"] = json!({"type":if enabled {"enabled"} else {"disabled"}});
    if !enabled {
        out.as_object_mut().unwrap().remove("reasoning_effort");
    }
    super::deepseek::prepare(&mut out, enabled)?;
    Ok(out)
}

pub(crate) fn use_intrinsic_reasoning(protocol: &str, body: &mut Value) {
    let Some(body) = body.as_object_mut() else {
        return;
    };
    body.remove("thinking");
    match protocol {
        "messages" => {
            body.insert("output_config".into(), json!({"effort":"low"}));
        }
        "chat-completions" => {
            body.insert("reasoning_effort".into(), json!("low"));
            body.insert("reasoning_split".into(), json!(true));
        }
        _ => {
            body.insert("reasoning".into(), json!({"effort":"low"}));
        }
    }
}

pub(crate) fn remove_intrinsic_reasoning(protocol: &str, body: &mut Value) {
    match protocol {
        "messages" => {
            if let Some(content) = body["content"].as_array_mut() {
                content.retain(|block| {
                    !matches!(
                        block["type"].as_str(),
                        Some("thinking" | "redacted_thinking")
                    )
                });
            }
        }
        "chat-completions" => {
            if let Some(message) = body
                .pointer_mut("/choices/0/message")
                .and_then(Value::as_object_mut)
            {
                message.remove("reasoning_content");
            }
        }
        _ => {
            if let Some(output) = body["output"].as_array_mut() {
                output.retain(|item| item["type"] != "reasoning");
            }
        }
    }
}

pub fn request(source: &str, target: &str, v: &Value, model: &str) -> Result<Value> {
    super::capabilities::builtin().resolve(source)?;
    super::capabilities::builtin()
        .resolve(target)?
        .request(source, v, model)
}

pub(super) fn request_builtin(source: &str, target: &str, v: &Value, model: &str) -> Result<Value> {
    request_with_reasoning(source, target, v, model, None)
}

fn request_with_reasoning(
    source: &str,
    target: &str,
    v: &Value,
    model: &str,
    scope: Option<&str>,
) -> Result<Value> {
    if source == target {
        let mut v = v.clone();
        v["model"] = json!(model);
        return Ok(v);
    }
    if v["background"] == true
        || v.get("previous_response_id").is_some_and(|x| !x.is_null())
        || v.get("conversation").is_some_and(|x| !x.is_null())
    {
        return Err("跨协议路由需要完整对话输入，不支持后台响应或服务端会话引用");
    }
    let mut messages = vec![];
    let mut tools = vec![];
    let mut reasoning = None;
    if source == "messages" {
        if let Some(system) = v.get("system") {
            messages
                .push(json!({"role":"system","content":content(system,source,"chat-completions")?}))
        }
        for m in list(&v["messages"])? {
            if m["content"].is_string() {
                messages.push(m.clone());
                continue;
            }
            let mut body = vec![];
            let mut calls = vec![];
            let mut thought: Option<String> = None;
            let flush = |body: &mut Vec<Value>,
                         calls: &mut Vec<Value>,
                         thought: &mut Option<String>,
                         messages: &mut Vec<Value>| {
                if !body.is_empty() || !calls.is_empty() || thought.is_some() {
                    let mut message = json!({"role":m["role"],"content":std::mem::take(body)});
                    if let Some(text) = thought.take() {
                        message["reasoning_content"] = json!(text);
                    }
                    if !calls.is_empty() {
                        message["tool_calls"] = json!(std::mem::take(calls));
                    }
                    messages.push(message);
                }
            };
            for part in list(&m["content"])? {
                match part["type"].as_str() {
                    Some("tool_use") => calls.push(call(
                        &part["id"],
                        &part["name"],
                        json!(part["input"].to_string()),
                    )),
                    Some("tool_result") => {
                        flush(&mut body, &mut calls, &mut thought, &mut messages);
                        let mut result = plain(&part["content"])?;
                        if part["is_error"] == true {
                            result = format!("[tool error] {result}");
                        }
                        messages.push(json!({"role":"tool","tool_call_id":part["tool_use_id"],"content":result}));
                    }
                    Some("thinking") if m["role"] == "assistant" => {
                        thought
                            .get_or_insert_default()
                            .push_str(&super::deepseek::thinking(part, scope)?);
                    }
                    Some("thinking" | "redacted_thinking") => {
                        return Err(
                            "跨协议暂不重放 Anthropic 思考签名；请开启新会话或使用同协议连接",
                        );
                    }
                    _ => body.extend(
                        list(&content(&json!([part]), source, "chat-completions")?)?.clone(),
                    ),
                }
            }
            flush(&mut body, &mut calls, &mut thought, &mut messages);
        }
        for tool in v["tools"].as_array().into_iter().flatten() {
            if tool
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|t| t != "custom")
            {
                return Err("服务商原生托管工具不能转换成标准函数");
            }
            tools.push(json!({"type":"function","function":{"name":tool["name"],"description":tool["description"],"parameters":tool["input_schema"]}}));
        }
        if v["thinking"]["type"] == "enabled" || v["thinking"]["type"] == "adaptive" {
            reasoning = Some(
                v["output_config"]["effort"]
                    .as_str()
                    .unwrap_or("medium")
                    .to_string(),
            );
        }
    } else if source == "responses" {
        if let Some(instructions) = v.get("instructions").filter(|x| !x.is_null()) {
            messages.push(json!({"role":"system","content":instructions}));
        }
        if v["input"].is_string() {
            messages.push(json!({"role":"user","content":v["input"]}));
        } else {
            let mut thought: Option<String> = None;
            for item in list(&v["input"])? {
                match item["type"].as_str().unwrap_or("message"){
                    // Chat Completions-compatible providers may not implement OpenAI's
                    // developer role. Preserve its instruction priority as system content.
                    "message"=> {
                        let mut message = json!({"role":if item["role"]=="developer" {json!("system")} else {item["role"].clone()},"content":content(&item["content"],source,"chat-completions")?});
                        if let Some(text) = thought.take() {
                            if message["role"] != "assistant" { return Err("思考内容后缺少助手响应"); }
                            message["reasoning_content"] = json!(text);
                        }
                        messages.push(message);
                    },
                    "function_call"=> {
                        let call=call(&item["call_id"],&item["name"],item["arguments"].clone());
                        // Responses stores parallel calls as separate items. Chat Completions
                        // requires all calls before their results in one assistant message.
                        if let Some(previous)=messages.last_mut().filter(|m|m["role"]=="assistant" && thought.is_none() && (m["tool_calls"].is_array() || scope.is_some())) {
                            if !previous["tool_calls"].is_array() { previous["tool_calls"] = json!([]); }
                            previous["tool_calls"].as_array_mut().unwrap().push(call);
                        } else {
                            let mut message = json!({"role":"assistant","content":null,"tool_calls":[call]});
                            if let Some(text) = thought.take() { message["reasoning_content"] = json!(text); }
                            messages.push(message);
                        }
                    },
                    "function_call_output"=>messages.push(json!({"role":"tool","tool_call_id":item["call_id"],"content":plain(&item["output"])?})),
                    "reasoning"=>{if item.get("encrypted_content").is_some_and(|x|!x.is_null()) || item["summary"].as_array().is_some_and(|a|!a.is_empty()){thought.get_or_insert_default().push_str(&super::deepseek::reasoning(item, scope)?);}},
                    _=>return Err("此 Responses 输入类型尚不可跨协议转换，请使用同协议连接"),
                }
            }
            if thought.is_some() {
                return Err("思考内容后缺少助手响应");
            }
        }
        for tool in v["tools"].as_array().into_iter().flatten() {
            if tool["type"] != "function" {
                return Err(
                    "跨协议仅支持标准 function 工具，原生工具 / 自由格式工具请使用同协议连接",
                );
            }
            tools.push(json!({"type":"function","function":{"name":tool["name"],"description":tool["description"],"parameters":tool["parameters"],"strict":tool["strict"]}}));
        }
        reasoning = v["reasoning"]["effort"].as_str().map(str::to_owned);
    } else {
        return Err("不支持的 Agent 输入协议");
    }
    let mut out = json!({"model":model,"stream":v["stream"]==true});
    for m in &messages {
        for t in m["tool_calls"].as_array().into_iter().flatten() {
            string(&t["id"])?;
            string(&t["function"]["name"])?;
            let args: Value = serde_json::from_str(string(&t["function"]["arguments"])?)
                .map_err(|_| "工具参数不是完整 JSON")?;
            if !args.is_object() {
                return Err("工具参数需要 JSON 对象");
            }
        }
    }
    let max = v.get("max_tokens").or_else(|| v.get("max_output_tokens"));
    for key in ["temperature", "top_p"] {
        if let Some(value) = v.get(key) {
            out[key] = value.clone();
        }
    }
    let choice = v.get("tool_choice").filter(|x| !x.is_null());
    if target == "chat-completions" {
        out["messages"] = json!(messages);
        out["store"] = json!(false);
        if let Some(max) = max {
            out["max_tokens"] = max.clone();
        }
        if !tools.is_empty() {
            out["tools"] = json!(tools);
        }
        if let Some(choice) = choice {
            out["tool_choice"] = if source == "messages" {
                match choice["type"].as_str() {
                    Some("auto") => json!("auto"),
                    Some("any") => json!("required"),
                    Some("none") => json!("none"),
                    Some("tool") => json!({"type":"function","function":{"name":choice["name"]}}),
                    _ => return Err("工具选择方式不可转换"),
                }
            } else if choice["type"] == "function" {
                json!({"type":"function","function":{"name":choice["name"]}})
            } else if choice.is_string() {
                choice.clone()
            } else {
                return Err("此工具选择方式尚不可转换");
            };
        }
        if let Some(effort) = reasoning {
            out["reasoning_effort"] = json!(effort);
        }
        if out["stream"] == true {
            out["stream_options"] = json!({"include_usage":true});
        }
    } else if target == "responses" {
        let mut input = vec![];
        for m in messages {
            if m["role"] == "tool" {
                input.push(json!({"type":"function_call_output","call_id":m["tool_call_id"],"output":m["content"]}));
                continue;
            }
            if !m["content"].is_null() {
                let mut parts = content(&m["content"], "chat-completions", "responses")?;
                if m["role"] == "assistant" {
                    for p in parts.as_array_mut().into_iter().flatten() {
                        if p["type"] == "input_text" {
                            p["type"] = json!("output_text");
                        }
                    }
                }
                input.push(json!({"role":m["role"],"content":parts}));
            }
            for t in m["tool_calls"].as_array().into_iter().flatten() {
                input.push(json!({"type":"function_call","call_id":t["id"],"name":t["function"]["name"],"arguments":t["function"]["arguments"]}));
            }
        }
        out["input"] = json!(input);
        out["store"] = json!(false);
        if let Some(max) = max {
            out["max_output_tokens"] = max.clone();
        }
        if !tools.is_empty() {
            out["tools"] = json!(
                tools
                    .into_iter()
                    .map(|t| {
                        let mut t = t["function"].clone();
                        t["type"] = json!("function");
                        t
                    })
                    .collect::<Vec<_>>()
            );
        }
        if let Some(choice) = choice {
            out["tool_choice"] = match choice["type"].as_str() {
                Some("auto") => json!("auto"),
                Some("any") => json!("required"),
                Some("none") => json!("none"),
                Some("tool") => json!({"type":"function","name":choice["name"]}),
                _ => return Err("工具选择方式不可转换"),
            };
        }
        if let Some(effort) = reasoning {
            out["reasoning"] = json!({"effort":effort});
        }
    } else if target == "messages" {
        let mut system = vec![];
        let mut converted = vec![];
        for m in messages {
            if m["role"] == "system" || m["role"] == "developer" {
                system.push(plain(&m["content"])?);
                continue;
            }
            let (role, mut body) = if m["role"] == "tool" {
                (
                    "user",
                    vec![
                        json!({"type":"tool_result","tool_use_id":m["tool_call_id"],"content":m["content"]}),
                    ],
                )
            } else {
                let body = if m["content"].is_null() {
                    vec![]
                } else {
                    let c = content(&m["content"], "chat-completions", "messages")?;
                    if c.is_string() {
                        vec![json!({"type":"text","text":c})]
                    } else {
                        list(&c)?.clone()
                    }
                };
                (m["role"].as_str().unwrap_or("user"), body)
            };
            for t in m["tool_calls"].as_array().into_iter().flatten() {
                let args: Value = serde_json::from_str(string(&t["function"]["arguments"])?)
                    .map_err(|_| "工具参数不是完整 JSON")?;
                body.push(json!({"type":"tool_use","id":t["id"],"name":t["function"]["name"],"input":args}));
            }
            converted.push(json!({"role":role,"content":body}));
        }
        if !system.is_empty() {
            out["system"] = json!(system.join("\n\n"));
        }
        out["messages"] = json!(converted);
        out["max_tokens"] = max.cloned().unwrap_or(json!(8192));
        if !tools.is_empty() {
            out["tools"]=json!(tools.into_iter().map(|t|json!({"name":t["function"]["name"],"description":t["function"]["description"],"input_schema":t["function"]["parameters"]})).collect::<Vec<_>>());
        }
        if let Some(choice) = choice {
            out["tool_choice"] = if choice.is_string() {
                json!({"type":match choice.as_str(){Some("auto")=>"auto",Some("required")=>"any",Some("none")=>"none",_=>return Err("工具选择方式不可转换")}})
            } else if choice["type"] == "function" {
                json!({"type":"tool","name":choice["name"]})
            } else {
                return Err("工具选择方式不可转换");
            };
        }
        if let Some(effort) = reasoning.filter(|s| s != "none" && s != "minimal") {
            out["thinking"] = json!({"type":"adaptive"});
            out["output_config"] = json!({"effort":effort});
        } else if source == "responses" && v["thinking"]["type"] == "disabled" {
            out["thinking"] = json!({"type":"disabled"});
        }
    } else {
        return Err("上游协议不受支持");
    }
    if target != "messages" {
        if let Some(parallel) = v.get("parallel_tool_calls").and_then(Value::as_bool) {
            out["parallel_tool_calls"] = json!(parallel);
        }
        if let Some(disable) = v["tool_choice"]["disable_parallel_tool_use"].as_bool() {
            out["parallel_tool_calls"] = json!(!disable);
        }
    } else if let Some(parallel) = v.get("parallel_tool_calls").and_then(Value::as_bool) {
        if out["tool_choice"].is_null() {
            out["tool_choice"] = json!({"type":"auto"});
        }
        out["tool_choice"]["disable_parallel_tool_use"] = json!(!parallel);
    }
    if let Some(stop) = v
        .get("stop_sequences")
        .filter(|v| v.as_array().is_some_and(|v| !v.is_empty()))
    {
        if target == "chat-completions" {
            out["stop"] = stop.clone();
        } else {
            return Err("该停止序列不能映射到 Responses，请使用同协议连接");
        }
    }
    for t in out
        .get_mut("tools")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        let t = if target == "chat-completions" {
            &mut t["function"]
        } else {
            t
        };
        if let Some(object) = t.as_object_mut() {
            for key in ["description", "strict"] {
                if object.get(key).is_some_and(Value::is_null) {
                    object.remove(key);
                }
            }
            let key = if target == "messages" {
                "input_schema"
            } else {
                "parameters"
            };
            if object.get(key).is_none_or(Value::is_null) {
                object.insert(key.into(), json!({"type":"object","properties":{}}));
            }
        }
    }
    Ok(out)
}

#[derive(Default, Clone)]
pub struct Answer {
    pub text: String,
    pub refusal: String,
    pub reasoning: String,
    pub reasoning_reported: bool,
    pub reasoning_scope: Option<String>,
    pub tools: Vec<Value>,
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cached: Option<u64>,
    pub finish: String,
}
impl Answer {
    pub fn parse_provider(provider: &local_agent_core::Provider, v: &Value) -> Result<Self> {
        let Some(scope) = super::deepseek::scope(provider) else {
            return Self::parse(&provider.protocol, v);
        };
        let mut body = v.clone();
        let reasoning = body
            .pointer_mut("/choices/0/message")
            .and_then(Value::as_object_mut)
            .and_then(|m| m.remove("reasoning_content"))
            .unwrap_or(Value::Null);
        if !reasoning.is_null() && !reasoning.is_string() {
            return Err("上游思考内容需要字符串");
        }
        let mut answer = Self::parse(&provider.protocol, &body)?;
        answer.reasoning_reported = reasoning.is_string();
        answer.reasoning = reasoning.as_str().unwrap_or("").into();
        answer.reasoning_scope = Some(scope);
        Ok(answer)
    }
    pub fn parse(protocol: &str, v: &Value) -> Result<Self> {
        super::capabilities::builtin().resolve(protocol)?.answer(v)
    }
    pub fn render(&self, protocol: &str, id: &str, model: &str) -> Result<Value> {
        super::capabilities::builtin()
            .resolve(protocol)?
            .render(self, id, model)
    }

    pub(super) fn parse_builtin(protocol: &str, v: &Value) -> Result<Self> {
        if reply_error(v).is_some() {
            return Err("上游返回协议错误");
        }
        let mut a = Self::default();
        let usage = &v["usage"];
        if protocol == "chat-completions" {
            if v["choices"].as_array().is_none_or(|c| c.len() != 1) {
                return Err("上游未返回单个 Chat Completions 结果");
            }
            let c = &v["choices"][0];
            if c["message"]
                .get("reasoning_content")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty())
            {
                return Err("当前转换不重放模型思考内容，请使用同协议连接或关闭上游深度思考");
            }
            a.text = c["message"]["content"].as_str().unwrap_or("").into();
            if let Some(refusal) = c["message"].get("refusal").filter(|v| !v.is_null()) {
                a.refusal = string(refusal)?.into();
            }
            a.tools = c["message"]["tool_calls"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            a.finish = c["finish_reason"].as_str().unwrap_or("").into();
            a.input = usage["prompt_tokens"].as_u64();
            a.output = usage["completion_tokens"].as_u64();
            a.cached = usage["prompt_cache_hit_tokens"]
                .as_u64()
                .or_else(|| usage["prompt_tokens_details"]["cached_tokens"].as_u64());
        } else if protocol == "messages" {
            if v["type"] != "message" {
                return Err("上游没有返回 Messages 结果");
            }
            for p in list(&v["content"])? {
                match p["type"].as_str() {
                    Some("text") => a.text.push_str(string(&p["text"])?),
                    Some("tool_use") => {
                        a.tools
                            .push(call(&p["id"], &p["name"], json!(p["input"].to_string())))
                    }
                    _ => return Err("上游返回当前不可转换的非文本 / 函数内容"),
                }
            }
            a.finish = match v["stop_reason"].as_str() {
                Some("end_turn" | "stop_sequence") => "stop",
                Some("tool_use") => "tool_calls",
                Some("max_tokens") => "length",
                Some("refusal") => "stop",
                _ => return Err("上游未返回可转换的结束状态"),
            }
            .into();
            a.cached = usage["cache_read_input_tokens"].as_u64();
            a.input = usage["input_tokens"].as_u64().map(|n| {
                n.saturating_add(a.cached.unwrap_or(0))
                    .saturating_add(usage["cache_creation_input_tokens"].as_u64().unwrap_or(0))
            });
            a.output = usage["output_tokens"].as_u64();
        } else {
            if v["object"] != "response"
                || !matches!(v["status"].as_str(), Some("completed" | "incomplete"))
            {
                return Err("上游没有返回完成的 Responses 结果");
            }
            for p in list(&v["output"])? {
                match p["type"].as_str() {
                    Some("message") => {
                        for part in list(&p["content"])? {
                            match part["type"].as_str() {
                                Some("output_text") => a.text.push_str(string(&part["text"])?),
                                Some("refusal") => a.refusal.push_str(string(&part["refusal"])?),
                                _ => return Err("上游响应消息类型尚不可转换"),
                            }
                        }
                    }
                    Some("function_call") => {
                        a.tools
                            .push(call(&p["call_id"], &p["name"], p["arguments"].clone()))
                    }
                    Some("reasoning")
                        if p["summary"].as_array().is_none_or(Vec::is_empty)
                            && p.get("encrypted_content").is_none_or(Value::is_null) => {}
                    _ => return Err("上游输出类型尚不可跨协议转换"),
                }
            }
            a.finish = if v["status"] == "incomplete" {
                "length"
            } else if a.tools.is_empty() {
                "stop"
            } else {
                "tool_calls"
            }
            .into();
            a.input = usage["input_tokens"].as_u64();
            a.output = usage["output_tokens"].as_u64();
            a.cached = usage["input_tokens_details"]["cached_tokens"].as_u64();
        }
        a.validate()?;
        Ok(a)
    }
    pub fn validate(&self) -> Result<()> {
        if !["stop", "length", "tool_calls", "content_filter"].contains(&self.finish.as_str()) {
            return Err("上游没有完整的结束状态");
        }
        if !self.refusal.is_empty() && !self.tools.is_empty() {
            return Err("上游同时返回拒绝内容和工具调用，无法安全转换");
        }
        for t in &self.tools {
            string(&t["id"])?;
            string(&t["function"]["name"])?;
            let args: Value = serde_json::from_str(string(&t["function"]["arguments"])?)
                .map_err(|_| "上游工具参数不是完整 JSON")?;
            if !args.is_object() {
                return Err("工具参数需要 JSON 对象");
            }
        }
        Ok(())
    }
    pub fn end_status(&self) -> &'static str {
        if matches!(self.finish.as_str(), "length" | "content_filter") {
            "incomplete"
        } else {
            "success"
        }
    }
    pub(super) fn render_builtin(&self, protocol: &str, id: &str, model: &str) -> Result<Value> {
        if protocol == "messages" {
            if self.input.is_none() || self.output.is_none() {
                return Err("上游未报告完整 Token 用量，无法构造完整 Messages 结果");
            }
            let mut body = vec![];
            if !self.reasoning.is_empty() || self.reasoning_reported {
                let scope = self.reasoning_scope.as_deref().ok_or("思考内容缺少来源")?;
                body.push(json!({"type":"thinking","thinking":self.reasoning,"signature":super::deepseek::marker(scope)}));
            }
            if !self.text.is_empty() {
                body.push(json!({"type":"text","text":self.text}));
            }
            if !self.refusal.is_empty() {
                body.push(json!({"type":"text","text":self.refusal}));
            }
            for t in &self.tools {
                body.push(json!({"type":"tool_use","id":t["id"],"name":t["function"]["name"],"input":serde_json::from_str::<Value>(string(&t["function"]["arguments"]) ?).map_err(|_|"工具参数无效")?}));
            }
            Ok(
                json!({"id":id,"type":"message","role":"assistant","model":model,"content":body,"stop_reason":if self.finish=="length"{"max_tokens"}else if !self.refusal.is_empty() || self.finish=="content_filter"{"refusal"}else if self.tools.is_empty(){"end_turn"}else{"tool_use"},"stop_sequence":null,"usage":{"input_tokens":self.input.map(|n|n.saturating_sub(self.cached.unwrap_or(0))),"output_tokens":self.output,"cache_read_input_tokens":self.cached}}),
            )
        } else {
            let mut output = vec![];
            if !self.reasoning.is_empty() || self.reasoning_reported {
                let scope = self.reasoning_scope.as_deref().ok_or("思考内容缺少来源")?;
                output.push(super::deepseek::item(scope, id, &self.reasoning));
            }
            if !self.text.is_empty() {
                output.push(json!({"type":"message","id":format!("msg_{id}"),"status":"completed","role":"assistant","content":[{"type":"output_text","text":self.text,"annotations":[]}]}));
            }
            if !self.refusal.is_empty() {
                output.push(json!({"type":"message","id":format!("refusal_{id}"),"status":"completed","role":"assistant","content":[{"type":"refusal","refusal":self.refusal}]}));
            }
            for (i, t) in self.tools.iter().enumerate() {
                output.push(json!({"type":"function_call","id":format!("fc_{id}_{i}"),"call_id":t["id"],"name":t["function"]["name"],"arguments":t["function"]["arguments"],"status":"completed"}));
            }
            let mut response = json!({"id":id,"object":"response","created_at":chrono::Utc::now().timestamp(),"model":model,"status":if self.end_status()=="incomplete"{"incomplete"}else{"completed"},"error":null,"output":output,"parallel_tool_calls":true,"incomplete_details":if self.finish=="length"{json!({"reason":"max_output_tokens"})}else if self.finish=="content_filter"{json!({"reason":"content_filter"})}else{Value::Null},"usage":{"input_tokens":self.input,"output_tokens":self.output,"total_tokens":self.input.zip(self.output).map(|(a,b)|a.saturating_add(b)),"input_tokens_details":{"cached_tokens":self.cached}}});
            if self.input.is_none() || self.output.is_none() {
                response["usage"] = Value::Null;
            }
            Ok(response)
        }
    }
}
