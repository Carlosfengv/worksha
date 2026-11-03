//! Token-bound reads of edited Drafts; Saved origins only fence ownership.
use crate::{ApiResult, AppState, json_value, plugin_host::external::catalog, provider_models};
use axum::{Json, extract::State, http::StatusCode};
use local_agent_core::{
    Provider,
    domain::{
        provider_access::Target,
        provider_draft_catalog::{DraftCatalogAction, DraftCatalogRequest, DraftOrigin},
    },
    providers::ConfigError,
};
use std::time::Duration;

fn changed() -> ConfigError {
    ConfigError::new(
        "catalog_source_changed",
        "原连接或目录来源已变化，请重新核对；没有读取其他目录",
    )
}
fn unavailable() -> ConfigError {
    ConfigError::new(
        "catalog_source_unavailable",
        "草稿目录范围尚不可核查，未读取其他目录",
    )
}
fn plugin_error(e: crate::plugin_host::external::Error) -> ConfigError {
    ConfigError::new(e.code, crate::plugin_host::external::message(e.code))
}
fn models(input: &DraftCatalogRequest) -> provider_models::ModelsRequest {
    provider_models::ModelsRequest {
        endpoint: input.draft.endpoint.clone(),
        protocol: input.draft.protocol.clone(),
        vendor: input.draft.vendor.clone(),
        credential_ref: input.draft.credential_ref.clone(),
    }
}
async fn scope(
    state: AppState,
    input: DraftCatalogRequest,
) -> Result<(Vec<Provider>, Provider), ConfigError> {
    let (providers, mut provider) = match &input.origin {
        DraftOrigin::Saved {
            query,
            source_revision,
        } => {
            let loaded = super::catalog_source::load(state, query.clone()).await?;
            if loaded.source.revision != *source_revision || loaded.source.pending {
                return Err(changed());
            }
            (loaded.providers, loaded.provider)
        }
        DraftOrigin::New {
            store_id,
            provider_id,
        } => {
            let store_id = store_id.clone();
            let provider_id = provider_id.clone();
            crate::blocking::run(move || {
                let manager = state.providers.lock().map_err(|_| unavailable())?;
                let store = state.store.lock().map_err(|_| unavailable())?;
                if store.provider_write_protocol()?.store_id != store_id {
                    return Err(ConfigError::new(
                        "operation_store_changed",
                        "草稿属于原数据库，本次未读取",
                    ));
                }
                if provider_id.is_empty() || provider_id.len() > 200 {
                    return Err(changed());
                }
                let existing = store
                    .provider_by_id(&provider_id)
                    .map_err(|_| unavailable())?;
                if existing
                    .as_ref()
                    .is_some_and(|p| p.configured || p.revision != 0)
                {
                    return Err(changed());
                }
                let providers = existing.clone().into_iter().collect();
                let provider = existing.unwrap_or(Provider {
                    id: provider_id,
                    name: String::new(),
                    vendor: String::new(),
                    endpoint: String::new(),
                    protocol: String::new(),
                    model: String::new(),
                    configured: false,
                    credential_ref: None,
                    credential_label: None,
                    models: vec![],
                    disabled_models: vec![],
                    model_catalog: None,
                    revision: 0,
                    category: "custom".into(),
                });
                // Includes the existing file-loss marker guard even when no SQL row exists.
                let source = manager.catalog_source_snapshot(&store, &provider)?;
                if source.revision.is_some() || source.pending || source.catalog.is_some() {
                    return Err(changed());
                }
                Ok((providers, provider))
            })
            .await
            .map_err(crate::blocking::config_error)??
        }
    };
    provider.endpoint = input.draft.endpoint;
    provider.protocol = input.draft.protocol;
    provider.vendor = input.draft.vendor;
    provider.credential_ref = input.draft.credential_ref;
    Ok((providers, provider))
}
async fn ensure(
    state: AppState,
    input: DraftCatalogRequest,
    lease: Option<catalog::Lease>,
) -> Result<(), ConfigError> {
    let (providers, provider) = scope(state.clone(), input).await?;
    if let Some(lease) = lease {
        crate::blocking::run(move || {
            lease
                .ensure(&state.plugins, &providers, &provider)
                .map_err(plugin_error)
        })
        .await
        .map_err(crate::blocking::config_error)??;
    }
    Ok(())
}
async fn authority(
    state: AppState,
    input: &DraftCatalogRequest,
) -> Result<Option<catalog::Lease>, ConfigError> {
    let (providers, provider) = scope(state.clone(), input.clone()).await?;
    if let Some(selection) = input.source.catalog() {
        if !matches!(selection.target, Target::Draft { .. }) {
            return Err(ConfigError::new(
                "catalog_source_authorization",
                "请明确授权当前草稿；已保存权限不代替草稿授权",
            ));
        }
        let selection = selection.clone();
        Ok(Some(
            crate::blocking::run(move || {
                state
                    .plugins
                    .catalog_connection_lease(&selection, &providers, &provider)
                    .map_err(plugin_error)
            })
            .await
            .map_err(crate::blocking::config_error)??,
        ))
    } else {
        provider_models::model_url_with(&state.provider_adapters, &models(input))?;
        Ok(None)
    }
}
async fn read(
    state: AppState,
    input: DraftCatalogRequest,
    lease: Option<catalog::Lease>,
) -> Result<provider_models::Catalog, ConfigError> {
    let work = async {
        ensure(state.clone(), input.clone(), lease.clone()).await?;
        let result = if let Some(selection) = input.source.catalog() {
            catalog::read_for_connection_checked(state.clone(), selection, || async {
                ensure(state.clone(), input.clone(), lease.clone())
                    .await
                    .map_err(|_| catalog::scope_changed())
            })
            .await
            .map_err(plugin_error)
        } else {
            let _permit = state
                .verification_capacity
                .clone()
                .try_acquire_owned()
                .map_err(|_| ConfigError::new("busy", "已有目录或验证任务正在进行"))?;
            let secret = if let Some(reference) = &input.draft.credential_ref {
                Some(crate::blocking::secret(state.credentials.clone(), reference.clone()).await?)
            } else {
                None
            };
            ensure(state.clone(), input.clone(), lease.clone()).await?;
            tokio::time::timeout(
                Duration::from_secs(15),
                provider_models::fetch_with_checked(
                    &state.provider_adapters,
                    &state.provider_client,
                    &models(&input),
                    secret,
                    || ensure(state.clone(), input.clone(), lease.clone()),
                ),
            )
            .await
            .map_err(|_| ConfigError::new("models_unavailable", "获取模型超时；已有模型保持不变"))?
        };
        // Recover the precise host error if an external worker's finite guard declined.
        ensure(state.clone(), input.clone(), lease.clone()).await?;
        let mut result = result?;
        let copy = state.clone();
        let request = models(&input);
        let observations = crate::blocking::run(move || {
            let manager = copy.providers.lock().map_err(|_| unavailable())?;
            Ok::<_, ConfigError>(manager.model_observations(
                &request.endpoint,
                &request.protocol,
                request.credential_ref.as_deref(),
            ))
        })
        .await
        .map_err(crate::blocking::config_error)??;
        result.observations(observations);
        ensure(state.clone(), input.clone(), lease.clone()).await?;
        Ok(result)
    };
    let stopped = async {
        loop {
            tokio::time::sleep(Duration::from_millis(50)).await;
            ensure(state.clone(), input.clone(), lease.clone()).await?;
        }
    };
    tokio::select! {result=tokio::time::timeout(Duration::from_secs(45),work)=>result.unwrap_or_else(|_|Err(ConfigError::new("catalog_source_timeout","草稿目录读取超时，没有切换来源"))), result=stopped=>result}
}
fn error(e: ConfigError) -> (StatusCode, Json<serde_json::Value>) {
    let status = match e.code.as_str() {
        "catalog_source_changed" | "conflict" | "operation_store_changed" | "stale_snapshot" => {
            StatusCode::CONFLICT
        }
        "catalog_source_unavailable" => StatusCode::SERVICE_UNAVAILABLE,
        "catalog_source_authorization"
        | "grant_revoked"
        | "permission_denied"
        | "plugin_disabled" => StatusCode::FORBIDDEN,
        _ => StatusCode::BAD_REQUEST,
    };
    (
        status,
        Json(serde_json::json!({"code":e.code,"message":e.message})),
    )
}
pub(crate) async fn action(
    State(state): State<AppState>,
    Json(action): Json<DraftCatalogAction>,
) -> ApiResult {
    let (input, reading) = match action {
        DraftCatalogAction::Check { request } => (request, false),
        DraftCatalogAction::Read { request } => (request, true),
    };
    let lease = authority(state.clone(), &input).await.map_err(error)?;
    if reading {
        json_value(read(state, input, lease).await.map_err(error)?)
    } else {
        ensure(state, input, lease).await.map_err(error)?;
        json_value(())
    }
}
