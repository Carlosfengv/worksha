use super::{
    Reason, SendAuthority, account_planning_hint, guard_publication, health::pool_health, intent,
    rule_project, trace::Journal,
};
use crate::{
    AppState,
    plugin_host::external::{self, policy::Choice},
};
use local_agent_core::{
    model_services::{
        ChildServiceRef, ServiceSelection,
        policy::{ChildDispatch, Dispatch, IntentTurn, MatchedRule, RoutingPolicy, RuleFacts},
    },
    providers::ConfigError,
    routing::Selection,
};
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::time::Instant;

#[derive(Clone)]
pub(super) struct Scope {
    pub reference: ChildServiceRef,
    pub depth: usize,
    pub preferred: Option<String>,
}

impl Scope {
    pub(super) fn root(selection: &ServiceSelection) -> Self {
        Self {
            reference: ChildServiceRef {
                service_id: selection.service_id.clone(),
                version_id: selection.service_version_id.clone(),
            },
            depth: 0,
            preferred: selection.preferred_model_id.clone(),
        }
    }

    pub(super) fn child(selection: &ServiceSelection) -> Result<Self, ConfigError> {
        Ok(Self {
            reference: selection.pending_child.clone().ok_or_else(invalid)?,
            depth: selection.nested_route.len() + 1,
            preferred: None,
        })
    }

    pub(super) fn model_id<'a>(&self, selection: &'a Selection) -> Option<&'a str> {
        let service = selection.service.as_ref()?;
        if self.depth == 0 {
            return (service.service_id == self.reference.service_id
                && service.service_version_id == self.reference.version_id)
                .then_some(service.model_id.as_str());
        }
        let hop = service.nested_route.get(self.depth - 1)?;
        (hop.service_id == self.reference.service_id && hop.version_id == self.reference.version_id)
            .then_some(hop.model_id.as_str())
    }

    fn rule<'a>(&self, selection: &'a Selection) -> Option<&'a MatchedRule> {
        let service = selection.service.as_ref()?;
        if self.depth == 0 {
            service.routing_rule.as_ref()
        } else {
            service
                .nested_route
                .get(self.depth - 1)?
                .routing_rule
                .as_ref()
        }
    }
}

pub(super) async fn rank(
    state: &AppState,
    scope: &Scope,
    candidates: &mut Vec<Selection>,
    policy: &RoutingPolicy,
    deadline: Instant,
    journal: &mut Journal,
) -> external::Result<Option<Choice>> {
    if !state.plugins.policy_needed(&scope.reference.service_id) {
        return Ok(None);
    }
    let (id, request) = {
        let scope = scope.clone();
        let candidates = candidates.clone();
        crate::blocking::store(state.store.clone(), move |store| {
            use local_agent_core::domain::routing_policy::{
                Candidate, CandidateHealth, RankRequest,
            };
            let published = store.model_service_child(&scope.reference)?;
            let ids: HashSet<_> = candidates
                .iter()
                .filter_map(|c| scope.model_id(c))
                .collect();
            if candidates
                .iter()
                .any(|candidate| scope.model_id(candidate).is_none())
            {
                return Err(ConfigError::new("invalid_record", "候选服务版本不一致"));
            }
            let preferred = candidates
                .first()
                .and_then(|c| scope.rule(c))
                .map(|rule| rule.model_id.clone())
                .or(scope.preferred)
                .or_else(|| Some(published.default_model_id.clone()));
            let request = RankRequest {
                service_id: published.id,
                service_version_id: published.version_id,
                preferred_model_id: preferred.filter(|id| ids.contains(id.as_str())),
                default_model_id: ids
                    .contains(published.default_model_id.as_str())
                    .then_some(published.default_model_id),
                configured_order: published
                    .routing_policy
                    .order
                    .iter()
                    .filter(|id| ids.contains(id.as_str()))
                    .cloned()
                    .collect(),
                candidates: published
                    .models
                    .iter()
                    .filter(|model| ids.contains(model.id.as_str()))
                    .map(|model| Candidate {
                        model_id: model.id.clone(),
                        capabilities: (&model.capabilities).into(),
                        health: if model.child.is_some() {
                            CandidateHealth::Unknown
                        } else {
                            match pool_health(
                                store,
                                &model.provider,
                                published.routing_policy.accounts(&model.id),
                                published.routing_policy.fallback_on_access_denied,
                            )["state"]
                                .as_str()
                            {
                                Some("cooling") => CandidateHealth::Cooling,
                                Some("ready") => CandidateHealth::Available,
                                _ => CandidateHealth::Unknown,
                            }
                        },
                    })
                    .collect(),
            };
            Ok((store.model_service_operation_store_id()?, request))
        })
        .await
        .map_err(|_| external::fail("unavailable"))?
    };
    let ranked = tokio::time::timeout_at(deadline, state.plugins.policy_rank(id, request))
        .await
        .map_err(|_| external::fail("deadline_exceeded"))??;
    if let Some(order) = &ranked.order {
        let previous = std::mem::take(candidates);
        journal.remaining(
            previous.iter().filter(|c| {
                scope
                    .model_id(c)
                    .is_some_and(|id| !order.iter().any(|allowed| allowed == id))
            }),
            policy,
            Reason::PolicyExcluded,
        );
        let rule_target = previous
            .first()
            .and_then(|s| scope.rule(s))
            .map(|rule| &rule.model_id)
            .filter(|id| order.contains(id));
        *candidates = rule_target
            .into_iter()
            .chain(order.iter().filter(|id| Some(*id) != rule_target))
            .flat_map(|id| {
                previous
                    .iter()
                    .filter(move |candidate| scope.model_id(candidate) == Some(id.as_str()))
                    .cloned()
            })
            .collect();
    }
    Ok(Some(ranked))
}

pub(super) struct PlanningFacts {
    pub facts: RuleFacts,
    // A trusted fresh default turn owns one excerpt for its entire routing tree.
    pub excerpt: Option<String>,
}

pub(super) async fn open(
    state: &AppState,
    root: Arc<Dispatch>,
    group: Selection,
    inputs: &mut PlanningFacts,
    deadline: Instant,
    ancestors: SendAuthority,
    journal: &mut Journal,
) -> Result<(Scope, ChildDispatch, Instant, Option<Choice>), ConfigError> {
    let opened = Instant::now();
    let selection = group.service.as_ref().ok_or_else(invalid)?;
    let scope = Scope::child(selection)?;
    let reference = scope.reference.clone();
    let (published, saved) = {
        let state = state.clone();
        let root = root.clone();
        let selection = selection.clone();
        crate::blocking::store(state.store.clone(), move |store| {
            let published = store.model_service_child(&reference)?;
            guard_publication(
                store,
                &state.plugins,
                &published.id,
                published.routing_policy.enabled,
            )?;
            Ok((
                published,
                store.service_saved_child_plan(&root, &selection)?,
            ))
        })
        .await?
    };
    let deadline =
        deadline.min(opened + Duration::from_secs(published.routing_policy.deadline_seconds));
    let mut child = if let Some(saved) = saved {
        saved
    } else {
        let mut facts = inputs.facts.clone();
        if published
            .routing_policy
            .rules
            .iter()
            .any(|rule| rule.project_id.is_some())
        {
            let state = state.clone();
            let session = root.session.clone();
            let agent = facts.agent.clone();
            facts.project_id = crate::blocking::store(state.store.clone(), move |store| {
                Ok(rule_project(store, &state.sessions, Some(&session), &agent))
            })
            .await?;
        }
        if selection.mode == "default"
            && let Some((classifier, labels)) = intent::prepare(&published, selection, &facts)
        {
            facts.intent = tokio::time::timeout_at(
                deadline,
                intent::classify(
                    state,
                    &state.observation.policy,
                    classifier.with_ancestors(ancestors),
                    labels,
                    inputs.excerpt.take(),
                    IntentTurn {
                        binding: root.binding.clone(),
                        session: root.session.clone(),
                        digest: root.turn.clone(),
                    },
                ),
            )
            .await
            .ok()
            .flatten();
        }
        let billing = state.billing.clone();
        let runtime = state.observation.policy.clone();
        crate::blocking::store(state.store.clone(), move |store| {
            let now = chrono::Utc::now();
            store.service_child_dispatch(
                &root,
                &group,
                |_| facts,
                |target, reference, limit| {
                    account_planning_hint(store, &billing, &runtime, target, reference, limit, now)
                },
            )
        })
        .await?
    };
    journal.extend(child.skipped.iter().cloned());
    let choice = match rank(
        state,
        &scope,
        &mut child.candidates,
        &child.policy,
        deadline,
        journal,
    )
    .await
    {
        Ok(choice) => choice,
        Err(error) => {
            journal.remaining(&child.candidates, &child.policy, Reason::PolicyUnavailable);
            return Err(ConfigError::new(error.code, "子服务的外部排序不可用"));
        }
    };
    Ok((scope, child, deadline, choice))
}

pub(super) fn pinned_scopes(
    store: &local_agent_core::Store,
    selection: &ServiceSelection,
) -> Result<Vec<(Scope, String, RoutingPolicy)>, ConfigError> {
    let mut scope = Scope::root(selection);
    let mut model_id = selection.model_id.clone();
    let mut result = Vec::new();
    for hop in selection
        .nested_route
        .iter()
        .map(Some)
        .chain(std::iter::once(None))
    {
        let published = store.model_service_child(&scope.reference)?;
        let model = published
            .models
            .iter()
            .find(|model| model.id == model_id)
            .ok_or_else(invalid)?;
        let child = model.child.clone();
        result.push((scope, model_id, published.routing_policy));
        match (child, hop) {
            (Some(child), Some(hop))
                if child.service_id == hop.service_id && child.version_id == hop.version_id =>
            {
                scope = Scope {
                    reference: child,
                    depth: result.len(),
                    preferred: None,
                };
                model_id = hop.model_id.clone();
            }
            (None, None) => return Ok(result),
            _ => return Err(invalid()),
        }
    }
    Err(invalid())
}

fn invalid() -> ConfigError {
    ConfigError::new("invalid_child_service", "本轮子服务路由无法核查")
}
