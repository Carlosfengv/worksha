use crate::{ApiResult, AppState, json_value, providers};
use axum::{
    Json,
    extract::{Path, Query, State},
};
use local_agent_core::{
    model_services::{
        CONNECTION_AGENTS, ModelService, ServiceDraft,
        operations::{OperationAction, OperationRequest, RECEIPT_LIMIT},
    },
    providers::{BindingDraft, ConfigError, fingerprint, select_model},
};
use serde::Deserialize;
use serde_json::{Value, json};

fn unavailable() -> (axum::http::StatusCode, Json<Value>) {
    providers::error(ConfigError::new("service_unavailable", "模型服务暂不可用"))
}

pub(crate) fn verify_snapshot(
    store: &local_agent_core::Store,
    manager: &local_agent_core::providers::ProviderManager,
    service: &ModelService,
) -> Result<(), ConfigError> {
    let sources = store
        .providers()
        .map_err(|_| ConfigError::new("service_unavailable", "Provider 不可读取"))?;
    for model in &service.models {
        let source = sources
            .iter()
            .find(|p| p.id == model.provider.id)
            .ok_or_else(|| ConfigError::new("conflict", "模型服务引用的 Provider 已移除"))?;
        if fingerprint(&select_model(source, Some(&model.provider.model))?)
            != fingerprint(&model.provider)
        {
            return Err(ConfigError::new(
                "conflict",
                "Provider 已改变，请重新验证并发布模型服务",
            ));
        }
        manager.require_verification(&model.provider)?;
    }
    Ok(())
}
pub async fn list(State(state): State<AppState>) -> ApiResult {
    crate::blocking::run(move||{
        let m=state.providers.lock().map_err(|_|unavailable())?;
        let configs=m.inspect_configs();
        let store=state.store.lock().map_err(|_|unavailable())?;
        let services=store.model_services().map_err(providers::error)?;
        let bindings=store.dynamic_bindings().map_err(providers::error)?;
        let attachments=store.model_service_attachments().map_err(providers::error)?.into_iter().map(|a|{
            let binding=bindings.iter().find(|b|b.id==a.binding_id);
            let endpoint=binding.map(|b|crate::gateway::dynamic::configured_endpoint(&state,b));
            let attached=binding.is_some_and(|b|configs.iter().any(|c|c.agent_id==b.agent_id && c.path==b.scope && c.endpoint==endpoint && c.error.is_none()));
            let referenced=binding.is_some_and(|b|configs.iter().any(|c|c.agent_id==b.agent_id && c.path==b.scope && c.error.is_none() && (c.endpoint==endpoint || endpoint.as_ref().is_some_and(|e|c.catalog_endpoints.contains(e)))));
            let configuration_unknown=binding.is_none_or(|b|configs.iter().find(|c|c.agent_id==b.agent_id && c.path==b.scope).is_none_or(|c|c.error.is_some()));
            let current=services.iter().find(|s|s.id==a.service_id);
            let directory_sync_required=current.is_none_or(|s|{
                let a_ids=a.installed.models.iter().map(|m|(&m.id,&m.name,&m.capabilities)).collect::<Vec<_>>();
                let b_ids=s.models.iter().map(|m|(&m.id,&m.name,&m.capabilities)).collect::<Vec<_>>();
                a_ids!=b_ids || a.installed.name!=s.name
            });
            json!({"bindingId":a.binding_id,"serviceId":a.service_id,"installed":a.installed,"agentId":binding.map(|b|&b.agent_id),"localEndpoint":endpoint,"attached":attached,"referenced":referenced,"configurationUnknown":configuration_unknown,"directorySyncRequired":directory_sync_required})
        }).collect::<Vec<_>>();
        let health=services.iter().flat_map(|s|s.models.iter().map(|m|json!({"serviceId":s.id,"modelId":m.id,"status":crate::observation::policy::health(&store,&m.provider)}))).collect::<Vec<_>>();
        Ok(Json(json!({"services":services,"attachments":attachments,"connectionAgents":CONNECTION_AGENTS,"routingHealth":health,"writeProtocol":{"version":1,"receiptLimit":RECEIPT_LIMIT,"storeId":store.model_service_operation_store_id().map_err(providers::error)?}})))
    }).await.map_err(|e|providers::error(crate::blocking::config_error(e)))?
}
pub async fn save(State(state): State<AppState>, Json(draft): Json<ServiceDraft>) -> ApiResult {
    crate::blocking::run(move || {
        let m = state.providers.lock().map_err(|_| unavailable())?;
        let store = state.store.lock().map_err(|_| unavailable())?;
        policy_publication_guard(&state, &store, &draft.id, None)?;
        validate_draft(&store, &m, &draft).map_err(providers::error)?;
        json_value(store.save_model_service(draft).map_err(providers::error)?)
    })
    .await
    .map_err(|e| providers::error(crate::blocking::config_error(e)))?
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Revision {
    pub expected_revision: u64,
}
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(input): Json<Revision>,
) -> ApiResult {
    crate::blocking::run(move || {
        let m = state.providers.lock().map_err(|_| unavailable())?;
        let store = state.store.lock().map_err(|_| unavailable())?;
        policy_publication_guard(&state, &store, &id, None)?;
        let active = active_bindings(&state, &store, &m).map_err(providers::error)?;
        store
            .delete_model_service(&id, input.expected_revision, &active)
            .map_err(providers::error)?;
        Ok(Json(json!({"deleted":true})))
    })
    .await
    .map_err(|e| providers::error(crate::blocking::config_error(e)))?
}
fn validate_draft(
    store: &local_agent_core::Store,
    manager: &local_agent_core::providers::ProviderManager,
    draft: &ServiceDraft,
) -> Result<(), ConfigError> {
    let sources = store
        .providers()
        .map_err(|_| ConfigError::new("service_unavailable", "Provider 不可读取"))?;
    let previous = match store.model_service(&draft.id) {
        Ok(service) => Some(service),
        Err(error) if error.code == "not_found" => None,
        Err(error) => return Err(error),
    };
    for model in &draft.models {
        let source = sources
            .iter()
            .find(|p| p.id == model.provider_id)
            .ok_or_else(|| ConfigError::new("invalid_provider", "请选择已保存 Provider"))?;
        let selected = select_model(source, Some(&model.model))?;
        let unchanged = previous.as_ref().is_some_and(|s| {
            s.models.iter().any(|m| {
                m.id == model.id
                    && m.capabilities == model.capabilities
                    && fingerprint(&m.provider) == fingerprint(&selected)
            })
        });
        if !unchanged {
            manager.require_verification(&selected)?;
        }
    }
    Ok(())
}
fn active_bindings(
    state: &AppState,
    store: &local_agent_core::Store,
    manager: &local_agent_core::providers::ProviderManager,
) -> Result<Vec<String>, ConfigError> {
    let configs = manager.inspect_configs();
    Ok(store
        .dynamic_bindings()?
        .into_iter()
        .filter(|b| {
            configs.iter().any(|c| {
                c.agent_id == b.agent_id
                    && c.path == b.scope
                    && (c.error.is_some()
                        || c.catalog_endpoints
                            .contains(&crate::gateway::dynamic::configured_endpoint(state, b))
                        || c.endpoint.as_deref()
                            == Some(
                                crate::gateway::dynamic::configured_endpoint(state, b).as_str(),
                            ))
            })
        })
        .map(|b| b.id)
        .collect())
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationScope {
    store_id: String,
}
pub async fn operation(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(scope): Query<OperationScope>,
) -> ApiResult {
    crate::blocking::run(move || {
        let store = state.store.lock().map_err(|_| unavailable())?;
        json_value(
            store
                .model_service_operation(&id, &scope.store_id)
                .map_err(providers::error)?,
        )
    })
    .await
    .map_err(|e| providers::error(crate::blocking::config_error(e)))?
}
pub async fn commit_operation(
    State(state): State<AppState>,
    Json(request): Json<OperationRequest>,
) -> ApiResult {
    crate::blocking::run(move || {
        let manager = state.providers.lock().map_err(|_| unavailable())?;
        let store = state.store.lock().map_err(|_| unavailable())?;
        // The core transaction checks a prior receipt before this validation;
        // a retry cannot re-verify or re-publish an already committed operation.
        let receipt = store
            .commit_model_service_operation(&request, |action| {
                policy_publication_guard(&state, &store, action.identity().1, Some(&request))
                    .map_err(|_| {
                        ConfigError::new("result_pending", "策略发布结果尚未确认，请核查原操作编号")
                    })?;
                match action {
                    OperationAction::Save { draft } => {
                        validate_draft(&store, &manager, draft)?;
                        Ok(vec![])
                    }
                    OperationAction::Delete { .. } => active_bindings(&state, &store, &manager),
                }
            })
            .map_err(providers::error)?;
        json_value(receipt)
    })
    .await
    .map_err(|e| providers::error(crate::blocking::config_error(e)))?
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Connect {
    #[serde(default)]
    pub operation: Option<local_agent_core::providers::credential_scope::IdentityV3>,
    pub agent_id: String,
    pub expected_revision: String,
    pub service_id: String,
    pub expected_service_revision: u64,
}
pub async fn connect(State(state): State<AppState>, Json(input): Json<Connect>) -> ApiResult {
    crate::blocking::run(move || {
        if !CONNECTION_AGENTS.contains(&input.agent_id.as_str()) {
            return Err(providers::error(ConfigError::new(
                "unsupported",
                "此 Agent 尚未支持模型服务接入",
            )));
        }
        providers::require_supported(&input.agent_id).map_err(providers::error)?;
        let manager = state.providers.lock().map_err(|_| unavailable())?;
        let store = state.store.lock().map_err(|_| unavailable())?;
        if let Some(t) = crate::local_tokens::replay_service(&manager, &store, &input)
            .map_err(providers::error)?
        {
            return json_value(t);
        }
        let status = manager.status().map_err(providers::error)?;
        let config = status
            .configs
            .iter()
            .find(|c| c.agent_id == input.agent_id)
            .ok_or_else(unavailable)?;
        if config.error.is_some() || config.revision.as_deref() != Some(&input.expected_revision) {
            return Err(providers::error(ConfigError::new(
                "conflict",
                "Agent 配置已改变，请重新读取",
            )));
        }
        let service = store
            .model_service(&input.service_id)
            .map_err(providers::error)?;
        if service.revision != input.expected_service_revision {
            return Err(providers::error(ConfigError::new(
                "conflict",
                "模型服务已改变，请重新读取",
            )));
        }
        if let Some(pending) = status
            .transactions
            .iter()
            .find(|t| t.agent_id == input.agent_id && t.state == "pending")
        {
            let same_preview = pending
                .routing
                .as_ref()
                .map(|route| store.model_service_attachment(&route.id))
                .transpose()
                .map_err(providers::error)?
                .flatten()
                .is_some_and(|a| {
                    a.service_id == service.id && a.installed.version_id == service.version_id
                })
                && pending.before.revision.as_deref() == Some(input.expected_revision.as_str())
                && pending.before.path == config.path
                && pending.restore_of.is_none();
            if same_preview && input.operation.is_none() {
                return json_value(pending);
            }
            return Err(providers::error(ConfigError::new(
                "conflict",
                "已有未完成的配置切换，请继续处理或撤销现有预览后再选择服务",
            )));
        }
        verify_snapshot(&store, &manager, &service).map_err(providers::error)?;
        let model = service
            .models
            .iter()
            .find(|m| m.id == service.default_model_id)
            .ok_or_else(unavailable)?;
        let source = store
            .providers()
            .map_err(crate::error)?
            .into_iter()
            .find(|p| p.id == model.provider.id)
            .ok_or_else(unavailable)?;
        let draft = BindingDraft {
            operation: input.operation,
            agent_id: input.agent_id,
            expected_revision: input.expected_revision,
            provider_id: Some(source.id.clone()),
            expected_provider_revision: Some(source.revision),
            model: Some(model.provider.model.clone()),
        };
        json_value(
            crate::local_tokens::model_service(&state, &manager, &store, &draft, &service)
                .map_err(providers::error)?,
        )
    })
    .await
    .map_err(|e| providers::error(crate::blocking::config_error(e)))?
}

pub(crate) fn ensure_enrollment(
    store: &local_agent_core::Store,
    manager: &local_agent_core::providers::ProviderManager,
    binding: &str,
) -> Result<(), ConfigError> {
    if let Some(a) = store.model_service_attachment(binding)? {
        let service = store.model_service(&a.service_id)?;
        if service.version_id != a.installed.version_id {
            return Err(ConfigError::new(
                "conflict",
                "接入预览生成后模型服务已更新，请撤销旧预览并重新同步",
            ));
        }
        verify_snapshot(store, manager, &service)?;
    }
    Ok(())
}

use local_agent_core::model_services::operations::VerifyMemberRequest as VerifyMember;
pub async fn verify_member(
    State(state): State<AppState>,
    Json(input): Json<VerifyMember>,
) -> ApiResult {
    let copy = state.clone();
    let expected = input.store_id;
    crate::blocking::run(move || {
        let store = copy.store.lock().map_err(|_| unavailable())?;
        store
            .require_model_service_store(&expected)
            .map_err(providers::error)
    })
    .await
    .map_err(|e| providers::error(crate::blocking::config_error(e)))??;
    providers::verify(
        State(state),
        Path(input.provider_id),
        Json(providers::Verify {
            expected_revision: input.expected_revision,
            model: Some(input.model),
        }),
    )
    .await
}

fn policy_publication_guard(
    state: &AppState,
    store: &local_agent_core::Store,
    service: &str,
    operation: Option<&OperationRequest>,
) -> Result<(), (axum::http::StatusCode, Json<Value>)> {
    let id = store
        .model_service_operation_store_id()
        .map_err(providers::error)?;
    state
        .plugins
        .policy_publication_guard(&id, service, operation)
        .map_err(|e| {
            providers::error(ConfigError::new(
                e.code,
                crate::plugin_host::external::message(e.code),
            ))
        })
}
