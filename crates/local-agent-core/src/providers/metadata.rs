//! One metadata validation and persistence path for legacy and receipted writes.
use super::ConfigError;
use crate::{Provider, Store, providers, storage::default_providers};
use rusqlite::{Transaction, TransactionBehavior, params};
fn invalid(message: &str) -> ConfigError {
    ConfigError::new("invalid_provider", message)
}
fn unavailable() -> ConfigError {
    ConfigError::new(
        "service_unavailable",
        "服务商配置暂不可保存，请核查原操作结果",
    )
}
impl Store {
    /// Save metadata under one write transaction, including the revision check.
    pub fn save_provider(&self, provider: Provider) -> Result<(), String> {
        let transaction = if self.connection.is_autocommit() {
            Some(
                Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
                    .map_err(|_| unavailable().message)?,
            )
        } else {
            None
        };
        self.write_provider(provider)
            .map_err(|error| error.message)?;
        if let Some(transaction) = transaction {
            transaction.commit().map_err(|_| unavailable().message)?;
        }
        Ok(())
    }
    pub(crate) fn write_provider(&self, mut provider: Provider) -> Result<Provider, ConfigError> {
        if self.connection.is_autocommit() {
            return Err(unavailable());
        }
        provider = self.normalize_provider(provider)?;
        let value = serde_json::to_string(&provider).map_err(|_| unavailable())?;
        let changed = self
            .connection
            .execute(
                "INSERT INTO providers(id,value) VALUES(?1,?2)
             ON CONFLICT(id) DO UPDATE SET value=excluded.value",
                params![provider.id, value],
            )
            .map_err(|_| unavailable())?;
        if changed != 1 {
            return Err(unavailable());
        }
        Ok(provider)
    }
    /// Validate and normalize the complete metadata before an external import write.
    pub(crate) fn normalize_provider(
        &self,
        mut provider: Provider,
    ) -> Result<Provider, ConfigError> {
        if !(default_providers().iter().any(|p| p.id == provider.id)
            || provider.id.starts_with("custom-")
                && uuid::Uuid::parse_str(&provider.id[7..]).is_ok())
        {
            return Err(invalid("未知服务商"));
        }
        let previous = self
            .providers()
            .map_err(|_| unavailable())?
            .into_iter()
            .find(|p| p.id == provider.id);
        if provider.revision != previous.as_ref().map_or(0, |p| p.revision) {
            return Err(ConfigError::new(
                "conflict",
                "服务商已被修改，请重新读取后再保存",
            ));
        }
        provider.name = provider.name.trim().to_string();
        if provider.vendor.is_empty() {
            provider.vendor = if default_providers().iter().any(|p| p.id == provider.id) {
                provider.id.clone()
            } else {
                "custom".into()
            };
        }
        if !providers::catalog::builtin().contains(&provider.vendor)
            || (!provider.id.starts_with("custom-") && provider.vendor != provider.id)
        {
            return Err(invalid("请选择有效的供应商"));
        }
        if provider.name.is_empty()
            || provider.name.chars().count() > 100
            || provider.endpoint.len() > 2048
            || provider.model.chars().count() > 200
        {
            return Err(invalid("请检查名称、端点与模型长度"));
        }
        if !["responses", "messages", "chat-completions"].contains(&provider.protocol.as_str()) {
            return Err(invalid("不支持的接口协议"));
        }
        let endpoint = url::Url::parse(provider.endpoint.trim())
            .map_err(|_| invalid("请输入完整的 HTTP 或 HTTPS 地址"))?;
        if !["http", "https"].contains(&endpoint.scheme())
            || endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(invalid(
                "端点必须为 HTTP(S) 地址，不能包含凭据、查询参数或片段",
            ));
        }
        let loopback = providers::is_loopback_url(&endpoint);
        if endpoint.scheme() == "http" && !loopback {
            return Err(invalid("远程 Endpoint 需要 HTTPS；HTTP 仅支持本机回环地址"));
        }
        // Retain the legacy field for stored snapshots; it no longer controls UI grouping or trust.
        provider.category = if loopback {
            "local"
        } else if ["ollama", "vllm", "custom"].contains(&provider.vendor.as_str()) {
            "custom"
        } else {
            "cloud"
        }
        .into();
        provider.endpoint = endpoint.to_string().trim_end_matches('/').into();
        if let Some(source) = &provider.model_catalog {
            if source.endpoint != provider.endpoint
                || source.protocol != provider.protocol
                || source.credential_ref != provider.credential_ref
            {
                provider.model_catalog = None;
            } else {
                let source_url = providers::validate_endpoint(&source.source_url)
                    .map_err(|_| invalid("模型目录来源地址无效"))?;
                if source.source_url.len() > 2048
                    || source_url.origin() != endpoint.origin()
                    || chrono::DateTime::parse_from_rfc3339(&source.fetched_at).is_err()
                {
                    return Err(invalid("模型目录来源或获取时间无效"));
                }
            }
        }
        provider.model = provider.model.trim().into();
        if !["cloud", "custom", "local"].contains(&provider.category.as_str())
            || provider.models.len() > 2000
            || provider
                .models
                .iter()
                .any(|m| m.is_empty() || m.chars().count() > 200 || m.chars().any(char::is_control))
            || provider
                .credential_ref
                .as_ref()
                .is_some_and(|s| uuid::Uuid::parse_str(s).is_err())
            || provider
                .credential_label
                .as_ref()
                .is_some_and(|s| s.len() > 200 || s.chars().any(char::is_control))
            || provider.model.chars().any(char::is_control)
        {
            return Err(invalid("模型、凭据引用或连接分类无效"));
        }
        if !provider.model.is_empty() && !provider.models.contains(&provider.model) {
            provider.models.push(provider.model.clone());
        }
        if let Some(previous) = previous {
            for model in previous.models {
                if !provider.models.contains(&model) {
                    provider.models.push(model);
                }
            }
        }
        if provider.models.len() > 2000 {
            return Err(invalid("模型目录最多保留 2000 项"));
        }
        if provider.disabled_models.len() > 2000
            || provider
                .disabled_models
                .iter()
                .any(|id| !provider.models.contains(id))
        {
            return Err(invalid("停用列表只能包含已登记的模型"));
        }
        provider.disabled_models.sort();
        provider.disabled_models.dedup();
        provider.revision = provider
            .revision
            .checked_add(1)
            .filter(|revision| *revision <= 9_007_199_254_740_991)
            .ok_or_else(|| invalid("服务商版本已达上限"))?;
        // Connection verification is separate from saving metadata.
        provider.configured = true;
        Ok(provider)
    }
}
