//! Text tokenization and protocol framing estimates; transport byte limits are separate.
use local_agent_core::{
    Provider,
    model_services::{Capabilities, ContextBudget},
};
use serde_json::{Value, json};
use std::sync::OnceLock;
use tokenizers::Tokenizer;

fn tokenizer(flash: bool) -> Result<&'static Tokenizer, &'static str> {
    static PRO: OnceLock<Result<Tokenizer, String>> = OnceLock::new();
    static FLASH: OnceLock<Result<Tokenizer, String>> = OnceLock::new();
    let slot = if flash { &FLASH } else { &PRO };
    slot.get_or_init(|| {
        use std::io::Read;
        let bytes = include_bytes!("../../assets/tokenizers/tokenizers.zip");
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes.as_slice()))
            .map_err(|e| e.to_string())?;
        let mut file = archive
            .by_name(if flash {
                "deepseek-v4-flash.json"
            } else {
                "deepseek-v4-pro.json"
            })
            .map_err(|e| e.to_string())?;
        let mut raw = Vec::new();
        file.by_ref()
            .take(8 * 1024 * 1024)
            .read_to_end(&mut raw)
            .map_err(|e| e.to_string())?;
        Tokenizer::from_bytes(&raw).map_err(|e| e.to_string())
    })
    .as_ref()
    .map_err(|_| "入口分词数据无法读取，未向上游发送请求")
}
pub(super) fn inspect(
    body: &Value,
    caps: &Capabilities,
    provider: Option<&Provider>,
) -> Result<ContextBudget, &'static str> {
    let input = json!({"system":body.get("system"),"messages":body.get("messages"),"tools":body.get("tools"),"tool_choice":body.get("tool_choice")});
    let text = serde_json::to_string(&input).map_err(|_| "请求输入无法计数")?;
    let deepseek = provider.is_some_and(|p| {
        url::Url::parse(&p.endpoint)
            .ok()
            .is_some_and(|u| u.host_str() == Some("api.deepseek.com"))
            && (p.model.starts_with("deepseek-v4-pro")
                || p.model.starts_with("deepseek-flash")
                || p.model.starts_with("deepseek-v4-flash"))
    });
    let count = if deepseek {
        tokenizer(provider.is_some_and(|p| !p.model.contains("v4-pro")))?
            .encode(text.as_str(), false)
            .map_err(|_| "请求输入无法分词")?
            .len() as u64
    } else {
        // Portable estimate only, never presented as an exact model token count.
        let ascii = text.chars().filter(char::is_ascii).count() as u64;
        let other = text.chars().filter(|c| !c.is_ascii()).count() as u64;
        ascii.div_ceil(3) + other * 2
    };
    let messages = body["messages"].as_array().map_or(0, Vec::len) as u64;
    Ok(ContextBudget {
        input_tokens: count + count.div_ceil(10) + 256 + messages * 16,
        output_tokens: body["max_tokens"].as_u64().unwrap_or(0),
        context_window: caps.context_window,
        method: if deepseek {
            "deepseek-v4-tokenizer-with-framing-margin"
        } else {
            "portable-text-estimate-with-framing-margin"
        }
        .into(),
        estimated: true,
    })
}
pub(super) async fn evaluate(
    body: Value,
    caps: Capabilities,
    provider: Provider,
) -> Result<ContextBudget, &'static str> {
    static CAPACITY: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);
    let permit = CAPACITY.acquire().await.map_err(|_| "上下文计数暂不可用")?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        inspect(&body, &caps, Some(&provider))
    })
    .await
    .map_err(|_| "上下文计数未完成")?
}
pub(super) fn is_context_error(bytes: &[u8]) -> bool {
    let v: Value = serde_json::from_slice(bytes).unwrap_or_default();
    let code = v["error"]["code"].as_str().unwrap_or("");
    let message = v["error"]["message"]
        .as_str()
        .unwrap_or("")
        .to_ascii_lowercase();
    code == "context_length_exceeded"
        || message.contains("maximum context length")
        || message.contains("context window")
        || message.contains("prompt is too long")
}
pub(super) fn upstream_rejection() -> axum::response::Response {
    use axum::response::IntoResponse;
    (axum::http::StatusCode::BAD_REQUEST,axum::Json(json!({"type":"error","error":{"type":"invalid_request_error","code":"context_length_exceeded","message":"Input exceeds the context window of this model. 上游拒绝上下文长度，请压缩历史或开启新会话。"}}))).into_response()
}
pub(super) fn rejection(budget: &ContextBudget) -> axum::response::Response {
    use axum::response::IntoResponse;
    (axum::http::StatusCode::BAD_REQUEST,axum::Json(json!({"type":"error","error":{"type":"invalid_request_error","code":"context_length_exceeded","message":"Input exceeds the context window of this model. 输入与输出预留超过入口上下文，请压缩历史或开启新会话。","budget":budget}}))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn large_utf8_json_is_not_equivalent_to_tokens_and_output_is_reserved() {
        let body = json!({"max_tokens":8192,"system":"tool documentation ".repeat(2000),"messages":[{"role":"user","content":"请分析上下文问题。".repeat(100)}]});
        let b = inspect(&body, &Capabilities::default(), None).unwrap();
        assert!(serde_json::to_vec(&body).unwrap().len() > 32000);
        assert!(b.input_tokens + b.output_tokens < 32000);
        let huge = json!({"max_tokens":8192,"messages":[{"role":"user","content":"中文数据".repeat(20000)}]});
        let b = inspect(&huge, &Capabilities::default(), None).unwrap();
        assert!(b.input_tokens + b.output_tokens > 32000);
        assert!(
            tokenizer(false)
                .unwrap()
                .encode("测试 tools", false)
                .is_ok()
        );
        assert!(tokenizer(true).unwrap().encode("测试 tools", false).is_ok());
    }
}
