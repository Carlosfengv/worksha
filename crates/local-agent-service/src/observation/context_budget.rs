//! Text tokenization and protocol framing estimates; transport byte limits are separate.
use local_agent_core::{
    Provider,
    model_services::{Capabilities, ContextBudget},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
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
// Carried only in this request's extensions through policy admission/rebuilds.
// Include all inputs so a different attempt cannot reuse a stale token budget.
#[derive(Clone)]
pub(super) struct Evaluation {
    fingerprint: [u8; 32],
    pub budget: ContextBudget,
}

fn evaluate_once(
    body: &Value,
    caps: &Capabilities,
    provider: &Provider,
    previous: Option<Evaluation>,
) -> Result<Evaluation, &'static str> {
    let input = serde_json::to_vec(&(body, caps, provider)).map_err(|_| "请求输入无法计数")?;
    let fingerprint = Sha256::digest(input).into();
    if let Some(previous) = previous
        && previous.fingerprint == fingerprint
    {
        return Ok(previous);
    }
    Ok(Evaluation {
        fingerprint,
        budget: inspect(body, caps, Some(provider))?,
    })
}

pub(super) async fn evaluate(
    body: Value,
    caps: Capabilities,
    provider: Provider,
    previous: Option<Evaluation>,
) -> Result<Evaluation, &'static str> {
    static CAPACITY: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);
    let permit = CAPACITY.acquire().await.map_err(|_| "上下文计数暂不可用")?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        evaluate_once(&body, &caps, &provider, previous)
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

    fn deepseek_provider() -> Provider {
        serde_json::from_value(json!({
            "id":"deepseek-test", "name":"DeepSeek", "configured":true,
            "endpoint":"https://api.deepseek.com", "protocol":"chat-completions",
            "model":"deepseek-v4-pro"
        }))
        .unwrap()
    }

    #[test]
    fn request_budget_is_reused_only_for_identical_input_capabilities_and_provider() {
        let body = json!({"max_tokens":160,"messages":[{"role":"user","content":"hello"}]});
        let caps = Capabilities::default();
        let provider = deepseek_provider();
        let mut cached = evaluate_once(&body, &caps, &provider, None).unwrap();
        // Mark the private memo to distinguish reuse from another successful count.
        cached.budget.method = "reused-in-this-request".into();
        assert_eq!(
            evaluate_once(&body, &caps, &provider, Some(cached.clone()))
                .unwrap()
                .budget
                .method,
            "reused-in-this-request"
        );
        for (pointer, value) in [
            ("/messages/0/content", json!("different input")),
            ("/max_tokens", json!(40_000)),
            ("/tools", json!([{"name":"new_tool"}])),
            ("/system", json!("different instructions")),
            ("/tool_choice", json!({"type":"auto"})),
        ] {
            let mut changed = body.clone();
            if pointer.matches('/').count() == 1 {
                changed[&pointer[1..]] = value;
            } else {
                *changed.pointer_mut(pointer).unwrap() = value;
            }
            let budget = evaluate_once(&changed, &caps, &provider, Some(cached.clone()))
                .unwrap()
                .budget;
            assert_ne!(budget.method, "reused-in-this-request", "{pointer}");
            if pointer == "/max_tokens" {
                assert_eq!(budget.output_tokens, 40_000);
                assert!(
                    budget.input_tokens + budget.output_tokens > u64::from(caps.context_window)
                );
            }
        }
        let changed_caps = Capabilities {
            context_window: 64_000,
            ..caps.clone()
        };
        let budget = evaluate_once(&body, &changed_caps, &provider, Some(cached.clone()))
            .unwrap()
            .budget;
        assert_eq!(budget.context_window, 64_000);
        assert_ne!(budget.method, "reused-in-this-request");
        for (endpoint, model) in [
            ("https://api.deepseek.com", "deepseek-v4-flash"),
            ("https://example.test", "deepseek-v4-pro"),
        ] {
            let mut changed = provider.clone();
            changed.endpoint = endpoint.into();
            changed.model = model.into();
            assert_ne!(
                evaluate_once(&body, &caps, &changed, Some(cached.clone()))
                    .unwrap()
                    .budget
                    .method,
                "reused-in-this-request"
            );
        }
    }

    #[test]
    #[ignore = "manual CPU benchmark; no timing assertions"]
    fn compare_repeated_counting_with_request_memo() {
        let body = json!({"max_tokens":160,"messages":[{"role":"user","content":"Benchmark context: preserve all facts, avoid tools. ".repeat(700)}]});
        let caps = Capabilities::default();
        let provider = deepseek_provider();
        let expected = inspect(&body, &caps, Some(&provider)).unwrap();
        let iterations = 10;
        let start = std::time::Instant::now();
        for _ in 0..iterations {
            for _ in 0..2 {
                std::hint::black_box(inspect(&body, &caps, Some(&provider)).unwrap());
            }
        }
        let original = start.elapsed();
        let start = std::time::Instant::now();
        for _ in 0..iterations {
            let first = evaluate_once(&body, &caps, &provider, None).unwrap();
            let next = evaluate_once(&body, &caps, &provider, Some(first)).unwrap();
            assert_eq!(next.budget.input_tokens, expected.input_tokens);
            assert_eq!(next.budget.output_tokens, expected.output_tokens);
            std::hint::black_box(next);
        }
        eprintln!(
            "context budget: input={} output={} iterations={} original_ms={:.3} memo_ms={:.3}",
            expected.input_tokens,
            expected.output_tokens,
            iterations,
            original.as_secs_f64() * 1000.0,
            start.elapsed().as_secs_f64() * 1000.0
        );
    }

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
