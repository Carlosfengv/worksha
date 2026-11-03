//! Read-only model discovery. Never forward upstream payloads, secrets, or redirects to the UI.
use crate::{ApiResult, AppState, json_value};
use axum::{Json, extract::State};
use local_agent_core::{
    domain::provider as d,
    providers::{ConfigError, validate_endpoint},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, time::Duration};
use zeroize::Zeroizing;

const MAX_MODELS: usize = 2000;
const MAX_BYTES: usize = 4 * 1024 * 1024;
pub(crate) mod adapters;
mod metadata;
mod official;

#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelsRequest {
    pub(crate) endpoint: String,
    pub(crate) protocol: String,
    pub(crate) vendor: String,
    pub(crate) credential_ref: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub(crate) id: String,
    name: String,
    #[serde(flatten)]
    metadata: metadata::Metadata,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub(crate) models: Vec<Model>,
    pub(crate) fetched_at: String,
    pub(crate) source_url: String,
    truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    observations: Option<local_agent_core::providers::model_observations::Observations>,
}
fn invalid(message: &str) -> ConfigError {
    ConfigError::new("models_unavailable", message)
}
#[cfg(test)]
pub(crate) fn model_url(input: &ModelsRequest) -> Result<url::Url, ConfigError> {
    model_url_with(&adapters::builtin(), input)
}
pub(crate) fn model_url_with(
    adapters: &adapters::Adapters,
    input: &ModelsRequest,
) -> Result<url::Url, ConfigError> {
    if input.endpoint.len() > 2048 || input.vendor.len() > 50 {
        return Err(invalid("Endpoint 或供应商标识超出长度限制"));
    }
    let adapter = adapters.resolve(&input.protocol)?;
    let mut url = validate_endpoint(&input.endpoint)?;
    let context = d::CatalogContext {
        protocol: input.protocol.clone(),
        vendor: input.vendor.clone(),
        base_path: url.path().into(),
        official_catalog_root: input.vendor == "deepseek"
            && url.host_str() == Some("api.deepseek.com"),
    };
    let request = adapter.catalog_request(&context)?;
    adapter.ensure_active()?;
    request.validate(&context)?;
    url.set_path(&request.path);
    Ok(url)
}
pub(crate) fn safe_label(value: &str, limit: usize, secret: Option<&str>) -> bool {
    !value.trim().is_empty()
        && value.len() <= limit
        && !value.chars().any(char::is_control)
        && secret.is_none_or(|key| key.is_empty() || !value.contains(key))
}
#[cfg(test)]
fn read_models(
    body: &Value,
    models: &mut BTreeMap<String, Model>,
    secret: Option<&str>,
) -> Result<bool, ConfigError> {
    read_models_with(
        &*adapters::builtin().resolve("responses")?,
        body,
        models,
        secret,
    )
    .map(|v| v.0)
}
pub(crate) fn read_models_with(
    adapter: &dyn d::ProviderAdapter,
    body: &Value,
    models: &mut BTreeMap<String, Model>,
    secret: Option<&str>,
) -> Result<(bool, d::CatalogContinuation), ConfigError> {
    let mut truncated = false;
    let continuation = adapter.parse_catalog(body, &mut |row| {
        let id = row.id.as_str();
        if !safe_label(id, 200, secret) {
            return Err(invalid("模型目录包含无效标识，未替换已有模型"));
        }
        if models.len() >= MAX_MODELS && !models.contains_key(id) {
            truncated = true;
            return Ok(());
        }
        let name = row.name.as_str();
        row.metadata.validate()?;
        models.insert(
            id.into(),
            Model {
                id: id.into(),
                name: if safe_label(name, 200, secret) {
                    name.into()
                } else {
                    id.into()
                },
                metadata: row.metadata.into(),
            },
        );
        Ok(())
    })?;
    Ok((truncated, continuation))
}
pub(crate) fn external_catalog(
    input: &ModelsRequest,
    mut models: BTreeMap<String, Model>,
    source_url: String,
    truncated: bool,
) -> Catalog {
    let fetched_at = chrono::Utc::now();
    official::supplement(input, &mut models, fetched_at);
    Catalog {
        models: models.into_values().collect(),
        fetched_at: fetched_at.to_rfc3339(),
        source_url,
        truncated,
        observations: None,
    }
}
impl Catalog {
    pub(crate) fn observations(
        &mut self,
        value: local_agent_core::providers::model_observations::Observations,
    ) {
        self.observations = Some(value);
    }
}
pub async fn discover(
    State(state): State<AppState>,
    Json(input): Json<ModelsRequest>,
) -> ApiResult {
    let adapters = state.provider_adapters.clone();
    model_url_with(&adapters, &input).map_err(crate::providers::error)?;
    let _permit = state
        .verification_capacity
        .clone()
        .try_acquire_owned()
        .map_err(|_| crate::providers::error(invalid("已有连接请求正在进行，请稍后重试")))?;
    let secret = if let Some(reference) = &input.credential_ref {
        Some(
            crate::blocking::secret(state.credentials.clone(), reference.clone())
                .await
                .map_err(crate::providers::error)?,
        )
    } else {
        None
    };
    let connection = input.clone();
    let client = state.provider_client.clone();
    let observations = crate::blocking::run(move || {
        let manager = state
            .providers
            .lock()
            .map_err(|_| invalid("凭据服务暂不可用"))?;
        let observations = manager.model_observations(
            &connection.endpoint,
            &connection.protocol,
            connection.credential_ref.as_deref(),
        );
        Ok::<_, ConfigError>(observations)
    })
    .await
    .map_err(|e| crate::providers::error(crate::blocking::config_error(e)))?
    .map_err(crate::providers::error)?;
    let mut result = tokio::time::timeout(
        Duration::from_secs(15),
        fetch_with(&adapters, &client, &input, secret),
    )
    .await
    .map_err(|_| crate::providers::error(invalid("获取模型超时；已有模型保持不变")))?
    .map_err(crate::providers::error)?;
    result.observations = Some(observations);
    json_value(result)
}
#[cfg(test)]
pub(crate) async fn fetch(
    client: &reqwest::Client,
    input: &ModelsRequest,
    secret: Option<Zeroizing<Vec<u8>>>,
) -> Result<Catalog, ConfigError> {
    fetch_with(&adapters::builtin(), client, input, secret).await
}
pub(crate) async fn fetch_with(
    adapters: &adapters::Adapters,
    client: &reqwest::Client,
    input: &ModelsRequest,
    secret: Option<Zeroizing<Vec<u8>>>,
) -> Result<Catalog, ConfigError> {
    fetch_detailed_with(adapters, client, input, secret)
        .await
        .map_err(|e| {
            if e.code.starts_with("catalog_") {
                invalid(&e.message)
            } else {
                e
            }
        })
}
#[cfg(test)]
pub(crate) async fn fetch_detailed(
    client: &reqwest::Client,
    input: &ModelsRequest,
    secret: Option<Zeroizing<Vec<u8>>>,
) -> Result<Catalog, ConfigError> {
    fetch_detailed_with(&adapters::builtin(), client, input, secret).await
}
pub(crate) async fn fetch_detailed_with(
    adapters: &adapters::Adapters,
    client: &reqwest::Client,
    input: &ModelsRequest,
    secret: Option<Zeroizing<Vec<u8>>>,
) -> Result<Catalog, ConfigError> {
    fetch_detailed_checked(adapters, client, input, secret, || async { Ok(()) }).await
}
pub(crate) async fn fetch_with_checked<C, F>(
    adapters: &adapters::Adapters,
    client: &reqwest::Client,
    input: &ModelsRequest,
    secret: Option<Zeroizing<Vec<u8>>>,
    check: C,
) -> Result<Catalog, ConfigError>
where
    C: Fn() -> F,
    F: std::future::Future<Output = Result<(), ConfigError>>,
{
    fetch_detailed_checked(adapters, client, input, secret, check)
        .await
        .map_err(|e| {
            if e.code.starts_with("catalog_") && !e.code.starts_with("catalog_source_") {
                invalid(&e.message)
            } else {
                e
            }
        })
}
async fn fetch_detailed_checked<C, F>(
    adapters: &adapters::Adapters,
    client: &reqwest::Client,
    input: &ModelsRequest,
    secret: Option<Zeroizing<Vec<u8>>>,
    check: C,
) -> Result<Catalog, ConfigError>
where
    C: Fn() -> F,
    F: std::future::Future<Output = Result<(), ConfigError>>,
{
    check().await?;
    let url = model_url_with(adapters, input)?;
    let adapter = adapters.resolve(&input.protocol)?;
    let wire = crate::gateway::capabilities::builtin()
        .resolve(&input.protocol)
        .map_err(|_| invalid("不支持此接口协议的模型目录"))?;
    let source_url = url.to_string();
    let key = secret
        .as_deref()
        .map(|bytes| std::str::from_utf8(bytes))
        .transpose()
        .map_err(|_| invalid("凭据格式无效"))?;
    let mut next = url;
    let mut models = BTreeMap::new();
    let mut truncated = false;
    let mut cursors = std::collections::BTreeSet::new();
    for page in 0..5 {
        adapter.ensure_active()?;
        let mut request = client.get(next.clone()).timeout(Duration::from_secs(8));
        if let Some(key) = key {
            request = request.bearer_auth(key);
            if wire.api_key_header() {
                request = request.header("x-api-key", key);
            }
        }
        for (name, value) in wire.headers() {
            request = request.header(*name, *value);
        }
        check().await?;
        let mut response = request
            .send()
            .await
            .map_err(|_| invalid("无法连接模型目录，请检查网络、证书和 Endpoint"))?;
        adapter.ensure_active()?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            return Err(ConfigError::new(
                match status {
                    401 | 403 => "catalog_auth",
                    429 => "catalog_rate_limit",
                    300..=399 => "catalog_redirect",
                    500..=599 => "catalog_upstream",
                    _ => "models_unavailable",
                },
                match status {
                    401 | 403 => "模型目录鉴权被拒绝，请检查 API Key 与目录权限",
                    404 | 405 | 501 => "此 Endpoint 不提供标准模型目录，可继续手动填写模型 ID",
                    429 => "模型目录请求被限流，请稍后重试",
                    300..=399 => "模型目录要求重定向；为保护凭据，未自动跟随",
                    _ => "服务商未能返回模型目录；已有模型保持不变",
                },
            ));
        }
        let mut bytes = Zeroizing::new(Vec::new());
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| invalid("模型目录响应中断"))?
        {
            check().await?;
            adapter.ensure_active()?;
            if bytes.len() + chunk.len() > MAX_BYTES {
                return Err(invalid("模型目录响应超过 4 MiB，未加载"));
            }
            bytes.extend_from_slice(&chunk);
        }
        check().await?;
        let body: Value = serde_json::from_slice(&bytes)
            .map_err(|_| invalid("模型目录不是 JSON 结果，可继续手动填写"))?;
        let (page_truncated, continuation) = read_models_with(&*adapter, &body, &mut models, key)?;
        adapter.ensure_active()?;
        truncated |= page_truncated;
        if !continuation.has_more {
            break;
        }
        if page == 4 || models.len() >= MAX_MODELS {
            truncated = true;
            break;
        }
        let cursor = continuation
            .after_id
            .as_deref()
            .filter(|v| safe_label(v, 200, key))
            .ok_or_else(|| invalid("模型目录分页信息无效，未加载"))?;
        if !cursors.insert(cursor.to_string()) {
            return Err(invalid("模型目录分页重复，已停止读取"));
        }
        next.query_pairs_mut()
            .clear()
            .append_pair("after_id", cursor);
    }
    let fetched_at = chrono::Utc::now();
    official::supplement(input, &mut models, fetched_at);
    check().await?;
    adapter.ensure_active()?;
    Ok(Catalog {
        models: models.into_values().collect(),
        fetched_at: fetched_at.to_rfc3339(),
        source_url,
        truncated,
        observations: None,
    })
}

#[cfg(test)]
mod adapter_tests;
#[cfg(test)]
mod tests;
