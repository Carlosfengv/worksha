//! Current consumers resolve remembered sources explicitly. Old native IPC stays native.
use crate::{ApiResult, AppState, json_value, plugin_host::external::catalog, provider_models};
use axum::{Json, extract::State, http::StatusCode};
use local_agent_core::{
    Provider, Store,
    domain::{
        provider_access::Target,
        provider_source::{Choice, SourceAction, SourceKind, SourceQuery, SourceState},
    },
    providers::{CatalogSourceSnapshot, ConfigError, ProviderManager},
};
use std::time::Duration;
pub(super) struct Loaded {
    pub(super) provider: Provider,
    pub(super) providers: Vec<Provider>,
    pub(super) source: CatalogSourceSnapshot,
}
fn fail(code: &str, message: &str) -> ConfigError {
    ConfigError::new(code, message)
}
fn unavailable() -> ConfigError {
    fail(
        "catalog_source_unavailable",
        "目录来源暂不可读取，未改用其他目录",
    )
}
fn native_request(provider: &Provider) -> provider_models::ModelsRequest {
    // Match the legacy UI's providerVendor rule for rows predating vendor.
    let vendor = if provider.vendor.is_empty() {
        if local_agent_core::providers::catalog::builtin().contains(&provider.id) {
            provider.id.clone()
        } else {
            "custom".into()
        }
    } else {
        provider.vendor.clone()
    };
    provider_models::ModelsRequest {
        endpoint: provider.endpoint.clone(),
        protocol: provider.protocol.clone(),
        vendor,
        credential_ref: provider.credential_ref.clone(),
    }
}
fn plugin_error(error: crate::plugin_host::external::Error) -> ConfigError {
    fail(
        error.code,
        crate::plugin_host::external::message(error.code),
    )
}
fn error(e: ConfigError) -> (StatusCode, Json<serde_json::Value>) {
    let status = match e.code.as_str() {
        "catalog_source_changed"
        | "catalog_source_pending"
        | "conflict"
        | "operation_store_changed" => StatusCode::CONFLICT,
        "catalog_source_authorization"
        | "permission_denied"
        | "grant_revoked"
        | "plugin_disabled" => StatusCode::FORBIDDEN,
        "catalog_source_unavailable" => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::BAD_REQUEST,
    };
    (
        status,
        Json(serde_json::json!({"code":e.code,"message":e.message})),
    )
}
pub(super) async fn load(state: AppState, query: SourceQuery) -> Result<Loaded, ConfigError> {
    crate::blocking::run(move || {
        let manager = state.providers.lock().map_err(|_| unavailable())?;
        let store = state.store.lock().map_err(|_| unavailable())?;
        load_locked(&manager, &store, &query)
    })
    .await
    .map_err(crate::blocking::config_error)?
}
fn load_locked(
    manager: &ProviderManager,
    store: &Store,
    query: &SourceQuery,
) -> Result<Loaded, ConfigError> {
    if store.provider_write_protocol()?.store_id != query.store_id {
        return Err(fail(
            "operation_store_changed",
            "目录选择属于原数据库，本次未执行",
        ));
    }
    let provider = store
        .provider_by_id(&query.provider_id)
        .map_err(|_| unavailable())?
        .ok_or_else(unavailable)?;
    let providers = vec![provider.clone()];
    if provider.revision.to_string() != query.expected_revision {
        return Err(fail("conflict", "连接已变化，请重新读取目录来源"));
    }
    let source = manager.catalog_source_snapshot(store, &provider)?;
    Ok(Loaded {
        provider,
        providers,
        source,
    })
}
async fn read_unchanged(
    state: AppState,
    query: SourceQuery,
    revision: Option<String>,
    lease: Option<catalog::Lease>,
) -> Result<(), ConfigError> {
    crate::blocking::run(move || {
        let manager = state.providers.lock().map_err(|_| unavailable())?;
        let store = state.store.lock().map_err(|_| unavailable())?;
        let current = load_locked(&manager, &store, &query)?;
        if current.source.revision != revision
            || current.source.pending
            || !current.source.scope_matches
        {
            return Err(fail(
                "catalog_source_changed",
                "目录选择已变化，旧结果没有交付",
            ));
        }
        if let Some(lease) = lease {
            lease
                .ensure(&state.plugins, &current.providers, &current.provider)
                .map_err(plugin_error)?;
        }
        Ok(())
    })
    .await
    .map_err(crate::blocking::config_error)?
}
async fn unchanged(
    state: AppState,
    query: SourceQuery,
    revision: Option<String>,
) -> Result<(), ConfigError> {
    let current = load(state, query).await?;
    if current.source.revision != revision || current.source.pending {
        return Err(fail(
            "catalog_source_changed",
            "目录选择已变化，旧结果没有交付",
        ));
    }
    Ok(())
}
async fn describe(state: AppState, query: SourceQuery) -> Result<SourceState, ConfigError> {
    let loaded = load(state.clone(), query.clone()).await?;
    let mut result = SourceState {
        provider_id: query.provider_id.clone(),
        expected_revision: query.expected_revision.clone(),
        source_revision: loaded.source.revision.clone(),
        kind: SourceKind::Builtin,
        choice: None,
        selection: None,
        authorized: true,
        policy: None,
        error_code: None,
    };
    if loaded.source.pending && loaded.source.catalog.is_none() {
        return Err(fail(
            "catalog_source_pending",
            "内置来源切换仍待原操作核查，没有交付目录权限",
        ));
    }
    if let Some(selection) = loaded.source.catalog.clone() {
        result.choice = Some(Choice {
            plugin_id: selection.plugin_id.clone(),
            digest: selection.digest.clone(),
        });
        result.authorized = false;
        result.kind = SourceKind::External;
        if loaded.source.pending {
            result.kind = SourceKind::Pending;
            result.error_code = Some("catalog_source_pending".into());
            return Ok(result);
        }
        if !loaded.source.scope_matches {
            result.kind = SourceKind::Changed;
            result.error_code = Some("catalog_source_changed".into());
            return Ok(result);
        }
        let manager = state.plugins.clone();
        let checked=crate::blocking::run(move|| {
            let policy=manager.catalog_policy(&selection.plugin_id,&selection.digest).map_err(plugin_error)?;
            let saved=matches!(&selection.target,Target::Saved{provider_id,expected_revision} if provider_id==&loaded.provider.id && expected_revision==&loaded.provider.revision.to_string());
            let authorized=if saved {manager.catalog_connection_lease(&selection,&loaded.providers,&loaded.provider).map(|_|()).map_err(plugin_error)} else {Err(fail("catalog_source_authorization","请为已保存连接重新授权目录读取"))};
            Ok::<_,ConfigError>((policy,authorized.map(|_|selection)))
        }).await.map_err(crate::blocking::config_error)?;
        match checked {
            Ok((policy, authorization)) => {
                result.policy = Some(policy);
                match authorization {
                    Ok(selection) => {
                        result.authorized = true;
                        result.selection = Some(selection);
                    }
                    Err(e) => result.error_code = Some(e.code),
                }
            }
            Err(e) => result.error_code = Some(e.code),
        }
    }
    unchanged(state, query, result.source_revision.clone()).await?;
    Ok(result)
}
async fn select(
    state: AppState,
    query: SourceQuery,
    revision: Option<String>,
    catalog: Option<local_agent_core::domain::provider_connection::Selection>,
) -> Result<SourceState, ConfigError> {
    let loaded = load(state.clone(), query.clone()).await?;
    if let Some(selection) = &catalog {
        if !matches!(&selection.target,Target::Saved{provider_id,expected_revision} if provider_id==&query.provider_id && expected_revision==&query.expected_revision)
        {
            return Err(fail(
                "catalog_source_authorization",
                "请单独授权当前已保存连接，草稿权限不会自动延续",
            ));
        }
        let manager = state.plugins.clone();
        let selection = selection.clone();
        let p = loaded.provider.clone();
        let providers = loaded.providers;
        crate::blocking::run(move || {
            manager
                .catalog_connection_lease(&selection, &providers, &p)
                .map(|_| ())
                .map_err(plugin_error)
        })
        .await
        .map_err(crate::blocking::config_error)??;
    }
    let copy = state.clone();
    let q = query.clone();
    crate::blocking::run(move || {
        let manager = copy.providers.lock().map_err(|_| unavailable())?;
        let store = copy.store.lock().map_err(|_| unavailable())?;
        if store.provider_write_protocol()?.store_id != q.store_id {
            return Err(fail("operation_store_changed", "目录选择属于原数据库"));
        }
        let provider = store
            .provider_by_id(&q.provider_id)
            .map_err(|_| unavailable())?
            .filter(|p| p.revision.to_string() == q.expected_revision)
            .ok_or_else(|| fail("conflict", "连接已变化"))?;
        let providers = std::slice::from_ref(&provider);
        if let Some(selection) = &catalog {
            copy.plugins
                .catalog_connection_lease(selection, providers, &provider)
                .map_err(plugin_error)?;
        }
        manager.select_catalog_source(&store, &provider, revision.as_deref(), catalog)
    })
    .await
    .map_err(crate::blocking::config_error)??;
    describe(state, query).await
}
async fn read(
    state: AppState,
    query: SourceQuery,
    revision: Option<String>,
) -> Result<provider_models::Catalog, ConfigError> {
    let loaded = load(state.clone(), query.clone()).await?;
    if loaded.source.revision != revision || loaded.source.pending || !loaded.source.scope_matches {
        return Err(fail(
            "catalog_source_changed",
            "目录选择或连接范围已变化，未读取其他目录",
        ));
    }
    let lease = if let Some(selection) = &loaded.source.catalog {
        if !matches!(&selection.target,Target::Saved{provider_id,expected_revision} if provider_id==&query.provider_id && expected_revision==&query.expected_revision)
        {
            return Err(fail(
                "catalog_source_authorization",
                "已记住目录实现，请重新授权当前已保存连接",
            ));
        }
        let copy = state.clone();
        let selection = selection.clone();
        let providers = loaded.providers.clone();
        let provider = loaded.provider.clone();
        Some(
            crate::blocking::run(move || {
                copy.plugins
                    .catalog_connection_lease(&selection, &providers, &provider)
                    .map_err(plugin_error)
            })
            .await
            .map_err(crate::blocking::config_error)??,
        )
    } else {
        None
    };
    let work = async {
        read_unchanged(
            state.clone(),
            query.clone(),
            revision.clone(),
            lease.clone(),
        )
        .await?;
        let mut result = if let Some(selection) = &loaded.source.catalog {
            catalog::read_for_connection(state.clone(), selection)
                .await
                .map_err(plugin_error)?
        } else {
            let _permit = state
                .verification_capacity
                .clone()
                .try_acquire_owned()
                .map_err(|_| fail("busy", "已有目录或验证任务正在进行"))?;
            let secret = if let Some(reference) = &loaded.provider.credential_ref {
                Some(crate::blocking::secret(state.credentials.clone(), reference.clone()).await?)
            } else {
                None
            };
            // Preserve the legacy native directory's complete-read deadline
            // and public error mapping; the outer source/lease fence is shared.
            tokio::time::timeout(
                Duration::from_secs(15),
                provider_models::fetch_with(
                    &state.provider_adapters,
                    &state.provider_client,
                    &native_request(&loaded.provider),
                    secret,
                ),
            )
            .await
            .map_err(|_| fail("models_unavailable", "获取模型超时；已有模型保持不变"))??
        };
        let copy = state.clone();
        let p = loaded.provider.clone();
        let observations = crate::blocking::run(move || {
            let manager = copy.providers.lock().map_err(|_| unavailable())?;
            Ok::<_, ConfigError>(manager.model_observations(
                &p.endpoint,
                &p.protocol,
                p.credential_ref.as_deref(),
            ))
        })
        .await
        .map_err(crate::blocking::config_error)??;
        result.observations(observations);
        read_unchanged(
            state.clone(),
            query.clone(),
            revision.clone(),
            lease.clone(),
        )
        .await?;
        Ok(result)
    };
    let stopped = async {
        loop {
            tokio::time::sleep(Duration::from_millis(50)).await;
            read_unchanged(
                state.clone(),
                query.clone(),
                revision.clone(),
                lease.clone(),
            )
            .await?;
        }
    };
    tokio::select! {result=tokio::time::timeout(Duration::from_secs(45),work)=>result.unwrap_or_else(|_|Err(fail("catalog_source_timeout","目录读取超时，未切换来源或重新发送"))),result=stopped=>result}
}

pub(crate) async fn action(
    State(state): State<AppState>,
    Json(input): Json<SourceAction>,
) -> ApiResult {
    match input {
        SourceAction::State { query } => json_value(describe(state, query).await.map_err(error)?),
        SourceAction::Select {
            query,
            source_revision,
            catalog,
        } => json_value(
            select(state, query, source_revision, catalog)
                .await
                .map_err(error)?,
        ),
        SourceAction::Read {
            query,
            source_revision,
        } => json_value(read(state, query, source_revision).await.map_err(error)?),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_native_request_preserves_legacy_vendor_inference_and_explicit_override() {
        for (id, vendor, expected) in [
            ("openai", "", "openai"),
            ("anthropic", "", "anthropic"),
            ("custom-owned", "", "custom"),
            ("openai", "custom", "custom"),
        ] {
            let provider:Provider=serde_json::from_value(serde_json::json!({"id":id,"name":"Owned","vendor":vendor,"endpoint":"https://example.invalid/v1","protocol":"responses","model":"fixture","configured":true,"category":"custom","credentialRef":"fixture-ref"})).unwrap();
            let request = native_request(&provider);
            assert_eq!(request.vendor, expected);
            assert_eq!(request.endpoint, provider.endpoint);
            assert_eq!(request.protocol, provider.protocol);
            assert_eq!(request.credential_ref, provider.credential_ref);
            assert_eq!(provider.vendor, vendor);
        }
    }
}
