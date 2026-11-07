use crate::{
    ApiResult, AppState,
    plugin_host::external::{self, ConnectionAccess},
};
use axum::{Json, extract::State};
use local_agent_core::{
    Provider,
    model_services::{Capabilities, ModelService},
    providers::{fingerprint, select_model},
    routing::{self, Binding},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
use uuid::Uuid;

mod bridge;
use bridge::{Profile, Selection, Status};

type Result<T> = external::Result<T>;
use external::fail;

const REVIEW_TTL: Duration = Duration::from_secs(300);
const SOURCE_GATE_WAIT: Duration = Duration::from_secs(2);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Request {
    agent_id: String,
    action: Action,
}

#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum Action {
    Status,
    Authorize,
    Revoke,
    Preview { target: Target },
    Apply { review_id: String },
    Check { review_id: String },
    Restore { review_id: String },
}

#[derive(Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum Target {
    Provider { id: String, model: String },
    Service { id: String },
}

#[derive(Clone)]
struct Destination {
    provider: Provider,
    service: Option<ModelService>,
}
impl Destination {
    fn version(&self) -> String {
        self.service
            .as_ref()
            .map_or_else(|| fingerprint(&self.provider), |s| s.version_id.clone())
    }

    fn display_target(&self, selected_model: &str) -> Option<Value> {
        if let Some(service) = &self.service {
            let model = service.models.iter().find(|model| {
                if selected_model == routing::ALIAS {
                    model.id == service.default_model_id
                } else {
                    model.alias() == selected_model
                }
            })?;
            Some(json!({"kind":"service","name":service.name,"model":model.name}))
        } else if selected_model == routing::ALIAS {
            Some(json!({"kind":"provider","name":self.provider.name,"model":self.provider.model}))
        } else {
            None
        }
    }

    fn profile(&self, id: Uuid, endpoint: String) -> Profile {
        let (name, default_model, models) = if let Some(service) = &self.service {
            (
                service.name.clone(),
                routing::ALIAS.to_owned(),
                std::iter::once({
                    let common = Capabilities::common(&service.models);
                    bridge::Model {
                        id: routing::ALIAS.to_owned(),
                        name: format!("{} · 默认", service.name),
                        context_window: common.context_window,
                        max_output_tokens: common.max_output_tokens,
                    }
                })
                .chain(service.models.iter().map(|model| bridge::Model {
                    id: model.alias(),
                    name: model.name.clone(),
                    context_window: model.capabilities.context_window,
                    max_output_tokens: model.capabilities.max_output_tokens,
                }))
                .collect(),
            )
        } else {
            let model = routing::ALIAS.to_owned();
            (
                self.provider.name.clone(),
                model.clone(),
                vec![bridge::Model {
                    id: model,
                    name: self.provider.model.clone(),
                    context_window: 8192,
                    max_output_tokens: 1024,
                }],
            )
        };
        Profile {
            route: format!("local-agent-{}", id.simple()),
            endpoint,
            name,
            default_model,
            models,
        }
    }
}

#[derive(Clone)]
pub(crate) struct Review {
    created: Instant,
    agent: String,
    access: ConnectionAccess,
    before: Status,
    target: Target,
    target_version: String,
    profile: Profile,
    applied: bool,
}
pub(crate) type Reviews = BTreeMap<Uuid, Review>;

fn access(state: &AppState, agent: &str, change: Option<bool>) -> Result<ConnectionAccess> {
    state.plugins.agent_connection_access(agent, change)
}

fn authorized(state: &AppState, agent: &str) -> Result<ConnectionAccess> {
    let access = access(state, agent, None)?;
    if access.grant.is_none() {
        return Err(fail("permission_denied"));
    }
    Ok(access)
}

fn destination(state: &AppState, target: &Target) -> Result<Destination> {
    if !crate::plugin_host::features::active(state, "builtin.providers") {
        return Err(fail("plugin_disabled"));
    }
    let manager = state.providers.lock().map_err(|_| fail("unavailable"))?;
    let store = state.store.lock().map_err(|_| fail("unavailable"))?;
    let (provider, service) = match target {
        Target::Provider { id, model } => {
            let source = store
                .providers()
                .map_err(|_| fail("unavailable"))?
                .into_iter()
                .find(|p| &p.id == id)
                .ok_or(fail("not_found"))?;
            let selected = select_model(&source, Some(model)).map_err(|_| fail("invalid_input"))?;
            manager
                .require_verification(&selected)
                .map_err(|_| fail("connection_unverified"))?;
            (selected, None)
        }
        Target::Service { id } => {
            if !crate::plugin_host::features::active(state, "builtin.model-services") {
                return Err(fail("plugin_disabled"));
            }
            let service = store.model_service(id).map_err(|_| fail("not_found"))?;
            crate::model_services::verify_snapshot(&store, &manager, &service)
                .map_err(|_| fail("connection_unverified"))?;
            let model = service
                .models
                .iter()
                .find(|m| m.id == service.default_model_id)
                .ok_or(fail("invalid_record"))?;
            (model.provider.clone(), Some(service))
        }
    };
    Ok(Destination { provider, service })
}

fn reply(status: &Status, profile: Option<&Profile>) -> Value {
    json!({
        "state": "authorized",
        "selected": status.selected,
        "configured": profile.is_some_and(|p| status.matches(p)),
        "selectedTarget": profile.is_some_and(|p| status.selected.as_ref() == Some(&p.selection())),
    })
}

async fn describe(
    state: &AppState,
    agent: &str,
    grant: &ConnectionAccess,
    status: &Status,
) -> Result<Value> {
    let mut value = reply(status, None);
    let Some(selected) = &status.selected else {
        return Ok(value);
    };
    let Some(id) = selected
        .provider
        .strip_prefix("local-agent-")
        .and_then(|route| Uuid::parse_str(route).ok())
    else {
        return Ok(value);
    };
    let target = {
        let store = state.store.lock().map_err(|_| fail("unavailable"))?;
        let Some(binding) = store
            .dynamic_binding(&id.to_string())
            .map_err(|_| fail("unavailable"))?
        else {
            return Ok(value);
        };
        if binding.agent_id != agent
            || routing::agent_connection_digest(agent, &binding.scope)
                != Some(grant.digest.as_str())
        {
            return Ok(value);
        }
        let service = store
            .model_service_attachment(&binding.id)
            .map_err(|_| fail("unavailable"))?
            .map(|attachment| attachment.installed);
        Destination {
            provider: binding.target.provider,
            service,
        }
    };
    let profile = target.profile(id, format!("http://{}/gateway/{id}", state.gateway.host));
    if selected.provider != profile.route {
        return Ok(value);
    }
    let current = bridge::read(grant, Some(&profile.route)).await?;
    if &access(state, agent, None)? != grant
        || current.instance != status.instance
        || current.selected != status.selected
    {
        return Err(fail("stale_snapshot"));
    }
    if !current.matches(&profile) {
        return Ok(value);
    }
    if let Some(display) = target.display_target(&selected.model) {
        value["currentTarget"] = display;
    }
    if let Ok(review) = get_review(state, id, agent, grant)
        && review.before.instance == current.instance
        && let Some(before) = review.before.selected
    {
        value["recovery"] = json!({"reviewId":id.to_string(),"before":before});
    }
    Ok(value)
}

fn get_review(
    state: &AppState,
    id: Uuid,
    agent: &str,
    current: &ConnectionAccess,
) -> Result<Review> {
    state
        .plugins
        .connection_reviews
        .lock()
        .map_err(|_| fail("unavailable"))?
        .get(&id)
        .filter(|r| {
            r.agent == agent
                && &r.access == current
                && (r.applied || r.created.elapsed() < REVIEW_TTL)
        })
        .cloned()
        .ok_or(fail("stale_snapshot"))
}

fn connection_binding(state: &AppState, id: Uuid) -> Result<Option<Binding>> {
    state
        .store
        .lock()
        .map_err(|_| fail("unavailable"))?
        .dynamic_binding(&id.to_string())
        .map_err(|_| fail("unavailable"))
}

fn store_binding(
    state: &AppState,
    id: Uuid,
    review: &Review,
    target: &Destination,
) -> Result<Binding> {
    let id = id.to_string();
    let token = zeroize::Zeroizing::new(format!(
        "{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    ));
    state
        .credentials
        .set(&id, token.as_bytes())
        .map_err(|_| fail("credential_unavailable"))?;
    let binding = state
        .store
        .lock()
        .map_err(|_| fail("unavailable"))?
        .create_agent_connection(
            &id,
            &review.agent,
            &format!(
                "{}{}",
                routing::AGENT_CONNECTION_SCOPE,
                review.access.digest
            ),
            &target.provider,
            target.service.as_ref(),
        );
    if binding.is_err() {
        let _ = state.credentials.delete(&id);
    }
    binding.map_err(|_| fail("conflict"))
}

fn discard(state: &AppState, binding: &Binding) -> bool {
    let removed = state
        .store
        .lock()
        .ok()
        .is_some_and(|s| s.discard_agent_connection(binding).is_ok());
    if removed {
        let _ = state.credentials.delete(&binding.credential_ref);
    }
    removed
}

fn retire(state: &AppState, binding: &Binding) -> Result<()> {
    state
        .store
        .lock()
        .map_err(|_| fail("unavailable"))?
        .revoke_agent_connection(binding)
        .map_err(|_| fail("conflict"))?;
    let _ = state.credentials.delete(&binding.credential_ref);
    Ok(())
}

fn remember_review(state: &AppState, id: Uuid, applied: bool) {
    if let Ok(mut reviews) = state.plugins.connection_reviews.lock()
        && let Some(review) = reviews.get_mut(&id)
    {
        review.applied = applied;
        if !applied {
            // Keep readback available briefly when a restore reply is lost.
            review.created = Instant::now();
        }
    }
}

async fn wait_for_source(
    state: &AppState,
    agent: &str,
) -> Result<tokio::sync::OwnedMutexGuard<()>> {
    let gate = state.plugins.agent_connection_source_gate(agent)?;
    tokio::time::timeout(SOURCE_GATE_WAIT, gate.lock_owned())
        .await
        .map_err(|_| fail("busy"))
}

async fn apply(state: &AppState, id: Uuid, review: &Review) -> Result<Value> {
    if connection_binding(state, id)?.is_some() {
        return Err(fail("connection_result_unknown"));
    }
    let current = bridge::read(&review.access, Some(&review.profile.route)).await?;
    if current.instance != review.before.instance
        || current.revision != review.before.revision
        || current.selected != review.before.selected
        || current.route.is_some()
    {
        return Err(fail("stale_snapshot"));
    }
    let target = destination(state, &review.target)?;
    if target.version() != review.target_version
        || authorized(state, &review.agent)? != review.access
    {
        return Err(fail("stale_snapshot"));
    }
    let binding = store_binding(state, id, review, &target)?;
    let token = match state.credentials.get(&binding.credential_ref) {
        Ok(token) => token,
        Err(_) => {
            discard(state, &binding);
            return Err(fail("credential_unavailable"));
        }
    };
    let token = std::str::from_utf8(&token).map_err(|_| fail("credential_unavailable"))?;
    let result = bridge::apply(&review.access, &review.before, &review.profile, token).await;
    if let Err(err) = result {
        if matches!(
            err.code,
            "stale_snapshot"
                | "invalid_input"
                | "permission_denied"
                | "bridge_unavailable"
                | "busy"
        ) {
            discard(state, &binding);
        }
        return Err(err);
    }
    let after = result?;
    if authorized(state, &review.agent)? != review.access {
        return Err(fail("grant_revoked"));
    }
    if !after.matches(&review.profile)
        || after.selected.as_ref() != Some(&review.profile.selection())
    {
        return Err(fail("connection_result_unknown"));
    }
    remember_review(state, id, true);
    Ok(reply(&after, Some(&review.profile)))
}

async fn operation(state: &AppState, request: Request) -> Result<Value> {
    let agent = request.agent_id;
    let gate = state.plugins.agent_connection_gate(&agent)?;
    let _guard = gate.try_lock_owned().map_err(|_| fail("busy"))?;
    match request.action {
        Action::Status => {
            let access = access(state, &agent, None)?;
            if access.grant.is_none() {
                return Ok(json!({"state":"authorization-required"}));
            }
            let status = bridge::read(&access, None).await?;
            describe(state, &agent, &access, &status).await
        }
        Action::Authorize => {
            let available = access(state, &agent, None)?;
            let status = bridge::read(&available, None).await?;
            if access(state, &agent, None)? != available {
                return Err(fail("grant_revoked"));
            }
            access(state, &agent, Some(true))?;
            Ok(reply(&status, None))
        }
        Action::Revoke => {
            access(state, &agent, Some(false))?;
            let retired = state
                .store
                .lock()
                .map_err(|_| fail("unavailable"))?
                .revoke_agent_connections(&agent)
                .map_err(|_| fail("unavailable"))?;
            for reference in retired {
                let _ = state.credentials.delete(&reference);
            }
            if let Ok(mut reviews) = state.plugins.connection_reviews.lock() {
                reviews.retain(|_, review| review.agent != agent);
            }
            Ok(json!({"state":"authorization-required"}))
        }
        Action::Preview { target } => {
            let access = authorized(state, &agent)?;
            let id = Uuid::new_v4();
            let selected = destination(state, &target)?;
            let profile =
                selected.profile(id, format!("http://{}/gateway/{id}", state.gateway.host));
            let before = bridge::read(&access, Some(&profile.route)).await?;
            if before.route.is_some() || authorized(state, &agent)? != access {
                return Err(fail("stale_snapshot"));
            }
            let mut reviews = state
                .plugins
                .connection_reviews
                .lock()
                .map_err(|_| fail("unavailable"))?;
            reviews.retain(|_, review| {
                review.created.elapsed()
                    < if review.applied {
                        Duration::from_secs(1800)
                    } else {
                        REVIEW_TTL
                    }
            });
            if reviews.len() >= 64 {
                return Err(fail("resource_limit"));
            }
            let response = json!({
                "reviewId": id.to_string(),
                "before": before.selected,
                "targetName": profile.name,
                "models": profile.models.iter().map(|m| &m.name).collect::<Vec<_>>(),
                "scope": "new-sessions",
            });
            reviews.insert(
                id,
                Review {
                    created: Instant::now(),
                    agent,
                    access,
                    before,
                    target,
                    target_version: selected.version(),
                    profile,
                    applied: false,
                },
            );
            Ok(response)
        }
        Action::Apply { review_id } => {
            let review_id = Uuid::parse_str(&review_id).map_err(|_| fail("invalid_input"))?;
            let _source = wait_for_source(state, &agent).await?;
            let review = get_review(state, review_id, &agent, &authorized(state, &agent)?)?;
            apply(state, review_id, &review).await
        }
        Action::Check { review_id } => {
            let review_id = Uuid::parse_str(&review_id).map_err(|_| fail("invalid_input"))?;
            let _source = wait_for_source(state, &agent).await?;
            let review = get_review(state, review_id, &agent, &authorized(state, &agent)?)?;
            let status = bridge::read(&review.access, Some(&review.profile.route)).await?;
            if authorized(state, &agent)? != review.access {
                return Err(fail("grant_revoked"));
            }
            if let Some(binding) = connection_binding(state, review_id)?
                && routing::agent_connection_digest(&binding.agent_id, &binding.scope).is_some()
            {
                if status.route.is_none() {
                    if review.applied || !discard(state, &binding) {
                        retire(state, &binding)?;
                    }
                    remember_review(state, review_id, false);
                } else if status.matches(&review.profile)
                    && status.selected.as_ref() == Some(&review.profile.selection())
                {
                    remember_review(state, review_id, true);
                }
            }
            Ok(reply(&status, Some(&review.profile)))
        }
        Action::Restore { review_id } => {
            let review_id = Uuid::parse_str(&review_id).map_err(|_| fail("invalid_input"))?;
            let _source = wait_for_source(state, &agent).await?;
            let review = get_review(state, review_id, &agent, &authorized(state, &agent)?)?;
            let binding = connection_binding(state, review_id)?.ok_or(fail("not_found"))?;
            let previous: &Selection =
                review.before.selected.as_ref().ok_or(fail("unsupported"))?;
            let status = bridge::restore(&review.access, &review.profile, previous).await?;
            if status.route.is_some() || status.selected.as_ref() != Some(previous) {
                return Err(fail("connection_result_unknown"));
            }
            retire(state, &binding)?;
            remember_review(state, review_id, false);
            Ok(reply(&status, None))
        }
    }
}

pub(crate) async fn handle(
    State(state): State<AppState>,
    Json(request): Json<Request>,
) -> ApiResult {
    if request.agent_id.len() > 200 {
        return external::api::response(Err(fail("invalid_input")));
    }
    // Do not cancel a confirmed mutation when the UI leaves the page.
    let result = tokio::spawn(async move { operation(&state, request).await })
        .await
        .unwrap_or_else(|_| Err(fail("connection_result_unknown")));
    external::api::response(result)
}
