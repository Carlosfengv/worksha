use super::{same_published_target, unavailable};
use crate::{
    ApiResult, AppState,
    billing::{Billing, UpstreamBudget},
    json_value,
    observation::policy::{Cooldown, CooldownSnapshot, QuotaAdmission, quota_admission},
    providers,
};
use axum::{Json, extract::State};
use chrono::{DateTime, Utc};
use local_agent_core::{
    Provider, Store,
    model_services::{
        Capabilities, ChildServiceRef, ModelService,
        operations::{PreviewEntry, PreviewRequest},
        policy::{RouteDecisionReason, RoutingPolicy},
    },
    providers::{
        ConfigError, Verification, fingerprint, select_model,
        upstream_accounts::{Account, AccountRef},
        validate_source,
    },
};
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Preview {
    store_id: String,
    checked_at: String,
    entry_state: &'static str,
    models: Vec<Model>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Model {
    id: String,
    name: String,
    scope: &'static str,
    source: Option<String>,
    model: Option<String>,
    state: &'static str,
    accounts: Vec<AccountReview>,
    child: Option<Child>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Child {
    name: String,
    version_id: String,
    revision: u64,
    models: Vec<Model>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountReview {
    label: Option<String>,
    position: Option<usize>,
    state: &'static str,
    verification: &'static str,
    verified_at: Option<String>,
    verify_on_publish: bool,
    cooldown_known: bool,
    cooldown: Option<Cooldown>,
    recovery: Option<Cooldown>,
    quota: Option<&'static str>,
}

struct Target<'a> {
    id: &'a str,
    name: &'a str,
    provider_id: &'a str,
    model: &'a str,
    child: Option<&'a ChildServiceRef>,
    capabilities: &'a Capabilities,
    frozen: Option<&'a Provider>,
}

struct Scope<'a> {
    policy: &'a RoutingPolicy,
    primary: &'a str,
    preferred: Vec<&'a str>,
    required: &'a Capabilities,
}

impl Scope<'_> {
    fn position(&self, target: &Target<'_>) -> &'static str {
        if self.preferred.is_empty() {
            return "entryUnavailable";
        }
        match self.policy.candidate_exclusion(target.id, self.primary) {
            Some(RouteDecisionReason::DisabledModel) => return "disabled",
            Some(_) => return "routingOff",
            None => {}
        }
        if !target.capabilities.covers(self.required) {
            "capabilityMismatch"
        } else if self.preferred.contains(&target.id) {
            "preferred"
        } else if self
            .preferred
            .iter()
            .any(|preferred| self.policy.in_fallback_scope(preferred, target.id))
        {
            "fallback"
        } else {
            "outsideFallback"
        }
    }
}

struct Reader<'a> {
    store: &'a Store,
    sources: &'a [Provider],
    records: Option<&'a [Verification]>,
    cooldowns: Option<&'a CooldownSnapshot>,
    billing: Option<&'a Billing>,
    previous: Option<&'a ModelService>,
    now: DateTime<Utc>,
    path: Vec<String>,
    remaining: usize,
    accounts: BTreeMap<String, Option<Vec<Account>>>,
}

impl Reader<'_> {
    fn models(&mut self, mut targets: Vec<Target<'_>>, scope: &Scope<'_>) -> Vec<Model> {
        targets.sort_by_key(|target| {
            (
                target.id != scope.primary,
                scope
                    .policy
                    .order
                    .iter()
                    .position(|id| id == target.id)
                    .unwrap_or(usize::MAX),
            )
        });
        targets
            .into_iter()
            .map(|target| self.model(target, scope))
            .collect()
    }

    fn model(&mut self, target: Target<'_>, scope: &Scope<'_>) -> Model {
        let mut result = Model {
            id: target.id.into(),
            name: target.name.into(),
            scope: scope.position(&target),
            source: None,
            model: (!target.model.is_empty()).then(|| target.model.into()),
            state: "configured",
            accounts: Vec::new(),
            child: None,
        };
        if let Some(child) = target.child {
            if self.path.len() >= 3 || self.path.contains(&child.service_id) || self.remaining == 0
            {
                self.remaining = self.remaining.saturating_sub(1);
                result.state = "childLimit";
                return result;
            }
            let Ok(published) = self
                .store
                .model_service(&child.service_id)
                .and_then(|_| self.store.model_service_child(child))
            else {
                self.remaining -= 1;
                result.state = "childUnavailable";
                return result;
            };
            let policy = &published.routing_policy;
            let eligible_rule = |model_id: &str| {
                published.models.iter().any(|model| {
                    model.id == model_id
                        && policy
                            .candidate_exclusion(model_id, &published.default_model_id)
                            .is_none()
                        && model.capabilities.covers(scope.required)
                })
            };
            let child_scope = Scope {
                policy,
                primary: &published.default_model_id,
                preferred: std::iter::once(published.default_model_id.as_str())
                    .chain(
                        policy
                            .rules
                            .iter()
                            .filter(|rule| policy.enabled && eligible_rule(&rule.model_id))
                            .map(|rule| rule.model_id.as_str()),
                    )
                    .collect(),
                required: scope.required,
            };
            self.path.push(published.id.clone());
            let models = self.models(
                published
                    .models
                    .iter()
                    .map(|model| Target {
                        id: &model.id,
                        name: &model.name,
                        provider_id: &model.provider.id,
                        model: &model.provider.model,
                        child: model.child.as_ref(),
                        capabilities: &model.capabilities,
                        frozen: Some(&model.provider),
                    })
                    .collect(),
                &child_scope,
            );
            self.path.pop();
            if *target.capabilities != Capabilities::common(&published.models) {
                result.state = "childCapabilitiesChanged";
            }
            result.model = None;
            result.child = Some(Child {
                name: published.name,
                version_id: published.version_id,
                revision: published.revision,
                models,
            });
            return result;
        }

        let pool = scope.policy.accounts(target.id);
        if pool.len() > 8 || pool.len().max(1) > self.remaining {
            self.remaining = 0;
            result.state = "candidateLimit";
            return result;
        }
        self.remaining -= pool.len().max(1);
        let current = self
            .sources
            .iter()
            .find(|source| source.id == target.provider_id);
        result.source = target.frozen.or(current).map(|source| source.name.clone());
        let configured = current.filter(|source| source.configured);
        let selected = configured.and_then(|source| select_model(source, Some(target.model)).ok());
        result.state = if let Some(frozen) = target.frozen {
            if selected
                .as_ref()
                .is_some_and(|selected| fingerprint(selected) == fingerprint(frozen))
            {
                "configured"
            } else {
                "sourceChanged"
            }
        } else if current.is_none() {
            "providerMissing"
        } else if configured.is_none() {
            "providerUnconfigured"
        } else if target.model.is_empty() {
            "modelMissing"
        } else if selected.is_none() {
            "modelUnavailable"
        } else if pool.is_empty()
            && selected
                .as_ref()
                .is_some_and(|target| validate_source(target).is_err())
        {
            "invalidSource"
        } else {
            "configured"
        };
        let base = target.frozen.or(selected.as_ref());
        let unchanged = base.is_some_and(|selected| {
            same_published_target(self.previous, target.id, target.capabilities, selected)
        });
        let old_pool = self.previous.map_or(&[][..], |service| {
            service.routing_policy.accounts(target.id)
        });
        let uses_quota = scope
            .policy
            .pool(target.id)
            .is_some_and(|pool| pool.strategy.uses_quota());
        result.accounts = if pool.is_empty() {
            vec![self.account(
                base,
                None,
                target.frozen.is_none() && (!unchanged || !old_pool.is_empty()),
                false,
            )]
        } else {
            let store = self.store;
            self.accounts
                .entry(target.provider_id.to_owned())
                .or_insert_with(|| store.upstream_accounts(target.provider_id).ok());
            let list = self
                .accounts
                .get(target.provider_id)
                .and_then(Option::as_ref);
            pool.iter()
                .enumerate()
                .map(|(index, reference)| {
                    let resolved =
                        list.map(|accounts| accounts.iter().find(|a| a.id == reference.id));
                    let mut account = self.account(
                        base,
                        Some((reference, resolved)),
                        target.frozen.is_none() && (!unchanged || !old_pool.contains(reference)),
                        uses_quota,
                    );
                    account.position = Some(index + 1);
                    account
                })
                .collect()
        };
        result
    }

    fn account(
        &self,
        source: Option<&Provider>,
        account: Option<(&AccountRef, Option<Option<&Account>>)>,
        verify_on_publish: bool,
        uses_quota: bool,
    ) -> AccountReview {
        let mut review = AccountReview {
            label: None,
            position: None,
            state: "configured",
            verification: "unknown",
            verified_at: None,
            verify_on_publish: false,
            cooldown_known: false,
            cooldown: None,
            recovery: None,
            quota: None,
        };
        let resolved = match account {
            Some((reference, lookup)) => {
                let account = lookup.flatten();
                review.label = account.map(|account| account.label.clone());
                review.state = match account {
                    None if lookup.is_none() => "unknown",
                    None => "missing",
                    Some(account) if account.state == "revoked" => "revoked",
                    Some(account) if account.revision != reference.revision => "changed",
                    Some(account) if account.state == "paused" => "paused",
                    Some(_) if source.is_none() => "sourceUnavailable",
                    Some(_) => "configured",
                };
                if review.state != "configured" {
                    return review;
                }
                let Some(source) = source else { return review };
                let Ok((target, saved)) = self.store.upstream_account_target(source, reference)
                else {
                    review.state = "sourceMismatch";
                    return review;
                };
                Some((target, Some(saved)))
            }
            None => source
                .cloned()
                .filter(|source| validate_source(source).is_ok())
                .map(|target| (target, None)),
        };
        let Some((target, account)) = resolved else {
            review.state = "sourceUnavailable";
            return review;
        };
        let reference = account.as_ref().map(Account::reference);
        if reference.is_some() || crate::billing::route_budget_supported(&target) {
            let budget = self
                .billing
                .map_or_else(UpstreamBudget::unavailable, |billing| {
                    billing.route_budget(self.store, &target, self.now)
                });
            review.quota = match quota_admission(self.store, &target, &budget) {
                Some(QuotaAdmission::Open) => self
                    .billing
                    .and_then(|billing| {
                        billing.upstream_quota(&target, reference.as_ref(), self.now)
                    })
                    .map(|_| "reported")
                    .or(uses_quota.then_some("unknown")),
                Some(admission) => admission.display_key(),
                None => Some("unknown"),
            };
        }
        review.verify_on_publish = verify_on_publish;
        if let Some(records) = self.records {
            let id = account
                .as_ref()
                .map_or(target.id.as_str(), |account| account.id.as_str());
            let record = records
                .iter()
                .rev()
                .find(|record| {
                    record.provider_id == id
                        && record.model.as_deref() == Some(target.model.as_str())
                })
                .or_else(|| {
                    account
                        .is_none()
                        .then(|| {
                            records.iter().rev().find(|record| {
                                record.provider_id == id
                                    && record.model.is_none()
                                    && record.fingerprint == fingerprint(&target)
                            })
                        })
                        .flatten()
                });
            if let Some(record) = record {
                let matches_target = record.account_revision
                    == account.as_ref().map(|a| a.revision)
                    && record.fingerprint == fingerprint(&target);
                let (state, checked) = providers::upstream_accounts::verification_state(
                    record,
                    matches_target,
                    self.now,
                );
                review.verification = state;
                review.verified_at = checked.map(|_| record.checked_at.clone());
            } else {
                review.verification = "unverified";
            }
        }
        if let Some(cooldowns) = self.cooldowns {
            review.cooldown_known = true;
            if let Some(status) = cooldowns.target(&target) {
                if status.state.is_some() {
                    review.recovery = Some(status);
                } else {
                    review.cooldown = Some(status);
                }
            }
        }
        review
    }
}

pub(crate) async fn read(
    State(state): State<AppState>,
    Json(input): Json<PreviewRequest>,
) -> ApiResult {
    crate::blocking::query(state.query_capacity.clone(), move || {
        let PreviewRequest {
            store_id,
            draft,
            entry,
        } = input;
        let ids: HashSet<_> = draft.models.iter().map(|model| &model.id).collect();
        if draft.models.is_empty()
            || draft.models.len() > 32
            || ids.len() != draft.models.len()
            || uuid::Uuid::parse_str(&draft.id).is_err()
        {
            return Err(providers::error(ConfigError::new(
                "invalid_input",
                "模型服务草稿范围无效",
            )));
        }
        let records = state
            .providers
            .lock()
            .ok()
            .and_then(|manager| manager.verifications().ok());
        let store = state.store.lock().map_err(|_| unavailable())?;
        store
            .require_model_service_store(&store_id)
            .map_err(providers::error)?;
        let previous = match store.model_service(&draft.id) {
            Ok(service) if service.revision == draft.revision => Some(service),
            Err(error) if error.code == "not_found" && draft.revision == 0 => None,
            Err(error) if error.code != "not_found" => return Err(providers::error(error)),
            _ => {
                return Err(providers::error(ConfigError::new(
                    "conflict",
                    "模型服务版本已变化，请重新打开草稿",
                )));
            }
        };
        let sources = store.providers().map_err(|_| unavailable())?;
        let cooldowns = CooldownSnapshot::read(&state, &store);
        let billing = state.billing.lock().ok();
        let now = Utc::now();
        let mut reader = Reader {
            store: &store,
            sources: &sources,
            records: records.as_deref(),
            cooldowns: cooldowns.as_ref(),
            billing: billing.as_deref(),
            previous: previous.as_ref(),
            now,
            path: vec![draft.id.clone()],
            remaining: 128,
            accounts: BTreeMap::new(),
        };
        let policy = &draft.routing_policy;
        let (primary, entry_state) = match &entry {
            PreviewEntry::Default => (Some(draft.default_model_id.as_str()), "configured"),
            PreviewEntry::Specified { model_id } => (Some(model_id.as_str()), "configured"),
            PreviewEntry::Rule { rule_id } => {
                let rule = policy.rules.iter().find(|rule| {
                    policy.enabled
                        && &rule.id == rule_id
                        && !policy.disabled_models.contains(&rule.model_id)
                });
                (
                    rule.map(|rule| rule.model_id.as_str()),
                    if rule.is_some() {
                        "configured"
                    } else {
                        "ruleUnavailable"
                    },
                )
            }
        };
        let selected = primary.and_then(|id| draft.models.iter().find(|model| model.id == id));
        let required = match (&entry, selected) {
            (PreviewEntry::Specified { .. }, Some(model)) => model.capabilities.clone(),
            _ => Capabilities::common_of(draft.models.iter().map(|model| &model.capabilities)),
        };
        let scope = Scope {
            policy,
            primary: selected.map_or("", |model| model.id.as_str()),
            preferred: selected
                .into_iter()
                .map(|model| model.id.as_str())
                .collect(),
            required: &required,
        };
        let models = reader.models(
            draft
                .models
                .iter()
                .map(|model| Target {
                    id: &model.id,
                    name: &model.name,
                    provider_id: &model.provider_id,
                    model: &model.model,
                    child: model.child.as_ref(),
                    capabilities: &model.capabilities,
                    frozen: None,
                })
                .collect(),
            &scope,
        );
        json_value(Preview {
            store_id,
            checked_at: now.to_rfc3339(),
            entry_state: if selected.is_none() && entry_state == "configured" {
                "entryUnavailable"
            } else {
                entry_state
            },
            models,
        })
    })
    .await
}
