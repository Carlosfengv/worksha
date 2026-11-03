//! Explicit, bounded onboarding probes; inference routing never guesses protocols mid-request.
mod plan;
use crate::{
    ApiResult, AppState, json_value,
    provider_models::{self, Catalog},
    providers,
};
use axum::{Json, extract::State};
use local_agent_core::{
    ModelCatalogSource, Provider,
    providers::{ConfigError, validate_endpoint},
};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use zeroize::Zeroizing;

fn fail(code: &str, message: &str) -> ConfigError {
    ConfigError::new(code, message)
}

pub(crate) struct Detected {
    pub(crate) provider: Provider,
    catalog: Option<Arc<Catalog>>,
}
fn attach_catalog(selected: &mut Provider, catalog: Option<&Catalog>) {
    if let Some(catalog) = catalog {
        for model in &catalog.models {
            if !selected.models.contains(&model.id) && selected.models.len() < 2000 {
                selected.models.push(model.id.clone());
            }
        }
        selected.model_catalog = Some(ModelCatalogSource {
            fetched_at: catalog.fetched_at.clone(),
            source_url: catalog.source_url.clone(),
            endpoint: selected.endpoint.clone(),
            protocol: selected.protocol.clone(),
            credential_ref: selected.credential_ref.clone(),
        });
    } else {
        selected.model_catalog = None;
    }
    selected.disabled_models.retain(|id| id != &selected.model);
}
fn model_order(id: &str) -> (bool, &str) {
    (
        ![
            "gpt-",
            "claude-",
            "deepseek-",
            "qwen",
            "llama",
            "gemini",
            "kimi",
        ]
        .iter()
        .any(|prefix| id.starts_with(prefix)),
        id,
    )
}
pub(crate) async fn detect(
    adapters: &provider_models::adapters::Adapters,
    client: &reqwest::Client,
    provider: Provider,
    secret: Option<Zeroizing<Vec<u8>>>,
) -> Result<Detected, ConfigError> {
    detect_checked(adapters, client, provider, secret, || async { Ok(()) }).await
}
pub(crate) async fn detect_checked<F, Fut>(
    adapters: &provider_models::adapters::Adapters,
    client: &reqwest::Client,
    provider: Provider,
    secret: Option<Zeroizing<Vec<u8>>>,
    check: F,
) -> Result<Detected, ConfigError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<(), ConfigError>>,
{
    check().await?;
    let mut candidates = plan::candidates(&provider.endpoint)?;
    let mut catalogs: BTreeMap<String, Option<Arc<Catalog>>> = BTreeMap::new();
    let mut attempted = 0;
    let mut auth_rejected = false;
    let mut saw_model = false;
    for round in 0..3 {
        for candidate in &mut candidates {
            if attempted >= 6 {
                break;
            }
            candidate.credential_ref = provider.credential_ref.clone();
            // Catalog supplements depend on the wire format even when the URL is identical.
            let key = format!(
                "{}:{}",
                provider_models::model_url_with(adapters, candidate)?,
                candidate.protocol
            );
            if !catalogs.contains_key(&key) {
                check().await?;
                let result = tokio::time::timeout(
                    Duration::from_secs(4),
                    provider_models::fetch_detailed_with(
                        adapters,
                        client,
                        candidate,
                        secret.clone(),
                    ),
                )
                .await;
                let result = match result {
                    Ok(Ok(catalog)) => Some(Arc::new(catalog)),
                    Ok(Err(e))
                        if ["catalog_rate_limit", "catalog_upstream", "catalog_redirect"]
                            .contains(&e.code.as_str()) =>
                    {
                        return Err(e);
                    }
                    Ok(Err(e)) => {
                        auth_rejected |= e.code == "catalog_auth";
                        None
                    }
                    Err(_) => None,
                };
                catalogs.insert(key.clone(), result);
            }
            let catalog = catalogs[&key].clone();
            let mut models: Vec<_> = catalog
                .as_ref()
                .into_iter()
                .flat_map(|c| &c.models)
                .map(|m| m.id.clone())
                .filter(|id| !provider.disabled_models.contains(id))
                .collect();
            models.sort_by(|a, b| model_order(a).cmp(&model_order(b)));
            let model = if provider.model.trim().is_empty() {
                models.get(round).cloned()
            } else if round == 0 {
                Some(provider.model.trim().into())
            } else {
                None
            };
            let Some(model) = model else { continue };
            saw_model = true;
            attempted += 1;
            let mut selected = provider.clone();
            selected.endpoint = candidate.endpoint.clone();
            selected.protocol = candidate.protocol.clone();
            if selected.id.starts_with("custom-") {
                selected.vendor = candidate.vendor.clone();
            }
            selected.model = model;
            selected.configured = true;
            check().await?;
            match providers::probe_detailed_with(adapters, client, &selected, secret.clone()).await
            {
                Ok(_) => {
                    check().await?;
                    attach_catalog(&mut selected, catalog.as_deref());
                    return Ok(Detected {
                        provider: selected,
                        catalog,
                    });
                }
                Err(e)
                    if [
                        "probe_network",
                        "probe_rate_limit",
                        "probe_upstream",
                        "probe_redirect",
                    ]
                    .contains(&e.code.as_str()) =>
                {
                    return Err(e);
                }
                Err(e) => {
                    auth_rejected |= e.code == "probe_auth";
                }
            }
        }
        if attempted >= 6 {
            break;
        }
    }
    // Catalog visibility alone is never evidence that an inference format works.
    Err(if auth_rejected {
        fail(
            "connection_auth",
            "连接验证被拒绝，请检查 API Key 和模型权限；没有保存连接",
        )
    } else if !saw_model {
        fail(
            "needs_model",
            "无法取得可验证的模型。补充模型 ID 后重试，连接方式仍会自动识别",
        )
    } else {
        fail(
            "connection_unknown",
            "尚未确认可调用的连接方式；没有保存连接。可指定模型重试或展开高级设置",
        )
    })
}

/// An explicitly selected catalog fixes the base protocol and endpoint. Only
/// the host's bounded verification is attempted; directory failure has no fallback.
pub(crate) async fn detect_catalog<F, Fut>(
    adapters: &provider_models::adapters::Adapters,
    client: &reqwest::Client,
    provider: Provider,
    secret: Option<Zeroizing<Vec<u8>>>,
    catalog: Catalog,
    check: F,
) -> Result<Detected, ConfigError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<(), ConfigError>>,
{
    let mut models: Vec<_> = catalog
        .models
        .iter()
        .map(|m| m.id.clone())
        .filter(|id| !provider.disabled_models.contains(id))
        .collect();
    models.sort_by(|a, b| model_order(a).cmp(&model_order(b)));
    if !provider.model.trim().is_empty() {
        models = vec![provider.model.trim().into()];
    }
    if models.is_empty() {
        return Err(fail(
            "needs_model",
            "所选目录未提供可验证模型，请补充模型 ID；未改用内置目录",
        ));
    }
    let mut auth = false;
    for model in models.into_iter().take(3) {
        check().await?;
        let mut selected = provider.clone();
        selected.model = model;
        selected.configured = true;
        match providers::probe_detailed_with(adapters, client, &selected, secret.clone()).await {
            Ok(_) => {
                check().await?;
                attach_catalog(&mut selected, Some(&catalog));
                return Ok(Detected {
                    provider: selected,
                    catalog: Some(Arc::new(catalog)),
                });
            }
            Err(error) if ["probe_auth", "probe_rejected"].contains(&error.code.as_str()) => {
                auth |= error.code == "probe_auth";
            }
            // A fixed protocol needs no further format guess. Malformed or
            // interrupted output may already follow a model execution.
            Err(error) => return Err(error),
        }
    }
    Err(if auth {
        fail(
            "connection_auth",
            "连接验证被拒绝，请检查 API Key 和模型权限；未保存连接",
        )
    } else {
        fail(
            "connection_unknown",
            "所选目录的基础协议尚未验证成功；未保存连接或改用其他目录",
        )
    })
}

pub(crate) async fn prepare(
    state: &AppState,
    mut input: Provider,
) -> Result<(Provider, Option<Zeroizing<Vec<u8>>>), ConfigError> {
    let url = validate_endpoint(input.endpoint.trim())?;
    if input.name.trim().is_empty() {
        input.name = url.host_str().unwrap_or("模型连接").into();
    }
    if input.name.chars().count() > 100
        || input.model.len() > 200
        || input.model.chars().any(char::is_control)
        || input.models.len() > 2000
        || input.disabled_models.len() > 2000
    {
        return Err(fail("invalid_provider", "连接名称或模型信息无效"));
    }
    let store = state.store.clone();
    let provider_id = input.id.clone();
    let current = crate::blocking::run(move || {
        Ok::<_, ConfigError>(
            store
                .lock()
                .map_err(|_| fail("service_unavailable", "数据服务暂不可用"))?
                .providers()
                .map_err(|_| fail("service_unavailable", "连接配置暂不可读取"))?
                .into_iter()
                .find(|p| p.id == provider_id),
        )
    })
    .await
    .map_err(crate::blocking::config_error)??;
    if input.revision != current.as_ref().map_or(0, |p| p.revision)
        || current.is_none()
            && (!input.id.starts_with("custom-") || uuid::Uuid::parse_str(&input.id[7..]).is_err())
    {
        return Err(fail("conflict", "连接版本已改变，请重新读取"));
    }
    if input.credential_ref.is_none() && !local_agent_core::providers::is_loopback_url(&url) {
        return Err(fail("invalid_credential", "请先填写 API Key"));
    }
    let secret = if let Some(reference) = &input.credential_ref {
        Some(crate::blocking::secret(state.credentials.clone(), reference.clone()).await?)
    } else {
        None
    };
    if secret
        .as_deref()
        .and_then(|v| std::str::from_utf8(v).ok())
        .is_some_and(|key| !key.is_empty() && input.endpoint.contains(key))
    {
        return Err(fail("invalid_provider", "请从服务地址中移除 API Key"));
    }
    Ok((input, secret))
}

pub(crate) async fn connect(
    State(state): State<AppState>,
    Json(input): Json<Provider>,
) -> ApiResult {
    let _permit = state
        .verification_capacity
        .clone()
        .try_acquire_owned()
        .map_err(|_| providers::error(fail("busy", "已有连接验证正在进行，请稍后重试")))?;
    let (input, secret) = prepare(&state, input).await.map_err(providers::error)?;
    let detected = tokio::time::timeout(
        Duration::from_secs(45),
        detect(
            &state.provider_adapters,
            &state.provider_client,
            input,
            secret,
        ),
    )
    .await
    .map_err(|_| {
        providers::error(fail(
            "connection_timeout",
            "自动连接超时，尚未保存；请检查网络后重试",
        ))
    })?
    .map_err(providers::error)?;
    let checked_at = chrono::Utc::now().to_rfc3339();
    crate::blocking::run(move || {
        let manager = state.providers.lock().map_err(|_| crate::error("配置服务暂不可用".into()))?;
        let store = state.store.lock().map_err(|_| crate::error("数据服务暂不可用".into()))?;
        let (provider, verification) = manager.save_connected_provider(&store, detected.provider, checked_at)
            .map_err(providers::error)?;
        json_value(serde_json::json!({"provider":provider,"verification":verification,"catalog":detected.catalog.as_deref()}))
    }).await.map_err(|e| providers::error(crate::blocking::config_error(e)))?
}
