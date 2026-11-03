//! Explicit, bounded recovery; a query never executes or reauthorizes anything.
use crate::{ApiResult, AppState, json_value};
use axum::{Json, extract::State};
use local_agent_core::{
    Store,
    domain::{
        provider_draft_grant::DraftGrantState,
        provider_draft_recovery::{
            DraftRecoveryAction, DraftRecoveryInspection, DraftRecoveryOwnership,
            DraftRecoveryReply, DraftRecoveryState,
        },
    },
    providers::{ConfigError, operations::Status},
};

pub(crate) async fn action(
    State(state): State<AppState>,
    Json(input): Json<DraftRecoveryAction>,
) -> ApiResult {
    crate::blocking::run(move || {
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
            DraftRecoveryAction::List { store_id } => store_id,
            DraftRecoveryAction::Inspect { owner } | DraftRecoveryAction::Finish { owner } => {
                &owner.store_id
            }
        };
        if store_id != &protocol.store_id {
            return Err(super::error(ConfigError::new(
                "operation_store_changed",
                "此归属属于原数据库，本次未执行",
            )));
        }
        use crate::plugin_host::external::catalog::ownership::request_hash;
        let result = (|| -> crate::plugin_host::external::Result<DraftRecoveryReply> {
            match input {
                DraftRecoveryAction::List { store_id } => {
                    let entries = state.plugins.draft_recovery_entries(&store_id)?;
                    Ok(DraftRecoveryReply::Listed { store_id, entries })
                }
                DraftRecoveryAction::Inspect { owner } => {
                    let mut snapshot = state.plugins.draft_recovery_snapshot(&owner, true)?;
                    let request = classify(&store, &mut snapshot, true)?;
                    snapshot.entry.can_finish = super::draft_grant::release_allowed(
                        &manager,
                        &store,
                        &protocol,
                        &owner,
                        snapshot.pin.as_ref(),
                        snapshot.selection.as_ref(),
                        snapshot.last_managed,
                    )?;
                    Ok(DraftRecoveryReply::Inspected {
                        inspection: Box::new(DraftRecoveryInspection {
                            entry: snapshot.entry,
                            request,
                        }),
                    })
                }
                DraftRecoveryAction::Finish { owner } => {
                    let snapshot = match state.plugins.draft_recovery_snapshot(&owner, true) {
                        Ok(s) => s,
                        Err(e) if e.code == "not_found" => {
                            return Ok(DraftRecoveryReply::Finished {
                                owner,
                                released: true,
                            });
                        }
                        Err(e) => return Err(e),
                    };
                    // This explicit lifecycle action uses the original terminal
                    // bookkeeping. Inspection above remains entirely read-only.
                    if let Some(pin) = &snapshot.pin {
                        let op = store.provider_operation(&owner.store_id, &pin.operation_id);
                        if let Ok(op) = op
                            && op.status != Status::Pending
                            && request_hash(&op.request)? == pin.request_hash
                        {
                            let _ = manager.finish_catalog_source(&store, &op.request);
                            let _ = manager.finish_catalog_operation_record(&store, &op.request);
                        }
                    }
                    let released =
                        if matches!(snapshot.entry.ownership, DraftRecoveryOwnership::Missing) {
                            state.plugins.finish_orphan_draft_request(&owner, |pin| {
                                super::draft_grant::release_allowed(
                                    &manager,
                                    &store,
                                    &protocol,
                                    &owner,
                                    Some(pin),
                                    None,
                                    false,
                                )
                            })?
                        } else {
                            matches!(
                                state
                                    .plugins
                                    .release_draft_owner(&owner, |pin, selected, last| {
                                        super::draft_grant::release_allowed(
                                            &manager, &store, &protocol, &owner, pin, selected,
                                            last,
                                        )
                                    })?
                                    .state,
                                DraftGrantState::Released
                            )
                        };
                    Ok(DraftRecoveryReply::Finished { owner, released })
                }
            }
        })()
        .map_err(|e| {
            super::error(ConfigError::new(
                e.code,
                crate::plugin_host::external::message(e.code),
            ))
        })?;
        let bytes = serde_json::to_vec(&result).map_err(|_| super::error(unavailable()))?;
        let limit = if matches!(result, DraftRecoveryReply::Listed { .. }) {
            1024 * 1024
        } else {
            local_agent_core::providers::operations::MAX_RESPONSE_BYTES
        };
        if bytes.len() > limit {
            return Err(super::error(ConfigError::new(
                "capacity",
                "恢复回复超过本次核查容量，原记录已保留",
            )));
        }
        json_value(result)
    })
    .await
    .map_err(|e| super::error(crate::blocking::config_error(e)))?
}

fn classify(
    store: &Store,
    snapshot: &mut crate::plugin_host::external::catalog::ownership::RecoverySnapshot,
    include_request: bool,
) -> crate::plugin_host::external::Result<
    Option<Box<local_agent_core::providers::operations::Request>>,
> {
    use crate::plugin_host::external::{catalog::ownership::request_hash, fail};
    let Some(pin) = &snapshot.pin else {
        snapshot.entry.state = DraftRecoveryState::Unattached;
        return Ok(None);
    };
    match store.provider_operation(&snapshot.entry.owner.store_id, &pin.operation_id) {
        Ok(op) => {
            if request_hash(&op.request)? != pin.request_hash {
                return Err(fail("invalid_record"));
            }
            snapshot.entry.state = match op.status {
                Status::Pending => DraftRecoveryState::Pending,
                Status::Committed => DraftRecoveryState::Committed,
                Status::Failed => DraftRecoveryState::Failed,
                Status::Cancelled => DraftRecoveryState::Cancelled,
            };
            Ok(include_request.then(|| Box::new(op.request)))
        }
        Err(e) if e.code == "operation_not_found" => {
            snapshot.entry.state = DraftRecoveryState::NotFound;
            Ok(if include_request {
                snapshot.request.take().map(|r| Box::new(r.operation))
            } else {
                None
            })
        }
        Err(_) => {
            snapshot.entry.state = DraftRecoveryState::Unavailable;
            Err(fail("unavailable"))
        }
    }
}

fn unavailable() -> ConfigError {
    ConfigError::new(
        "catalog_operation_unavailable",
        "草稿授权恢复记录暂不可核查，原请求与授权已保留",
    )
}
