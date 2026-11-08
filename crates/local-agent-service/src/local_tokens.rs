//! Local token recovery stays separate from upstream credential adoption.
use crate::{ApiResult, AppState, json_value, providers};
use axum::{Json, extract::State};
use local_agent_core::{
    Provider, Store,
    providers::{
        BindingDraft, ConfigError, ProviderManager, Routing, Transaction,
        credential_scope::IdentityV3,
        fingerprint,
        local_tokens::{Operation, Purpose, ServiceIdentity, Session, Status},
        select_model,
    },
};
use serde_json::json;

fn unknown() -> ConfigError {
    ConfigError::new(
        "local_token_result_unknown",
        "原本机入口结果尚未确认，请保留原编号核查；未重新生成令牌",
    )
}
fn pending(error: ConfigError) -> ConfigError {
    ConfigError::new(
        "local_token_result_unknown",
        &format!("{}；原本机令牌操作已保留，请按原编号核查", error.message),
    )
}
fn require_uncancelled(store: &Store, id: &IdentityV3) -> Result<(), ConfigError> {
    if store.local_token_cancellation(id)?.is_some() {
        return Err(ConfigError::new(
            "local_token_cancelled",
            "原编号已阻止后续创建，已有本机入口材料保留",
        ));
    }
    Ok(())
}

fn identity(manager: &ProviderManager, store: &Store, id: &IdentityV3) -> Result<(), ConfigError> {
    let protocol = manager.local_token_protocol(store)?;
    if protocol.store_id != id.store_id {
        return Err(ConfigError::new(
            "credential_store_changed",
            "请连接原数据库核查本机入口",
        ));
    }
    if protocol.namespace_id != id.namespace_id {
        return Err(ConfigError::new(
            "credential_namespace_changed",
            "请连接原凭据恢复空间核查本机入口",
        ));
    }
    if !matches!(
        protocol.namespace_state,
        local_agent_core::providers::credential_scope::NamespaceState::Active
    ) {
        return Err(ConfigError::new(
            "credential_namespace_unverified",
            "请先核查原凭据恢复空间，再创建或继续本机入口",
        ));
    }
    if !local_agent_core::providers::operations::valid_id(&id.operation_id) {
        return Err(ConfigError::new(
            "invalid_local_token",
            "本机入口原操作编号无效",
        ));
    }
    Ok(())
}
fn preview(manager: &ProviderManager, id: &IdentityV3) -> Result<Option<Transaction>, ConfigError> {
    let matching: Vec<_> = manager
        .status()?
        .transactions
        .into_iter()
        .filter(|t| t.local_token_operation.as_ref() == Some(id) && t.restore_of.is_none())
        .collect();
    if matching.len() > 1 {
        return Err(unknown());
    }
    Ok(matching.into_iter().next())
}
fn matches_draft(draft: &BindingDraft, t: &Transaction) -> bool {
    t.agent_id == draft.agent_id
        && t.before.revision.as_deref().unwrap_or("absent") == draft.expected_revision
        && t.provider.as_ref().map(|p| p.id.as_str()) == draft.provider_id.as_deref()
        && draft
            .expected_provider_revision
            .is_none_or(|r| t.provider.as_ref().is_some_and(|p| p.revision == r))
        && draft
            .model
            .as_ref()
            .is_none_or(|m| t.model.as_ref() == Some(m))
}
fn matches_operation(operation: &Operation, t: &Transaction) -> bool {
    let request = &operation.request;
    t.local_token_operation.as_ref() == Some(&request.identity)
        && t.agent_id == request.agent_id
        && t.before.path == request.configuration_path
        && t.before.revision.as_deref().unwrap_or("absent") == request.expected_revision
        && t.routing
            .as_ref()
            .is_some_and(|r| r.id == operation.owner_id)
        && t.provider.as_ref().is_some_and(|p| {
            select_model(p, Some(&request.provider.model))
                .is_ok_and(|p| fingerprint(&p) == fingerprint(&request.provider))
        })
}
pub(crate) fn replay_draft(
    manager: &ProviderManager,
    store: &Store,
    draft: &BindingDraft,
    purpose: Purpose,
) -> Result<Option<Transaction>, ConfigError> {
    let Some(id) = &draft.operation else {
        return Ok(None);
    };
    identity(manager, store, id)?;
    require_uncancelled(store, id)?;
    let record = store.local_token_record(id)?;
    if let Some(operation) = &record {
        let request = &operation.request;
        if request.purpose != purpose
            || request.agent_id != draft.agent_id
            || request.expected_revision != draft.expected_revision
            || draft.provider_id.as_deref() != Some(request.provider.id.as_str())
            || draft
                .expected_provider_revision
                .is_some_and(|r| r != request.provider.revision)
            || draft
                .model
                .as_ref()
                .is_some_and(|m| *m != request.provider.model)
        {
            return Err(ConfigError::new(
                "operation_conflict",
                "原本机令牌编号已绑定其他用途或输入",
            ));
        }
        if operation.status == Status::Cancelled {
            return Err(ConfigError::new(
                "local_token_cancelled",
                "原本机入口创建已停止",
            ));
        }
    }
    if let Some(t) = preview(manager, id)? {
        if !matches_draft(draft, &t)
            || record.as_ref().is_some_and(|o| !matches_operation(o, &t))
            || record.is_none() && purpose != Purpose::FixedRoute
        {
            return Err(unknown());
        }
        if record.as_ref().is_some_and(|o| o.status == Status::Pending) {
            manager
                .resume_local_token(store, id)?
                .finish_preview(&t.id)?;
        }
        return Ok(Some(t));
    }
    if record.is_some() {
        return Err(unknown());
    }
    Ok(None)
}
pub(crate) fn available(manager: &ProviderManager, agent: &str) -> Result<(), ConfigError> {
    let status = manager.status()?;
    if status
        .configs
        .iter()
        .find(|c| c.agent_id == agent)
        .is_none_or(|c| c.error.is_some())
    {
        return Err(ConfigError::new(
            "invalid_config",
            "原生配置不可核查，请修复原文件后重新预览",
        ));
    }
    if status
        .transactions
        .iter()
        .any(|t| t.agent_id == agent && t.state == "pending")
    {
        return Err(ConfigError::new(
            "conflict",
            "该 Agent 已有待应用事务，请先应用或撤销",
        ));
    }
    Ok(())
}
pub(crate) fn replay_service(
    manager: &ProviderManager,
    store: &Store,
    input: &crate::model_services::Connect,
) -> Result<Option<Transaction>, ConfigError> {
    let Some(id) = &input.operation else {
        return Ok(None);
    };
    identity(manager, store, id)?;
    require_uncancelled(store, id)?;
    let Some(record) = store.local_token_record(id)? else {
        return Ok(None);
    };
    let request = &record.request;
    if request.purpose != Purpose::ModelService
        || request.agent_id != input.agent_id
        || request.expected_revision != input.expected_revision
        || request.service.as_ref().is_none_or(|s| {
            s.id != input.service_id || s.revision != input.expected_service_revision
        })
    {
        return Err(ConfigError::new(
            "operation_conflict",
            "原本机令牌编号已绑定其他模型服务或输入",
        ));
    }
    let draft = BindingDraft {
        operation: Some(id.clone()),
        agent_id: input.agent_id.clone(),
        expected_revision: input.expected_revision.clone(),
        provider_id: Some(request.provider.id.clone()),
        expected_provider_revision: Some(request.provider.revision),
        model: Some(request.provider.model.clone()),
    };
    replay_draft(manager, store, &draft, Purpose::ModelService)
}
fn verified(
    manager: &ProviderManager,
    store: &Store,
    session: &Session<'_>,
) -> Result<(), ConfigError> {
    let request = &session.operation().request;
    session.validate_source()?;
    providers::require_supported(&request.agent_id)?;
    if let Some(service) = &request.service {
        let current = store.model_service(&service.id)?;
        crate::model_services::verify_snapshot(store, manager, &current)?;
    } else if request.purpose == Purpose::FixedRoute
        && ["codex", "opencode", "claude-desktop"].contains(&request.agent_id.as_str())
    {
        manager
            .require_model_verifications(&request.provider, request.provider.enabled_model_ids())?;
    } else {
        manager.require_verification(&request.provider)?;
    }
    Ok(())
}
pub(crate) fn execute(
    state: &AppState,
    manager: &ProviderManager,
    store: &Store,
    session: &mut Session<'_>,
) -> Result<Transaction, ConfigError> {
    let operation = session.operation().clone();
    let request = &operation.request;
    if operation.status == Status::Cancelled {
        return Err(ConfigError::new(
            "local_token_cancelled",
            "原本机入口创建已停止",
        ));
    }
    if let Some(t) = preview(manager, &request.identity)? {
        if !matches_operation(&operation, &t) {
            return Err(unknown());
        }
        session.finish_preview(&t.id)?;
        return Ok(t);
    }
    if operation.status == Status::Committed {
        return Err(unknown());
    }
    verified(manager, store, session)?;
    available(manager, &request.agent_id)?;
    let source = store
        .providers()
        .map_err(|_| unknown())?
        .into_iter()
        .find(|p| p.id == request.provider.id)
        .ok_or_else(unknown)?;
    let draft = BindingDraft {
        operation: Some(request.identity.clone()),
        agent_id: request.agent_id.clone(),
        expected_revision: request.expected_revision.clone(),
        provider_id: Some(source.id.clone()),
        expected_provider_revision: Some(source.revision),
        model: Some(request.provider.model.clone()),
    };
    let result = (|| {
        let (rendered, route, attachment) = if request.purpose == Purpose::FixedRoute {
            let (rendered, route) = state.gateway.prepare_owned_route(session)?;
            (rendered, route, None)
        } else {
            session.ensure_credential()?;
            let binding = store.create_owned_local_binding(session.operation())?;
            session.mark_owner_saved()?;
            let rendered = crate::gateway::dynamic::client(state, &binding);
            let route = Routing {
                id: binding.id.clone(),
                client_protocol: rendered.protocol.clone(),
                upstream_protocol: binding.target.provider.protocol.clone(),
                local_endpoint: rendered.endpoint.clone(),
                models: vec![],
            };
            let mut attachment = store.model_service_attachment(&binding.id)?;
            if let Some(a) = attachment.as_mut() {
                let config = manager
                    .inspect_configs()
                    .into_iter()
                    .find(|c| c.agent_id == request.agent_id)
                    .ok_or_else(unknown)?;
                for old in store.model_service_attachments()?.into_iter().rev() {
                    if old.binding_id == binding.id || old.service_id != a.service_id {
                        continue;
                    }
                    if let Some(old_binding) = store.dynamic_binding(&old.binding_id)?
                        && old_binding.agent_id == request.agent_id
                        && old_binding.scope == request.configuration_path
                    {
                        let endpoint =
                            crate::gateway::dynamic::configured_endpoint(state, &old_binding);
                        if request.agent_id == "codex" {
                            a.previous_codex_endpoints.push(endpoint.clone());
                        }
                        if config.endpoint.as_deref() == Some(endpoint.as_str()) {
                            a.previous_endpoint = Some(endpoint);
                            if request.agent_id != "codex" {
                                break;
                            }
                        }
                    }
                }
            }
            (rendered, route, attachment)
        };
        let t = match attachment {
            Some(attachment) => {
                let codex_catalog = (draft.agent_id == "codex")
                    .then(|| crate::gateway::codex_catalog::service_catalog_bytes(&attachment));
                manager.prepare_model_service(
                    draft,
                    &source,
                    &rendered,
                    route,
                    &attachment,
                    codex_catalog.as_deref(),
                )?
            }
            None => manager.prepare_routed(draft, Some(&source), Some((&rendered, route)))?,
        };
        if !matches_operation(session.operation(), &t) {
            return Err(unknown());
        }
        session.finish_preview(&t.id)?;
        Ok(t)
    })();
    result.map_err(pending)
}
pub(crate) fn fixed(
    state: &AppState,
    manager: &ProviderManager,
    store: &Store,
    draft: BindingDraft,
    source: &Provider,
    selected: &Provider,
) -> Result<Transaction, ConfigError> {
    available(manager, &draft.agent_id)?;
    if let Some((rendered, route)) =
        state
            .gateway
            .existing_prepared(store, &draft.agent_id, selected)?
    {
        let _guard = draft
            .operation
            .as_ref()
            .map(|id| manager.local_token_admission_guard(store, id))
            .transpose()?;
        return manager.prepare_routed(draft, Some(source), Some((&rendered, route)));
    }
    let request =
        manager.new_local_token_request(store, &draft, Purpose::FixedRoute, selected, None)?;
    let mut session = manager.begin_local_token(store, request)?;
    execute(state, manager, store, &mut session)
}
pub(crate) fn dynamic(
    state: &AppState,
    manager: &ProviderManager,
    store: &Store,
    draft: &BindingDraft,
    selected: &Provider,
) -> Result<Transaction, ConfigError> {
    available(manager, &draft.agent_id)?;
    let request =
        manager.new_local_token_request(store, draft, Purpose::DynamicBinding, selected, None)?;
    let mut session = manager.begin_local_token(store, request)?;
    execute(state, manager, store, &mut session)
}
pub(crate) fn model_service(
    state: &AppState,
    manager: &ProviderManager,
    store: &Store,
    draft: &BindingDraft,
    service: &local_agent_core::model_services::ModelService,
) -> Result<Transaction, ConfigError> {
    available(manager, &draft.agent_id)?;
    let model = service
        .models
        .iter()
        .find(|m| m.id == service.default_model_id)
        .ok_or_else(unknown)?;
    let request = manager.new_local_token_request(
        store,
        draft,
        Purpose::ModelService,
        &model.provider,
        Some(ServiceIdentity {
            id: service.id.clone(),
            version_id: service.version_id.clone(),
            revision: service.revision,
        }),
    )?;
    let mut session = manager.begin_local_token(store, request)?;
    execute(state, manager, store, &mut session)
}

pub async fn protocol(State(state): State<AppState>) -> ApiResult {
    crate::blocking::run(move || {
        let manager = state
            .providers
            .lock()
            .map_err(|_| providers::error(unknown()))?;
        let store = state
            .store
            .lock()
            .map_err(|_| providers::error(unknown()))?;
        json_value(
            manager
                .local_token_protocol(&store)
                .map_err(providers::error)?,
        )
    })
    .await
    .map_err(|e| providers::error(crate::blocking::config_error(e)))?
}
pub async fn query(State(state): State<AppState>, Json(id): Json<IdentityV3>) -> ApiResult {
    crate::blocking::run(move|| {
        let manager=state.providers.lock().map_err(|_|providers::error(unknown()))?;let store=state.store.lock().map_err(|_|providers::error(unknown()))?;
        identity(&manager,&store,&id).map_err(providers::error)?;
        if let Some(cancellation)=store.local_token_cancellation(&id).map_err(providers::error)? {
            if preview(&manager,&id).map_err(providers::error)?.is_some() {return Err(providers::error(unknown()));}
            return Ok(Json(json!({"version":2,"identity":id,"operation":null,"credential":null,"preview":null,"cancellation":cancellation,"canContinue":false,"sourceMessage":"原编号已阻止后续创建，已有本机入口材料继续保留；历史配置如有变化需另行核查"})));
        }
        let record=store.local_token_record(&id).map_err(providers::error)?;
        if record.is_none() {
            let found=preview(&manager,&id).map_err(providers::error)?.ok_or_else(||providers::error(unknown()))?;
            return Ok(Json(json!({"version":2,"identity":id,"operation":null,"credential":null,"preview":found,"cancellation":null,"canContinue":false,"sourceMessage":"原预览已查到；未创建或重发入口"})));
        }
        let mut session=manager.resume_local_token(&store,&id).map_err(providers::error)?;
        let found=preview(&manager,&id).map_err(providers::error)?;
        if let Some(t)=&found {
            if !matches_operation(session.operation(),t) {return Err(providers::error(unknown()));}
            if session.operation().status==Status::Pending {session.finish_preview(&t.id).map_err(providers::error)?;}
        }
        let credential=session.check_credential().map_err(providers::error)?;
        let validation=verified(&manager,&store,&session);
        let can_continue=found.is_none() && session.operation().status==Status::Pending && credential.as_ref().is_none_or(|c|matches!(c.status,local_agent_core::providers::credential_operations::Status::Pending|local_agent_core::providers::credential_operations::Status::Ready)) && validation.is_ok();
        let message=validation.err().map(|e|e.message).unwrap_or_else(||"原配置与供应商已核对；继续仍会在执行前再次检查".into());
        Ok(Json(json!({"version":2,"identity":id,"operation":session.operation(),"credential":credential,"preview":found,"cancellation":null,"canContinue":can_continue,"sourceMessage":message})))
    }).await.map_err(|e|providers::error(crate::blocking::config_error(e)))?
}
pub async fn resume(State(state): State<AppState>, Json(id): Json<IdentityV3>) -> ApiResult {
    crate::blocking::run(move || {
        let manager = state
            .providers
            .lock()
            .map_err(|_| providers::error(unknown()))?;
        let store = state
            .store
            .lock()
            .map_err(|_| providers::error(unknown()))?;
        identity(&manager, &store, &id).map_err(providers::error)?;
        require_uncancelled(&store, &id).map_err(providers::error)?;
        if store
            .local_token_record(&id)
            .map_err(providers::error)?
            .is_none()
        {
            let found = preview(&manager, &id)
                .map_err(providers::error)?
                .ok_or_else(|| providers::error(unknown()))?;
            return Ok(Json(
                json!({"version":2,"identity":id,"operation":null,"preview":found,"cancellation":null}),
            ));
        }
        let mut session = manager
            .resume_local_token(&store, &id)
            .map_err(providers::error)?;
        let t = execute(&state, &manager, &store, &mut session).map_err(providers::error)?;
        Ok(Json(
            json!({"version":2,"identity":id,"operation":session.operation(),"preview":t,"cancellation":null}),
        ))
    })
    .await
    .map_err(|e| providers::error(crate::blocking::config_error(e)))?
}
pub async fn cancel(State(state): State<AppState>, Json(id): Json<IdentityV3>) -> ApiResult {
    crate::blocking::run(move || {
        let manager = state
            .providers
            .lock()
            .map_err(|_| providers::error(unknown()))?;
        let store = state
            .store
            .lock()
            .map_err(|_| providers::error(unknown()))?;
        identity(&manager, &store, &id).map_err(providers::error)?;
        let record = store.local_token_record(&id).map_err(providers::error)?;
        let mut session = record
            .as_ref()
            .map(|_| manager.resume_local_token(&store, &id))
            .transpose()
            .map_err(providers::error)?;
        let mut found = preview(&manager, &id).map_err(providers::error)?;
        if let (Some(session), Some(t)) = (&session, &found)
            && !matches_operation(session.operation(), t)
        {
            return Err(providers::error(unknown()));
        }
        if let Some(t) = &found
            && t.state == "pending"
        {
            found = Some(manager.cancel(&t.id).map_err(providers::error)?);
        }
        if record.is_none() {
            if found.is_none() {
                let cancellation=manager.cancel_unowned_local_token(&store,&id).map_err(providers::error)?;
                return Ok(Json(json!({"version":2,"identity":id,"operation":null,"preview":null,"cancellation":cancellation})));
            }
            return Ok(Json(
                json!({"version":2,"identity":id,"operation":null,"preview":found,"cancellation":null}),
            ));
        }
        let session = session
            .as_mut()
            .ok_or_else(|| providers::error(unknown()))?;
        if let Some(t) = &found {
            if !matches_operation(session.operation(), t) {
                return Err(providers::error(unknown()));
            }
            if session.operation().status == Status::Pending {
                session.finish_preview(&t.id).map_err(providers::error)?;
            }
        } else {
            session.cancel().map_err(providers::error)?;
        }
        Ok(Json(
            json!({"version":2,"identity":id,"operation":session.operation(),"preview":found,"cancellation":null}),
        ))
    })
    .await
    .map_err(|e| providers::error(crate::blocking::config_error(e)))?
}
