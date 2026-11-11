//! Authorization and endpoint checks for first-party plan credentials.
use super::{ConfigError, fail};
use crate::Provider;

pub const MINIMAX_PLAN: &str = "minimax-plan";

pub fn minimax_plan(provider: &Provider) -> bool {
    provider.vendor == MINIMAX_PLAN || (provider.vendor.is_empty() && provider.id == MINIMAX_PLAN)
}

pub fn intrinsic_reasoning(provider: &Provider) -> bool {
    minimax_plan(provider)
        && matches!(
            provider.model.as_str(),
            "MiniMax-M3.1-Flash" | "MiniMax-M3.1-Flash-Preview"
        )
}

pub fn validate_endpoint(provider: &Provider) -> Result<(), ConfigError> {
    if !minimax_plan(provider) {
        return Ok(());
    }
    let endpoint = provider.endpoint.trim_end_matches('/');
    let supported = match provider.protocol.as_str() {
        "messages" => matches!(
            endpoint,
            "https://api.minimax.io/anthropic"
                | "https://api.minimax.io/anthropic/v1"
                | "https://api.minimax.io/anthropic/v1/messages"
        ),
        "chat-completions" => matches!(
            endpoint,
            "https://api.minimax.io/v1" | "https://api.minimax.io/v1/chat/completions"
        ),
        "responses" => matches!(
            endpoint,
            "https://api.minimax.io/v1" | "https://api.minimax.io/v1/responses"
        ),
        _ => false,
    };
    if !supported {
        return fail(
            "invalid_provider",
            "MiniMax M Plan 仅接受所选协议的国际站官方端点",
        );
    }
    Ok(())
}

pub fn validate_key(provider: &Provider, key: Option<&[u8]>) -> Result<(), ConfigError> {
    if !minimax_plan(provider) {
        return Ok(());
    }
    validate_endpoint(provider)?;
    if !key
        .and_then(|key| key.strip_prefix(b"sk-cp-"))
        .is_some_and(|suffix| {
            !suffix.is_empty() && suffix.len() <= 8186 && suffix.iter().all(u8::is_ascii_graphic)
        })
    {
        return fail(
            "invalid_credential",
            "请使用 MiniMax M Plan 控制台签发的 sk-cp- 订阅 Key",
        );
    }
    Ok(())
}
