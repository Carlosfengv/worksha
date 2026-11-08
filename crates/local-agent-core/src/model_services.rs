//! Published model services own the client catalog. Providers remain supply snapshots.
use crate::{
    Provider, Store,
    providers::{ConfigError, fail, select_model, validate_source},
};
use rusqlite::{Connection, OptionalExtension, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
pub mod notices;
pub mod operations;
pub mod policy;
use policy::RoutingPolicy;

pub const CONNECTION_AGENTS: [&str; 4] = ["claude", "opencode", "claude-desktop", "codex"];

pub fn client_model_id(agent: &str, alias: &str) -> String {
    if agent == "claude-desktop" {
        crate::providers::claude_desktop_model_id("model-service", alias)
    } else {
        alias.into()
    }
}

fn error(_: impl std::fmt::Display) -> ConfigError {
    ConfigError::new(
        "service_unavailable",
        "模型服务存储不可用，已发布版本保持不变",
    )
}
pub(crate) fn initialize(db: &Connection) -> Result<(), ConfigError> {
    db.execute_batch("BEGIN IMMEDIATE;
      CREATE TABLE IF NOT EXISTS model_services(id TEXT PRIMARY KEY,revision INTEGER NOT NULL,current TEXT NOT NULL,deleted INTEGER NOT NULL DEFAULT 0 CHECK(deleted IN(0,1)));
      CREATE TABLE IF NOT EXISTS model_service_versions(id TEXT PRIMARY KEY,service TEXT NOT NULL REFERENCES model_services(id) DEFERRABLE INITIALLY DEFERRED,value TEXT NOT NULL CHECK(json_valid(value)));
      CREATE INDEX IF NOT EXISTS model_service_history ON model_service_versions(service);
      CREATE TABLE IF NOT EXISTS model_service_bindings(binding TEXT PRIMARY KEY REFERENCES gateway_bindings(id),service TEXT NOT NULL REFERENCES model_services(id),version TEXT NOT NULL REFERENCES model_service_versions(id));
      CREATE TABLE IF NOT EXISTS model_service_targets(binding TEXT NOT NULL REFERENCES gateway_bindings(id),version TEXT NOT NULL REFERENCES model_service_versions(id),model TEXT NOT NULL,route TEXT NOT NULL REFERENCES gateway_route_versions(id),PRIMARY KEY(binding,version,model));
      COMMIT;").map_err(error)?;
    let columns = db
        .prepare("PRAGMA table_info(gateway_route_leases)")
        .map_err(error)?
        .query_map([], |r| r.get::<_, String>(1))
        .map_err(error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(error)?;
    if !columns.iter().any(|c| c == "service_selection") {
        db.execute_batch("ALTER TABLE gateway_route_leases ADD COLUMN service_selection TEXT CHECK(service_selection IS NULL OR json_valid(service_selection));").map_err(error)?;
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Capabilities {
    pub context_window: u32,
    pub max_output_tokens: u32,
    pub tools: bool,
    pub thinking: bool,
    pub input_modalities: Vec<String>,
}
impl Default for Capabilities {
    fn default() -> Self {
        Self {
            context_window: 32_000,
            max_output_tokens: 8192,
            tools: true,
            thinking: false,
            input_modalities: vec!["text".into()],
        }
    }
}
impl Capabilities {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !(2048..=2_000_000).contains(&self.context_window)
            || !(16..=65_536).contains(&self.max_output_tokens)
            || self.max_output_tokens >= self.context_window
        {
            return fail(
                "invalid_capabilities",
                "上下文需为 2048–2000000，输出需为 16–65536 且小于上下文",
            );
        }
        if self.thinking || self.input_modalities != ["text"] {
            return fail(
                "unsupported_capabilities",
                "首批模型服务支持文本和标准工具；思考、图片及文件能力尚未发布",
            );
        }
        Ok(())
    }
    pub fn covers(&self, required: &Self) -> bool {
        self.context_window >= required.context_window
            && self.max_output_tokens >= required.max_output_tokens
            && (!required.tools || self.tools)
            && (!required.thinking || self.thinking)
            && required
                .input_modalities
                .iter()
                .all(|m| self.input_modalities.contains(m))
    }
    pub fn common(models: &[ServiceModel]) -> Self {
        Self {
            context_window: models
                .iter()
                .map(|m| m.capabilities.context_window)
                .min()
                .unwrap_or(0),
            max_output_tokens: models
                .iter()
                .map(|m| m.capabilities.max_output_tokens)
                .min()
                .unwrap_or(0),
            tools: models.iter().all(|m| m.capabilities.tools),
            thinking: false,
            input_modalities: vec!["text".into()],
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelDraft {
    pub id: String,
    pub name: String,
    pub provider_id: String,
    pub model: String,
    pub capabilities: Capabilities,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServiceDraft {
    pub id: String,
    pub revision: u64,
    pub name: String,
    pub default_model_id: String,
    pub models: Vec<ModelDraft>,
    #[serde(default)]
    pub routing_policy: RoutingPolicy,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServiceModel {
    pub id: String,
    pub name: String,
    pub provider: Provider,
    pub capabilities: Capabilities,
}
impl ServiceModel {
    pub fn alias(&self) -> String {
        format!("local-agent-model-{}", self.id)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModelService {
    pub id: String,
    pub version_id: String,
    pub revision: u64,
    pub name: String,
    pub default_model_id: String,
    pub models: Vec<ServiceModel>,
    pub published_at: String,
    #[serde(default)]
    pub routing_policy: RoutingPolicy,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceAttachment {
    pub binding_id: String,
    pub service_id: String,
    pub installed: ModelService,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_endpoint: Option<String>,
    #[serde(skip)]
    pub previous_codex_endpoints: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextBudget {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub context_window: u32,
    pub method: String,
    pub estimated: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceSelection {
    pub service_id: String,
    pub service_version_id: String,
    pub service_name: String,
    pub model_id: String,
    pub model_name: String,
    pub requested_alias: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_model_name: Option<String>,
    #[serde(default)]
    pub attempt: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<ContextBudget>,
    pub mode: String,
    pub capabilities: Capabilities,
}

pub(crate) fn version(db: &Connection, id: &str) -> Result<ModelService, ConfigError> {
    let value: String = db
        .query_row(
            "SELECT value FROM model_service_versions WHERE id=?1",
            [id],
            |r| r.get(0),
        )
        .map_err(error)?;
    serde_json::from_str(&value).map_err(error)
}
pub(crate) fn current(db: &Connection, id: &str) -> Result<ModelService, ConfigError> {
    let id: Option<String> = db
        .query_row(
            "SELECT current FROM model_services WHERE id=?1 AND deleted=0",
            [id],
            |r| r.get(0),
        )
        .optional()
        .map_err(error)?;
    version(
        db,
        &id.ok_or_else(|| ConfigError::new("not_found", "模型服务已删除或不存在"))?,
    )
}
pub(crate) fn attachment(
    db: &Connection,
    binding: &str,
) -> Result<Option<ServiceAttachment>, ConfigError> {
    let raw = db
        .query_row(
            "SELECT service,version FROM model_service_bindings WHERE binding=?1",
            [binding],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(error)?;
    raw.map(|(service_id, id)| {
        Ok(ServiceAttachment {
            binding_id: binding.into(),
            service_id,
            installed: version(db, &id)?,
            previous_endpoint: None,
            previous_codex_endpoints: Vec::new(),
        })
    })
    .transpose()
}
pub(crate) fn attach_binding(
    db: &Connection,
    binding: &str,
    service: &ModelService,
) -> Result<ServiceAttachment, ConfigError> {
    let expected = serde_json::to_string(service).map_err(error)?;
    if serde_json::to_string(&current(db, &service.id)?).map_err(error)? != expected {
        return fail("conflict", "模型服务已更新，请重新准备接入");
    }
    if db
        .execute(
            "INSERT INTO model_service_bindings VALUES(?1,?2,?3)",
            params![binding, service.id, service.version_id],
        )
        .map_err(error)?
        != 1
    {
        return Err(error("attachment not inserted"));
    }
    let saved = attachment(db, binding)?.ok_or_else(|| error("missing attachment"))?;
    if saved.binding_id != binding
        || saved.service_id != service.id
        || serde_json::to_string(&saved.installed).map_err(error)? != expected
    {
        return Err(error("attachment readback changed"));
    }
    Ok(saved)
}
impl ServiceAttachment {
    pub fn alias_for_client_model(&self, agent: &str, model: &str) -> Option<String> {
        if agent != "claude-desktop" {
            return self.accepts(model).then(|| model.into());
        }
        std::iter::once(crate::routing::ALIAS.to_owned())
            .chain(self.installed.models.iter().map(ServiceModel::alias))
            .find(|alias| client_model_id(agent, alias) == model)
    }

    pub fn accepts(&self, alias: &str) -> bool {
        alias == crate::routing::ALIAS || self.installed.models.iter().any(|m| m.alias() == alias)
    }
    pub fn catalog(&self) -> Vec<(String, String, Capabilities)> {
        std::iter::once((
            crate::routing::ALIAS.into(),
            format!("{} · 默认", self.installed.name),
            Capabilities::common(&self.installed.models),
        ))
        .chain(
            self.installed
                .models
                .iter()
                .map(|m| (m.alias(), m.name.clone(), m.capabilities.clone())),
        )
        .collect()
    }
    pub fn resolve(
        &self,
        db: &Connection,
        alias: &str,
    ) -> Result<(ModelService, ServiceModel, ServiceSelection), ConfigError> {
        if !self.accepts(alias) {
            return fail(
                "invalid_model",
                "请选择此模型服务已注入的模型；新增目录需要重新同步客户端",
            );
        }
        let service = current(db, &self.service_id)?;
        let (id, required, mode) = if alias == crate::routing::ALIAS {
            (
                &service.default_model_id,
                Capabilities::common(&self.installed.models),
                "default",
            )
        } else {
            let installed = self
                .installed
                .models
                .iter()
                .find(|m| m.alias() == alias)
                .ok_or_else(|| ConfigError::new("invalid_model", "未发布此模型"))?;
            (&installed.id, installed.capabilities.clone(), "specified")
        };
        let model = service
            .models
            .iter()
            .find(|m| &m.id == id)
            .cloned()
            .ok_or_else(|| {
                ConfigError::new(
                    "catalog_sync_required",
                    "此模型已从服务目录移除，请同步目录或选择默认",
                )
            })?;
        if !model.capabilities.covers(&required) {
            return fail(
                "catalog_sync_required",
                "目标能力低于客户端已声明范围，请先同步模型目录；未降级或截断历史",
            );
        }
        let selection = ServiceSelection {
            service_id: service.id.clone(),
            service_version_id: service.version_id.clone(),
            service_name: service.name.clone(),
            model_id: model.id.clone(),
            model_name: model.name.clone(),
            requested_alias: alias.into(),
            preferred_model_id: Some(model.id.clone()),
            preferred_model_name: Some(model.name.clone()),
            attempt: 1,
            fallback_reason: None,
            budget: None,
            mode: mode.into(),
            capabilities: required,
        };
        Ok((service, model, selection))
    }
}

impl Store {
    pub fn model_services(&self) -> Result<Vec<ModelService>, ConfigError> {
        let mut s = self
            .connection
            .prepare("SELECT current FROM model_services WHERE deleted=0 ORDER BY rowid")
            .map_err(error)?;
        let ids = s
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(error)?;
        ids.iter().map(|id| version(&self.connection, id)).collect()
    }
    pub fn model_service_version(&self, id: &str) -> Result<ModelService, ConfigError> {
        version(&self.connection, id)
    }
    pub fn model_service(&self, id: &str) -> Result<ModelService, ConfigError> {
        current(&self.connection, id)
    }
    pub fn save_model_service(&self, draft: ServiceDraft) -> Result<ModelService, ConfigError> {
        self.save_model_service_as(draft, None)
    }
    pub fn save_model_service_as(
        &self,
        draft: ServiceDraft,
        creator: Option<&str>,
    ) -> Result<ModelService, ConfigError> {
        let tx = rusqlite::Transaction::new_unchecked(
            &self.connection,
            rusqlite::TransactionBehavior::Immediate,
        )
        .map_err(error)?;
        let service = self.write_model_service(&tx, draft, creator)?;
        tx.commit().map_err(error)?;
        Ok(service)
    }
    fn write_model_service(
        &self,
        tx: &Connection,
        mut draft: ServiceDraft,
        creator: Option<&str>,
    ) -> Result<ModelService, ConfigError> {
        if draft.revision >= operations::MAX_REVISION {
            return fail("invalid_input", "模型服务版本无效");
        }
        uuid::Uuid::parse_str(&draft.id)
            .map_err(|_| ConfigError::new("invalid_input", "模型服务标识无效"))?;
        draft.name = draft.name.trim().into();
        if draft.name.is_empty()
            || draft.name.chars().count() > 80
            || draft.name.chars().any(char::is_control)
            || !(1..=32).contains(&draft.models.len())
        {
            return fail("invalid_input", "请输入 1–80 字的名称，并添加 1–32 个模型");
        }
        let providers = self.providers().map_err(error)?;
        let mut ids = HashSet::new();
        let mut models = vec![];
        for mut m in draft.models {
            uuid::Uuid::parse_str(&m.id)
                .map_err(|_| ConfigError::new("invalid_input", "模型条目标识无效"))?;
            m.name = m.name.trim().into();
            if !ids.insert(m.id.clone())
                || m.name.is_empty()
                || m.name.chars().count() > 80
                || m.name.chars().any(char::is_control)
            {
                return fail("invalid_input", "模型名称无效或条目标识重复");
            }
            m.capabilities.validate()?;
            let source = providers
                .iter()
                .find(|p| p.id == m.provider_id && p.configured)
                .ok_or_else(|| ConfigError::new("invalid_provider", "请选择已配置的 Provider"))?;
            let provider = select_model(source, Some(&m.model))?;
            validate_source(&provider)?;
            models.push(ServiceModel {
                id: m.id,
                name: m.name,
                provider,
                capabilities: m.capabilities,
            });
        }
        if !ids.contains(&draft.default_model_id) {
            return fail("invalid_model", "默认模型必须属于当前服务");
        }
        draft
            .routing_policy
            .validate(&ids, &draft.default_model_id)?;
        let old: Option<(u32, bool)> = tx
            .query_row(
                "SELECT revision,deleted FROM model_services WHERE id=?1",
                [&draft.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(error)?;
        if old.is_some_and(|(_, deleted)| deleted)
            || old.map_or(0, |(r, _)| u64::from(r)) != draft.revision
        {
            return fail("conflict", "模型服务已被修改或删除，请重新读取后保存");
        }
        let count: i64 = tx
            .query_row("SELECT count(*) FROM model_service_versions", [], |r| {
                r.get(0)
            })
            .map_err(error)?;
        let services: i64 = tx
            .query_row(
                "SELECT count(*) FROM model_services WHERE deleted=0",
                [],
                |r| r.get(0),
            )
            .map_err(error)?;
        if count >= 4096 || (old.is_none() && services >= 64) {
            return fail("capacity", "模型服务或历史版本已达上限；已有引用保持");
        }
        let service = ModelService {
            id: draft.id,
            version_id: uuid::Uuid::new_v4().to_string(),
            revision: draft.revision + 1,
            name: draft.name,
            default_model_id: draft.default_model_id,
            models,
            routing_policy: draft.routing_policy,
            published_at: chrono::Utc::now().to_rfc3339(),
        };
        tx.execute("INSERT INTO model_services(id,revision,current) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,current=excluded.current",params![service.id,i64::try_from(service.revision).map_err(error)?,service.version_id]).map_err(error)?;
        if old.is_none() {
            crate::iam::model_created(tx, &service.id, creator).map_err(error)?;
        }
        tx.execute(
            "INSERT INTO model_service_versions VALUES(?1,?2,?3)",
            params![
                service.version_id,
                service.id,
                serde_json::to_string(&service).map_err(error)?
            ],
        )
        .map_err(error)?;
        Ok(service)
    }
    pub fn delete_model_service(
        &self,
        id: &str,
        revision: u64,
        active_bindings: &[String],
    ) -> Result<(), ConfigError> {
        let tx = rusqlite::Transaction::new_unchecked(
            &self.connection,
            rusqlite::TransactionBehavior::Immediate,
        )
        .map_err(error)?;
        self.remove_model_service(&tx, id, revision, active_bindings)?;
        tx.commit().map_err(error)?;
        Ok(())
    }
    fn remove_model_service(
        &self,
        tx: &Connection,
        id: &str,
        revision: u64,
        active_bindings: &[String],
    ) -> Result<(), ConfigError> {
        if revision >= operations::MAX_REVISION {
            return fail("invalid_input", "模型服务版本无效");
        }
        let service = current(tx, id)?;
        if service.revision != revision {
            return fail("conflict", "模型服务已改变，请重新读取");
        }
        for binding in active_bindings {
            if attachment(tx, binding)?.is_some_and(|a| a.service_id == id) {
                return fail(
                    "in_use",
                    "此服务仍被 Agent 目录引用，请先移除相关目录或恢复接入前配置",
                );
            }
        }
        tx.execute(
            "UPDATE model_services SET deleted=1,revision=revision+1 WHERE id=?1",
            [id],
        )
        .map_err(error)?;
        crate::iam::model_deleted(tx, id).map_err(error)?;
        Ok(())
    }
    pub fn attach_model_service(
        &self,
        binding: &str,
        service: &ModelService,
    ) -> Result<ServiceAttachment, ConfigError> {
        let tx = rusqlite::Transaction::new_unchecked(
            &self.connection,
            rusqlite::TransactionBehavior::Immediate,
        )
        .map_err(error)?;
        let attachment = attach_binding(&tx, binding, service)?;
        tx.commit().map_err(error)?;
        Ok(attachment)
    }
    pub fn model_service_attachment(
        &self,
        binding: &str,
    ) -> Result<Option<ServiceAttachment>, ConfigError> {
        attachment(&self.connection, binding)
    }
    pub fn model_service_binding_ids(&self) -> Result<Vec<String>, ConfigError> {
        let mut s = self
            .connection
            .prepare("SELECT binding FROM model_service_bindings ORDER BY rowid")
            .map_err(error)?;
        s.query_map([], |r| r.get::<_, String>(0))
            .map_err(error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(error)
    }
    pub fn model_service_attachments(&self) -> Result<Vec<ServiceAttachment>, ConfigError> {
        self.model_service_binding_ids()?
            .iter()
            .map(|id| attachment(&self.connection, id)?.ok_or_else(|| error("missing")))
            .collect()
    }
}

#[cfg(test)]
mod tests;
