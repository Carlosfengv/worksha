//! No network or business writes: ownership follows the original operation ledger.
use crate::{ApiResult, AppState, json_value};
use axum::{Json, extract::State};
use local_agent_core::{
    domain::provider_draft_grant::DraftGrantAction,
    providers::{ConfigError, operations::Status},
};

pub(crate) async fn action(
    State(state): State<AppState>,
    Json(input): Json<DraftGrantAction>,
) -> ApiResult {
    crate::blocking::run(move || {
        // Same manager -> Store -> plugin-metadata order as original commits.
        let manager = state
            .providers
            .lock()
            .map_err(|_| super::error(unavailable()))?;
        let store = state
            .store
            .lock()
            .map_err(|_| super::error(unavailable()))?;
        let protocol = store.provider_write_protocol().map_err(super::error)?;
        let store_id = match &input {
            DraftGrantAction::Open { store_id, .. } => store_id,
            DraftGrantAction::Acquire { owner }
            | DraftGrantAction::Attach { owner, .. }
            | DraftGrantAction::Release { owner } => &owner.store_id,
        };
        if store_id != &protocol.store_id {
            return Err(super::error(ConfigError::new(
                "operation_store_changed",
                "草稿授权属于原数据库，本次未执行",
            )));
        }

        let result = match input {
            DraftGrantAction::Open { store_id, scope } => {
                state.plugins.open_draft_owner(store_id, scope)
            }
            DraftGrantAction::Acquire { owner } => state.plugins.acquire_draft_owner(&owner),
            DraftGrantAction::Attach { owner, request } => {
                let existing = store
                    .existing_provider_operation(&request.operation)
                    .map_err(super::error)?;
                if existing.is_some_and(|op| op.status == Status::Pending) {
                    manager
                        .verify_source_operation(&request)
                        .map_err(super::error)?;
                }
                state.plugins.attach_draft_owner(&owner, &request)
            }
            DraftGrantAction::Release { owner } => {
                state
                    .plugins
                    .release_draft_owner(&owner, |pin, selection, last_managed| {
                        release_allowed(
                            &manager,
                            &store,
                            &protocol,
                            &owner,
                            pin,
                            selection,
                            last_managed,
                        )
                    })
            }
        }
        .map_err(|e| {
            super::error(ConfigError::new(
                e.code,
                crate::plugin_host::external::message(e.code),
            ))
        })?;
        json_value(result)
    })
    .await
    .map_err(|e| super::error(crate::blocking::config_error(e)))?
}

fn unavailable() -> ConfigError {
    ConfigError::new(
        "catalog_operation_unavailable",
        "草稿授权归属尚不可核查，原操作与授权已保留",
    )
}

// Shared by the original release command and independent recovery. Both keep
// the original manager -> Store -> plugin-metadata lock order and SQL proof.
pub(super) fn release_allowed(
    manager: &local_agent_core::providers::ProviderManager,
    store: &local_agent_core::Store,
    protocol: &local_agent_core::providers::operations::Protocol,
    owner: &local_agent_core::domain::provider_draft_grant::DraftGrantOwner,
    pin: Option<&crate::plugin_host::external::catalog::ownership::Pin>,
    selection: Option<&local_agent_core::domain::provider_connection::Selection>,
    last_managed: bool,
) -> crate::plugin_host::external::Result<bool> {
    use crate::plugin_host::external::{catalog::ownership::request_hash, fail};
    if let Some(pin) = pin {
        let op = match store.provider_operation(&owner.store_id, &pin.operation_id) {
            Ok(op) => op,
            Err(e) if e.code == "operation_not_found" => return Ok(false),
            Err(_) => return Err(fail("unavailable")),
        };
        if request_hash(&op.request)? != pin.request_hash {
            return Err(fail("invalid_record"));
        }
        if op.status == Status::Pending {
            return Ok(false);
        }
    }
    // Another operation may have borrowed the exact scope through
    // an older entry point. Never revoke its in-flight authority.
    if last_managed && let Some(selection) = selection {
        for id in &protocol.pending_operation_ids {
            let op = store
                .provider_operation(&owner.store_id, id)
                .map_err(|_| fail("unavailable"))?;
            let p = &op.request.provider;
            if let local_agent_core::domain::provider_access::Target::Draft {
                endpoint,
                protocol,
                vendor,
                credential_ref,
                ..
            } = &selection.target
                && (&p.endpoint, &p.protocol, &p.vendor, &p.credential_ref)
                    == (endpoint, protocol, vendor, credential_ref)
            {
                match manager.catalog_operation_selection(&op.request) {
                    Ok(other) if other.binding_id != selection.binding_id => {}
                    _ => return Ok(false),
                }
            }
        }
    }
    Ok(true)
}
