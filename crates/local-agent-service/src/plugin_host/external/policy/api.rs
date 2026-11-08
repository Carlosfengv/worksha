use super::{
    super::{Checked, Result, api::response, fail, run_sync},
    store::{self, Control, Intent},
};
use crate::{ApiResult, AppState};
use axum::{Extension, Json, Router, extract::State, routing::post};
use local_agent_core::{
    model_services::operations::{OperationAction, OperationReceipt},
    plugin::policy as p,
};
pub(in crate::plugin_host::external) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/routing-policy", post(perform))
        .route(
            "/api/routing-policy-recovery",
            post(super::recovery::perform),
        )
}
async fn perform(
    State(state): State<AppState>,
    actor: Option<Extension<crate::iam::Actor>>,
    Json(action): Json<p::Action>,
) -> ApiResult {
    response(
        apply(state, actor, action)
            .await
            .and_then(|v| serde_json::to_value(v).checked("invalid_record")),
    )
}
fn scope(state: &AppState, id: &str) -> Result<()> {
    state
        .store
        .lock()
        .checked("unavailable")?
        .require_model_service_store(id)
        .checked("stale_snapshot")
}
async fn apply(
    state: AppState,
    actor: Option<Extension<crate::iam::Actor>>,
    action: p::Action,
) -> Result<p::Reply> {
    let s = state.clone();
    let id = action.store_id().to_owned();
    run_sync(move || scope(&s, &id)).await?;
    match action {
        p::Action::State {
            package, action, ..
        } => Ok(p::Reply::Package {
            status: state.plugins.policy_change(package, action).await?,
        }),
        p::Action::Publish {
            request,
            expected_binding_revision,
            package,
            failure_mode,
        } => {
            publish(
                state,
                actor,
                request,
                expected_binding_revision,
                package,
                failure_mode,
            )
            .await
        }
        p::Action::ResumePublish {
            store_id,
            operation_id,
        } => {
            store::uuid(&operation_id)?;
            let s = state.clone();
            let status = run_sync(move || reconcile(&s, &store_id, &operation_id)).await?;
            if matches!(status.state.as_str(), "committed" | "cancelled") {
                return Ok(p::Reply::Publish {
                    status: Box::new(status),
                });
            }
            submit_original(state, actor, status.request).await
        }
        action => {
            run_sync(move || match action {
                p::Action::List { store_id } => {
                    let (packages, controls, intents) = {
                        let h = state.plugins.policy.lock().checked("unavailable")?;
                        (h.packages()?, h.controls(&store_id)?, h.intents(&store_id)?)
                    };
                    let packages = packages
                        .iter()
                        .map(|i| {
                            state.plugins.policy_status(&p::PackageQuery {
                                plugin_id: i.manifest.id.clone(),
                                digest: i.digest.clone(),
                            })
                        })
                        .collect::<Result<Vec<_>>>()?;
                    let scopes = controls
                        .iter()
                        .map(|c| p::ScopeStatus {
                            store_id: c.store_id.clone(),
                            service_id: c.service_id.clone(),
                            revision: c.revision.clone(),
                            pending_operation_id: c.pending.clone(),
                            error_code: state
                                .plugins
                                .policy
                                .lock()
                                .map_err(|_| fail("unavailable"))
                                .and_then(|h| h.consistent_control(&store_id, &c.service_id))
                                .err()
                                .map(|e| e.code.into()),
                        })
                        .collect();
                    let mut bindings = vec![];
                    for c in controls {
                        if let Some(mut b) = c.head {
                            let stored = {
                                state.plugins.policy.lock().checked("unavailable")?.binding(
                                    &store_id,
                                    &b.service_id,
                                    &b.service_version_id,
                                )
                            };
                            let result = stored
                                .and_then(|row| row.ok_or(fail("invalid_record")))
                                .and_then(|stored| {
                                    state.plugins.policy_context(&b.package, Some(stored))
                                })
                                .and_then(|c| state.plugins.policy_check(&c));
                            b.authorized = result.is_ok();
                            b.error_code = result.err().map(|e| e.code.into());
                            bindings.push(b);
                        }
                    }
                    Ok(p::Reply::Inventory {
                        store_id,
                        packages,
                        bindings,
                        scopes,
                        unavailable_service_ids: state
                            .plugins
                            .policy_presence
                            .read()
                            .map_err(|_| fail("unavailable"))?
                            .unknown
                            .iter()
                            .cloned()
                            .collect(),
                        publishes: intents
                            .into_iter()
                            .map(|i| publication_status(&state, i))
                            .collect::<Result<Vec<_>>>()?,
                    })
                }
                p::Action::Inspect { path, .. } => Ok(p::Reply::Review {
                    review: state.plugins.policy_inspect(std::path::Path::new(&path))?,
                }),
                p::Action::Install {
                    package_ref,
                    digest,
                    expected_signing_key,
                    approve_publisher,
                    ..
                } => Ok(p::Reply::Package {
                    status: state.plugins.policy_install(
                        &package_ref,
                        &digest,
                        &expected_signing_key,
                        approve_publisher,
                    )?,
                }),
                p::Action::Bind {
                    store_id,
                    service_id,
                    service_version_id,
                    expected_binding_revision,
                    package,
                    failure_mode,
                } => {
                    store::uuid(&service_id)?;
                    store::uuid(&service_version_id)?;
                    let c = state.plugins.policy_context(&package, None)?;
                    state.plugins.policy_check(&c)?;
                    let guard = state.store.lock().checked("unavailable")?;
                    guard
                        .require_model_service_store(&store_id)
                        .checked("stale_snapshot")?;
                    let model = guard.model_service(&service_id).checked("not_found")?;
                    if model.version_id != service_version_id || !model.routing_policy.enabled {
                        return Err(fail("stale_snapshot"));
                    }
                    let h = state.plugins.policy.lock().checked("unavailable")?;
                    let b = h.bind(
                        store::BindRequest {
                            store: &store_id,
                            service: &service_id,
                            version: &service_version_id,
                            expected: &expected_binding_revision,
                            package: &package,
                            failure: failure_mode,
                            pending: None,
                        },
                        &store::Authorization::from_context(&c, &state.plugins.catalog_instance),
                    )?;
                    Ok(p::Reply::Binding { binding: b.status })
                }
                p::Action::Unbind {
                    store_id,
                    service_id,
                    expected_binding_revision,
                } => {
                    store::uuid(&service_id)?;
                    let _store = state.store.lock().checked("unavailable")?;
                    let revision = state.plugins.policy.lock().checked("unavailable")?.unbind(
                        &store_id,
                        &service_id,
                        &expected_binding_revision,
                    )?;
                    Ok(p::Reply::Unbound {
                        store_id,
                        service_id,
                        revision,
                    })
                }
                p::Action::QueryPublish {
                    store_id,
                    operation_id,
                } => {
                    store::uuid(&operation_id)?;
                    Ok(p::Reply::Publish {
                        status: Box::new(reconcile(&state, &store_id, &operation_id)?),
                    })
                }
                p::Action::CancelPublish {
                    store_id,
                    operation_id,
                } => {
                    store::uuid(&operation_id)?;
                    let core = state.store.lock().checked("unavailable")?;
                    core.require_model_service_store(&store_id)
                        .checked("stale_snapshot")?;
                    let h = state.plugins.policy.lock().checked("unavailable")?;
                    let i = h.intent(&operation_id)?;
                    if i.status.store_id != store_id || i.status.state != "prepared" {
                        return Err(fail("result_pending"));
                    }
                    match core.model_service_operation_for(&i.status.request) {
                        Err(e) if e.code == "operation_not_found" => {}
                        _ => return Err(fail("result_pending")),
                    }
                    let i = h.cancel_prepared(i)?;
                    Ok(p::Reply::Publish {
                        status: Box::new(i.status),
                    })
                }
                _ => Err(fail("invalid_input")),
            })
            .await
        }
    }
}
async fn publish(
    state: AppState,
    actor: Option<Extension<crate::iam::Actor>>,
    request: local_agent_core::model_services::operations::OperationRequest,
    expected: Option<String>,
    package: p::PackageQuery,
    failure: p::FailureMode,
) -> Result<p::Reply> {
    let s = state.clone();
    let req = request.clone();
    let prepared = run_sync(move || {
        req.validate().checked("invalid_input")?;
        let OperationAction::Save { draft } = &req.action else {
            return Err(fail("invalid_input"));
        };
        if !draft.routing_policy.enabled {
            return Err(fail("invalid_input"));
        }
        let old = {
            s.plugins
                .policy
                .lock()
                .checked("unavailable")?
                .intent_optional(&req.operation_id)?
        };
        if let Some(old) = old {
            if store::encode(&old.status.request)? != store::encode(&req)?
                || old.package != package
                || old.expected != expected
                || old.failure != failure
            {
                return Err(fail("stale_snapshot"));
            }
            return Ok((false, publication_status(&s, old)?));
        }
        let c = s.plugins.policy_context(&package, None)?;
        s.plugins.policy_check(&c)?;
        let core = s.store.lock().checked("unavailable")?;
        core.require_model_service_store(&req.store_id)
            .checked("stale_snapshot")?;
        let h = s.plugins.policy.lock().checked("unavailable")?;
        if let Some(old) = h.intent_optional(&req.operation_id)? {
            if store::encode(&old.status.request)? != store::encode(&req)?
                || old.package != package
                || old.expected != expected
                || old.failure != failure
            {
                return Err(fail("stale_snapshot"));
            }
            drop(h);
            drop(core);
            return Ok((false, publication_status(&s, old)?));
        }
        if h.intents(&req.store_id)?.len() >= 256 {
            return Err(fail("resource_limit"));
        }
        match core.model_service(&draft.id) {
            Ok(current) if current.revision == draft.revision => {}
            Err(e) if e.code == "not_found" && draft.revision == 0 => {}
            _ => return Err(fail("stale_snapshot")),
        }
        let old = h.consistent_control(&req.store_id, &draft.id)?;
        if old.as_ref().map(|c| &c.revision) != expected.as_ref()
            || old.as_ref().is_some_and(|c| c.pending.is_some())
        {
            return Err(fail("stale_snapshot"));
        }
        let mut control = old.unwrap_or_else(|| Control {
            store_id: req.store_id.clone(),
            service_id: draft.id.clone(),
            revision: uuid::Uuid::new_v4().to_string(),
            authority: uuid::Uuid::new_v4().to_string(),
            version: None,
            head: None,
            pending: None,
            pending_request_identity: None,
        });
        control.pending = Some(req.operation_id.clone());
        control.pending_request_identity = Some(store::operation_identity(&req)?);
        let i = Intent {
            status: p::PublishStatus {
                store_id: req.store_id.clone(),
                operation_id: req.operation_id.clone(),
                service_id: draft.id.clone(),
                state: "prepared".into(),
                error_code: None,
                binding: None,
                request: req.clone(),
            },
            package,
            failure,
            expected,
            control_revision: control.revision.clone(),
            before_revision: draft.revision,
            authorization: Some(store::Authorization::from_context(
                &c,
                &s.plugins.catalog_instance,
            )),
            cancellation_revision: None,
        };
        h.prepare_intent(&control, &i)?;
        Ok((true, i.status))
    })
    .await?;
    if !prepared.0 {
        return Ok(p::Reply::Publish {
            status: Box::new(prepared.1),
        });
    }
    submit_original(state, actor, request).await
}
/// Only an explicit first publication or resume may enter the original publisher.
/// The persisted original request and operation ID remain the sole input.
async fn submit_original(
    state: AppState,
    actor: Option<Extension<crate::iam::Actor>>,
    request: local_agent_core::model_services::operations::OperationRequest,
) -> Result<p::Reply> {
    let s = state.clone();
    let op = request.operation_id.clone();
    let original = request.clone();
    let terminal = run_sync(move || {
        let h = s.plugins.policy.lock().checked("unavailable")?;
        let mut i = h.intent(&op)?;
        if store::encode(&i.status.request)? != store::encode(&original)?
            || i.status.store_id != original.store_id
            || i.status.operation_id != original.operation_id
            || i.status.service_id != original.action.identity().1
            || i.before_revision != original.action.identity().2
        {
            return Err(fail("stale_snapshot"));
        }
        if matches!(i.status.state.as_str(), "committed" | "cancelled") {
            return Ok(true);
        }
        if !matches!(
            i.status.state.as_str(),
            "prepared" | "committing" | "unknown"
        ) || i.authorization.is_none()
        {
            return Err(fail("grant_revoked"));
        }
        let c = h
            .control(&i.status.store_id, &i.status.service_id)?
            .ok_or(fail("invalid_record"))?;
        let identity = store::operation_identity(&original)?;
        if c.pending_request_identity.as_deref() != Some(identity.as_str()) {
            return Err(fail("invalid_record"));
        }
        h.sync_pending(&c, &op, &identity, &i.control_revision)?;
        i.status.state = "committing".into();
        h.save_intent(&i)?;
        Ok(false)
    })
    .await?;
    if terminal {
        let s = state.clone();
        return Ok(p::Reply::Publish {
            status: Box::new(
                run_sync(move || reconcile(&s, &request.store_id, &request.operation_id)).await?,
            ),
        });
    }
    // Core checks the immutable receipt before any fresh validation or write.
    let result =
        crate::model_services::commit_operation(State(state.clone()), actor, Json(request.clone()))
            .await;
    let s = state.clone();
    let store = request.store_id;
    let op = request.operation_id;
    let status = run_sync(move || match result {
        Ok(_) => reconcile(&s, &store, &op),
        Err(_) => {
            let h = s.plugins.policy.lock().checked("unavailable")?;
            let mut i = h.intent(&op)?;
            if matches!(i.status.state.as_str(), "committed" | "cancelled") {
                drop(h);
                return reconcile(&s, &store, &op);
            }
            i.status.state = "unknown".into();
            i.status.error_code = Some("result_pending".into());
            h.save_intent(&i)?;
            Ok(i.status)
        }
    })
    .await?;
    Ok(p::Reply::Publish {
        status: Box::new(status),
    })
}
fn publication_status(state: &AppState, mut i: Intent) -> Result<p::PublishStatus> {
    if let Some(mut status) = i.status.binding.clone() {
        let stored = {
            state.plugins.policy.lock().checked("unavailable")?.binding(
                &status.store_id,
                &status.service_id,
                &status.service_version_id,
            )
        };
        let result = stored
            .and_then(|row| {
                row.filter(|b| b.status.revision == status.revision)
                    .ok_or(fail("grant_revoked"))
            })
            .and_then(|b| state.plugins.policy_context(&status.package, Some(b)))
            .and_then(|c| state.plugins.policy_check(&c));
        status.authorized = result.is_ok();
        status.error_code = result.err().map(|e| e.code.into());
        i.status.binding = Some(status);
    }
    Ok(i.status)
}
fn reconcile(state: &AppState, store_id: &str, operation: &str) -> Result<p::PublishStatus> {
    let mut i = {
        state
            .plugins
            .policy
            .lock()
            .checked("unavailable")?
            .intent(operation)?
    };
    if i.status.store_id != store_id {
        return Err(fail("stale_snapshot"));
    }
    if i.status.state == "cancelled" {
        state
            .plugins
            .policy
            .lock()
            .checked("unavailable")?
            .repair_cancelled(&i)?;
        return Ok(i.status);
    }
    if i.status.state == "committed" {
        let binding = i.status.binding.as_ref().ok_or(fail("invalid_record"))?;
        {
            state
                .plugins
                .policy
                .lock()
                .checked("unavailable")?
                .clear_publish_pending(
                    binding,
                    operation,
                    &store::operation_identity(&i.status.request)?,
                )?;
        }
        return publication_status(state, i);
    }
    let receipt: OperationReceipt = match state
        .store
        .lock()
        .checked("unavailable")?
        .model_service_operation_for(&i.status.request)
    {
        Ok(r) => r,
        Err(e) if e.code == "operation_not_found" => {
            return Ok(p::PublishStatus {
                state: if i.status.state == "prepared" {
                    "prepared".into()
                } else {
                    "unknown".into()
                },
                error_code: if i.status.state == "prepared" {
                    None
                } else {
                    Some("result_pending".into())
                },
                ..i.status
            });
        }
        Err(e) if e.code == "operation_conflict" => return Err(fail("stale_snapshot")),
        Err(_) => return Err(fail("invalid_record")),
    };
    let service = receipt.service.ok_or(fail("invalid_record"))?;
    if service.id != i.status.service_id || service.revision != i.before_revision + 1 {
        return Err(fail("stale_snapshot"));
    }
    {
        let h = state.plugins.policy.lock().checked("unavailable")?;
        // A simultaneous query may have completed while the main receipt was
        // being read. Return that durable result rather than rebinding it.
        let latest = h.intent(operation)?;
        if matches!(latest.status.state.as_str(), "committed" | "cancelled") {
            drop(h);
            return reconcile(state, store_id, operation);
        }
        let ctrl = h
            .control(store_id, &service.id)?
            .ok_or(fail("invalid_record"))?;
        if ctrl.pending.as_deref() != Some(operation) {
            return Err(fail("stale_snapshot"));
        }
        let identity = store::operation_identity(&i.status.request)?;
        h.sync_pending(&ctrl, operation, &identity, &i.control_revision)?;
        let existing = h.binding(store_id, &service.id, &service.version_id)?;
        let binding = if let Some(b) = existing.filter(|b| {
            ctrl.head
                .as_ref()
                .is_some_and(|head| head.revision == b.status.revision)
                && ctrl.version.as_deref() == Some(service.version_id.as_str())
        }) {
            if b.status.package != i.package
                || b.status.failure_mode != i.failure
                || b.control_authority != ctrl.authority
                || i.authorization.as_ref().is_some_and(|a| !a.matches(&b))
            {
                return Err(fail("invalid_record"));
            }
            b
        } else {
            let authorization = i.authorization.as_ref().ok_or(fail("grant_revoked"))?;
            h.bind(
                store::BindRequest {
                    store: store_id,
                    service: &service.id,
                    version: &service.version_id,
                    expected: &Some(i.control_revision.clone()),
                    package: &i.package,
                    failure: i.failure,
                    pending: Some(operation),
                },
                authorization,
            )?
        };
        i.status.state = "committed".into();
        i.status.error_code = None;
        i.status.binding = Some(binding.status);
        h.save_intent(&i)?;
        h.clear_publish_pending(
            i.status.binding.as_ref().ok_or(fail("invalid_record"))?,
            operation,
            &identity,
        )?;
    }
    publication_status(state, i)
}
