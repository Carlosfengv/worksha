//! Public DeepSeek reasoning is replayable only to the connection that produced it.
//! These markers identify local protocol translations, not Anthropic signatures.
use local_agent_core::{Provider, providers::files};
use serde_json::{Value, json};

use super::protocol::Result;

pub fn scope(provider: &Provider) -> Option<String> {
    (provider.vendor == "deepseek" && provider.protocol == "chat-completions").then(|| {
        files::hash(
            json!([
                provider.id,
                provider.endpoint.trim_end_matches('/'),
                provider.model
            ])
            .to_string()
            .as_bytes(),
        )
    })
}

pub fn marker(scope: &str) -> String {
    format!("local_agent_deepseek_v1_{scope}")
}

/// DeepSeek accepts array content for users, but system/assistant text must be strings.
/// Normalize only equivalent text; never drop unsupported content or invent reasoning.
pub fn prepare(out: &mut Value, thinking: bool) -> Result<()> {
    out.as_object_mut().unwrap().remove("store");
    if let Some(effort) = out.get_mut("reasoning_effort") {
        *effort = json!(match effort.as_str() {
            Some("none") => "none",
            Some("minimal" | "low") => "low",
            Some("medium" | "high" | "xhigh") => "high",
            Some("max") => "max",
            _ => return Err("DeepSeek 不支持此思考强度，请选择 low、high 或 max"),
        });
    }
    if thinking
        && out
            .get("tool_choice")
            .is_some_and(|v| v == "required" || v.is_object())
    {
        return Err("DeepSeek 思考模式不支持强制工具选择，请关闭思考或使用自动工具选择");
    }
    let tools = out["tools"].as_array().is_some_and(|v| !v.is_empty());
    for message in out["messages"].as_array_mut().ok_or("对话内容需要数组")? {
        if matches!(message["role"].as_str(), Some("system" | "assistant"))
            && message["content"].is_array()
        {
            let mut text = String::new();
            for part in message["content"].as_array().unwrap() {
                if part["type"] != "text" {
                    return Err("DeepSeek 的系统与助手历史只支持文本内容");
                }
                text.push_str(part["text"].as_str().ok_or("历史文本需要字符串")?);
            }
            message["content"] = json!(text);
        }
        if thinking
            && tools
            && message["role"] == "assistant"
            && !message["reasoning_content"].is_string()
        {
            return Err(
                "DeepSeek 思考工具续调缺少原始思考内容，请开启新会话；不会补猜或重试上游请求",
            );
        }
    }
    Ok(())
}

pub fn thinking(part: &Value, scope: Option<&str>) -> Result<String> {
    let scope = scope.ok_or("跨协议暂不重放 Anthropic 思考签名；请开启新会话或使用同协议连接")?;
    if part["signature"] != marker(scope) {
        return Err("思考内容不属于当前 DeepSeek 连接；请开启新会话或使用原连接");
    }
    part["thinking"]
        .as_str()
        .map(str::to_owned)
        .ok_or("思考内容需要字符串")
}

pub fn reasoning(item: &Value, scope: Option<&str>) -> Result<String> {
    let scope = scope.ok_or("跨协议不能重放原服务商的思考内容；请使用新会话或同协议连接")?;
    if !item["id"]
        .as_str()
        .is_some_and(|id| id.starts_with(&format!("rs_{}_", marker(scope))))
        || item.get("encrypted_content").is_some_and(|v| !v.is_null())
    {
        return Err("思考内容不属于当前 DeepSeek 连接；请开启新会话或使用原连接");
    }
    let mut text = String::new();
    for part in item["summary"].as_array().ok_or("思考内容需要数组")? {
        if part["type"] != "summary_text" {
            return Err("思考内容类型无效");
        }
        text.push_str(part["text"].as_str().ok_or("思考内容需要字符串")?);
    }
    Ok(text)
}

pub fn item(scope: &str, id: &str, text: &str) -> Value {
    json!({"type":"reasoning","id":format!("rs_{}_{id}", marker(scope)),"summary":[{"type":"summary_text","text":text}]})
}
