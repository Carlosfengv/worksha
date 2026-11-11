//! A fallback checkpoint contains only candidate references and execution metadata.
use super::{Failure, RouteFrame, group_slot, guard_publication, nested, rejection};
use crate::{AppState, plugin_host::external::policy::ChoiceProof};
use axum::{
    http::{HeaderValue, StatusCode},
    response::Response,
};
use local_agent_core::{
    model_services::policy::{Dispatch, FallbackCheckpoint},
    providers::{ConfigError, upstream_accounts::AccountRef},
    routing::Selection,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};
use tokio::time::Instant;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct CandidateKey {
    route: String,
    account: Option<AccountRef>,
}

impl CandidateKey {
    pub(super) fn of(candidate: &Selection) -> Self {
        Self {
            route: candidate.version.id.clone(),
            account: candidate
                .service
                .as_ref()
                .and_then(|s| s.upstream_account.clone()),
        }
    }

    fn resolve(&self, source: &[Selection]) -> Result<Selection, ConfigError> {
        source
            .iter()
            .find(|candidate| {
                candidate.version.id == self.route
                    && candidate
                        .service
                        .as_ref()
                        .and_then(|s| s.upstream_account.as_ref())
                        == self.account.as_ref()
            })
            .cloned()
            .ok_or_else(invalid)
    }
}

#[derive(Serialize, Deserialize)]
struct Frame {
    group: Option<CandidateKey>,
    remaining: Vec<CandidateKey>,
    attempts: u8,
    failure: Option<Failure>,
    ended: bool,
    expires_at: i64,
    authority: Option<ChoiceProof>,
}

#[derive(Serialize, Deserialize)]
struct Progress {
    frames: Vec<Frame>,
    tried: u8,
    reason: Option<String>,
    status: u16,
    retry_at: i64,
    skipped_targets: HashMap<String, Failure>,
    rejected_access: HashSet<String>,
}

pub(super) fn capture(
    frames: &[RouteFrame],
    tried: u8,
    reason: Option<&str>,
    status: StatusCode,
    retry: u64,
    skipped_targets: &HashMap<String, Failure>,
    rejected_access: &HashSet<String>,
) -> serde_json::Result<serde_json::Value> {
    let wall = chrono::Utc::now().timestamp_millis();
    let clock = Instant::now();
    serde_json::to_value(Progress {
        frames: frames
            .iter()
            .map(|frame| Frame {
                group: frame.group.clone(),
                remaining: frame
                    .candidates
                    .as_slice()
                    .iter()
                    .map(CandidateKey::of)
                    .collect(),
                attempts: frame.attempts,
                failure: frame.failure,
                ended: frame.ended,
                expires_at: wall
                    + frame.deadline.saturating_duration_since(clock).as_millis() as i64,
                authority: frame.choice.as_ref().map(|choice| choice.proof.clone()),
            })
            .collect(),
        tried,
        reason: reason.map(str::to_owned),
        status: status.as_u16(),
        retry_at: wall + retry as i64 * 1000,
        skipped_targets: skipped_targets.clone(),
        rejected_access: rejected_access.clone(),
    })
}

pub(super) struct Running {
    pub frames: Vec<RouteFrame>,
    pub tried: u8,
    pub reason: Option<String>,
    pub last: Option<Response>,
    pub skipped_targets: HashMap<String, Failure>,
    pub rejected_access: HashSet<String>,
}

fn invalid() -> ConfigError {
    ConfigError::new("invalid_record", "无法核对原路由进度，未发送备用请求")
}

pub(super) async fn restore(
    state: &AppState,
    plan: &std::sync::Arc<Dispatch>,
    checkpoint: &FallbackCheckpoint,
    root: Option<nested::Scope>,
) -> Result<Running, ConfigError> {
    let progress: Progress =
        serde_json::from_value(checkpoint.progress.clone()).map_err(|_| invalid())?;
    if !(1..=3).contains(&progress.frames.len())
        || !(1..=plan.policy.max_attempts).contains(&progress.tried)
    {
        return Err(invalid());
    }
    let mut frames: Vec<RouteFrame> = Vec::with_capacity(progress.frames.len());
    let mut source = Vec::new();
    let mut root = root;
    let wall = chrono::Utc::now().timestamp_millis();
    let clock = Instant::now();
    let depth_limit = progress.frames.len();
    for (depth, saved) in progress.frames.into_iter().enumerate() {
        let may_continue = !saved.ended
            && saved.expires_at > wall
            && (depth + 1 < depth_limit || !saved.remaining.is_empty())
            && frames.iter().all(|frame| !frame.ended && !frame.expired());
        let (scope, candidates, policy, deprioritized, slot) = match (&saved.group, frames.last()) {
            (None, None) => (
                root.take().ok_or_else(invalid)?,
                plan.candidates.clone(),
                plan.policy.clone(),
                plan.deprioritized.clone(),
                None,
            ),
            (Some(key), Some(parent)) => {
                let group = key.resolve(&source)?;
                let selection = group.service.as_ref().ok_or_else(invalid)?;
                let scope = nested::Scope::child(selection)?;
                let child = {
                    let state = state.clone();
                    let plan = plan.clone();
                    let selection = selection.clone();
                    crate::blocking::store(state.store.clone(), move |store| {
                        store
                            .service_saved_child_plan(&plan, &selection)?
                            .ok_or_else(invalid)
                    })
                    .await?
                };
                let parent_scope = parent.scope.as_ref().ok_or_else(invalid)?;
                let model = parent_scope.model_id(&group).ok_or_else(invalid)?;
                let slot = if may_continue && saved.attempts < child.policy.max_attempts {
                    group_slot(
                        &state.observation.policy,
                        parent_scope,
                        model,
                        &parent.policy,
                    )
                    .map_err(|message| ConfigError::new("route_capacity", message))?
                } else {
                    None
                };
                (
                    scope,
                    child.candidates,
                    child.policy,
                    child.deprioritized,
                    slot,
                )
            }
            _ => return Err(invalid()),
        };
        if saved.attempts > progress.tried || saved.attempts > policy.max_attempts {
            return Err(invalid());
        }
        let millis = saved
            .expires_at
            .saturating_sub(wall)
            .clamp(0, policy.deadline_seconds as i64 * 1000);
        let deadline = clock + Duration::from_millis(millis as u64);
        let choice = if may_continue && saved.attempts < policy.max_attempts {
            restore_choice(state, &scope, &policy, deadline, saved.authority).await?
        } else {
            None
        };
        let remaining = saved
            .remaining
            .iter()
            .map(|key| key.resolve(&candidates))
            .collect::<Result<Vec<_>, _>>()?;
        frames.push(RouteFrame {
            scope: Some(scope),
            policy,
            candidates: remaining.into_iter(),
            deprioritized,
            deadline,
            attempts: saved.attempts,
            failure: saved.failure,
            ended: saved.ended,
            choice,
            group_slot: slot,
            group: saved.group,
        });
        source = candidates;
    }
    let expired = frames[0].deadline <= clock;
    let status = if expired {
        StatusCode::GATEWAY_TIMEOUT
    } else {
        StatusCode::from_u16(progress.status).map_err(|_| invalid())?
    };
    let mut last = rejection(
        status,
        if expired {
            "原轮次的路由等待时间已用完"
        } else {
            "原轮次保留了上一候选的明确失败"
        },
    );
    if (status == StatusCode::TOO_MANY_REQUESTS
        || (status == StatusCode::PAYMENT_REQUIRED
            && progress.reason.as_deref() == Some("upstream_rate_limit")))
        && progress.retry_at > wall
        && let Ok(value) =
            HeaderValue::from_str(&((progress.retry_at - wall + 999) / 1000).to_string())
    {
        last.headers_mut().insert("retry-after", value);
    }
    Ok(Running {
        frames,
        tried: progress.tried,
        reason: progress.reason,
        last: Some(last),
        skipped_targets: progress.skipped_targets,
        rejected_access: progress.rejected_access,
    })
}

async fn restore_choice(
    state: &AppState,
    scope: &nested::Scope,
    policy: &local_agent_core::model_services::policy::RoutingPolicy,
    deadline: Instant,
    authority: Option<ChoiceProof>,
) -> Result<Option<super::PolicyChoice>, ConfigError> {
    let unavailable =
        || ConfigError::new("grant_revoked", "无法核对原路由策略授权，未发送备用请求");
    let service = &scope.reference.service_id;
    let Some(authority) = authority else {
        return if state.plugins.policy_needed(service) {
            Err(unavailable())
        } else {
            Ok(None)
        };
    };
    let id = {
        let plugins = state.plugins.clone();
        let service = service.clone();
        let enabled = policy.enabled;
        crate::blocking::store(state.store.clone(), move |store| {
            guard_publication(store, &plugins, &service, enabled)?;
            store.model_service_operation_store_id()
        })
        .await?
    };
    tokio::time::timeout_at(
        deadline,
        state.plugins.policy_resume_choice(
            id,
            service.clone(),
            scope.reference.version_id.clone(),
            authority,
        ),
    )
    .await
    .map_err(|_| unavailable())?
    .map(Some)
    .map_err(|_| unavailable())
}
