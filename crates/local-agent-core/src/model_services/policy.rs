//! Service policy snapshots and durable dispatch ownership. No request bodies.
use super::{ModelService, NestedHop, ServiceSelection, error, nested, version};
use crate::{
    Store,
    domain::routing_policy::{
        BuiltinPriority, Candidate, CandidateHealth, RankRequest, RoutingPolicy as PolicyAdapter,
    },
    providers::{ConfigError, fail, upstream_accounts::AccountRef},
    routing::{Selection, Turn, UncommittedDispatch},
};
use rusqlite::{Connection, OptionalExtension, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

mod account_order;
mod cooldown_probes;
mod minimax_allowance;
mod openrouter_rejections;
mod quota;
mod quota_probes;
mod rules;
mod trace;
mod upstream_balance;
pub use account_order::{
    AccountAffinity, AccountPlanningHint, AccountStrategy, access_scopes, scopes,
};
pub use cooldown_probes::HttpProbeClaim;
pub use openrouter_rejections::{OpenRouterBudgetRejection, OpenRouterBudgetScope};
pub use quota::AccountQuotaHint;
pub use quota_probes::{QuotaProbeClaim, QuotaProbeSource, QuotaProbeState};
pub use rules::{IntentClassifier, IntentOutcome, MatchedRule, RouteRule, RuleFacts};
pub use trace::{RouteDecision, RouteDecisionReason, RouteTrace};
pub use upstream_balance::{BalanceRejection, BalanceState};

const MAX_ROUTE_DEADLINE_SECONDS: u64 = 300;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct ModelLimit {
    pub model_id: String,
    pub max_concurrent: u32,
    pub requests_per_minute: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountPool {
    pub model_id: String,
    pub accounts: Vec<AccountRef>,
    #[serde(default, skip_serializing_if = "AccountStrategy::is_priority")]
    pub strategy: AccountStrategy,
    #[serde(default, skip_serializing_if = "AccountAffinity::is_off")]
    pub affinity: AccountAffinity,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub weights: std::collections::BTreeMap<String, u16>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct RoutingPolicy {
    pub enabled: bool,
    pub order: Vec<String>,
    pub disabled_models: Vec<String>,
    pub fallback_on_failure: bool,
    pub fallback_on_rate_limit: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub fallback_on_access_denied: bool,
    pub max_attempts: u8,
    pub deadline_seconds: u64,
    pub cooldown_seconds: u64,
    pub limits: Vec<ModelLimit>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<RouteRule>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent_classifier: Option<IntentClassifier>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub account_pools: Vec<AccountPool>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub fallback_across_models: bool,
}
impl Default for RoutingPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            order: vec![],
            disabled_models: vec![],
            fallback_on_failure: true,
            fallback_on_rate_limit: true,
            fallback_on_access_denied: false,
            max_attempts: 3,
            deadline_seconds: 120,
            cooldown_seconds: 30,
            limits: vec![],
            rules: vec![],
            intent_classifier: None,
            account_pools: vec![],
            fallback_across_models: false,
        }
    }
}
impl RoutingPolicy {
    pub fn candidate_exclusion(&self, model: &str, default: &str) -> Option<RouteDecisionReason> {
        if self.enabled && self.disabled_models.iter().any(|id| id == model) {
            Some(RouteDecisionReason::DisabledModel)
        } else if !self.enabled && model != default {
            Some(RouteDecisionReason::RoutingDisabled)
        } else {
            None
        }
    }

    pub fn in_fallback_scope(&self, preferred: &str, candidate: &str) -> bool {
        self.account_pools.is_empty() || self.fallback_across_models || preferred == candidate
    }

    pub fn validate(&self, ids: &HashSet<String>, default: &str) -> Result<(), ConfigError> {
        rules::validate(&self.rules, ids)?;
        if let Some(classifier) = &self.intent_classifier {
            let accounts = self.accounts(&classifier.model_id);
            if !ids.contains(&classifier.model_id)
                || self.disabled_models.contains(&classifier.model_id)
                || classifier
                    .account
                    .as_ref()
                    .map_or(!accounts.is_empty(), |account| !accounts.contains(account))
            {
                return fail("invalid_policy", "意图分类须选择已启用的服务模型及有效账号");
            }
        }
        let order: HashSet<_> = self.order.iter().cloned().collect();
        let disabled: HashSet<_> = self.disabled_models.iter().cloned().collect();
        let limits: HashSet<_> = self.limits.iter().map(|m| m.model_id.clone()).collect();
        let mut pooled_models = HashSet::new();
        if (!self.enabled && !self.account_pools.is_empty())
            || self
                .account_pools
                .iter()
                .map(|p| p.accounts.len())
                .sum::<usize>()
                > 64
            || self.account_pools.iter().any(|pool| {
                let mut accounts = HashSet::new();
                !ids.contains(&pool.model_id)
                    || !pooled_models.insert(&pool.model_id)
                    || !(1..=8).contains(&pool.accounts.len())
                    || (pool.strategy != AccountStrategy::Weighted && !pool.weights.is_empty())
                    || pool.weights.iter().any(|(id, weight)| {
                        !(1..=1000).contains(weight) || !pool.accounts.iter().any(|a| &a.id == id)
                    })
                    || pool.accounts.iter().any(|a| {
                        a.revision == 0
                            || !crate::providers::operations::valid_id(&a.id)
                            || !accounts.insert(&a.id)
                    })
            })
        {
            return fail(
                "invalid_policy",
                "账号池须启用路由策略，且每个模型最多选择 8 个不同账号",
            );
        }
        if !(1..=5).contains(&self.max_attempts)
            || !(5..=MAX_ROUTE_DEADLINE_SECONDS).contains(&self.deadline_seconds)
            || !(1..=3600).contains(&self.cooldown_seconds)
            || (!self.order.is_empty()
                && (order != *ids || self.order.len() != ids.len() || self.order[0] != default))
            || !disabled.is_subset(ids)
            || disabled.len() != self.disabled_models.len()
            || disabled.len() == ids.len()
            || !limits.is_subset(ids)
            || limits.len() != self.limits.len()
            || self
                .limits
                .iter()
                .any(|m| m.max_concurrent > 1024 || m.requests_per_minute > 1_000_000)
        {
            return fail(
                "invalid_policy",
                "路由顺序须完整且默认模型排首位；至少启用一个模型，尝试 1–5 次、总等待 5–300 秒、冷却 1–3600 秒",
            );
        }
        Ok(())
    }
    pub fn limit(&self, id: &str) -> ModelLimit {
        self.limits
            .iter()
            .find(|l| l.model_id == id)
            .cloned()
            .unwrap_or_default()
    }
    pub fn accounts(&self, model_id: &str) -> &[AccountRef] {
        self.pool(model_id)
            .map_or(&[], |pool| pool.accounts.as_slice())
    }
    pub fn pool(&self, model_id: &str) -> Option<&AccountPool> {
        self.account_pools
            .iter()
            .find(|pool| pool.model_id == model_id)
    }
}

impl super::ModelService {
    pub fn intent_labels(&self, required: &super::Capabilities, facts: &RuleFacts) -> Vec<String> {
        let mut labels = Vec::new();
        for rule in &self.routing_policy.rules {
            if !rule.matches_metadata(facts)
                || self.routing_policy.disabled_models.contains(&rule.model_id)
                || !self
                    .models
                    .iter()
                    .any(|model| model.id == rule.model_id && model.capabilities.covers(required))
            {
                continue;
            }
            let Some(label) = &rule.intent else { break };
            if !labels.contains(label) {
                labels.push(label.clone());
            }
        }
        labels
    }
}

pub(crate) fn initialize(db: &Connection) -> Result<(), ConfigError> {
    db.execute_batch("BEGIN IMMEDIATE;
      CREATE TABLE IF NOT EXISTS service_route_dispatch(binding TEXT NOT NULL,session TEXT NOT NULL,turn TEXT NOT NULL,owner TEXT NOT NULL,state TEXT NOT NULL CHECK(state IN('pending','committed','failed')),value TEXT NOT NULL CHECK(json_valid(value)),retry_at INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(binding,session,turn));
      CREATE TABLE IF NOT EXISTS service_route_attempts(id INTEGER PRIMARY KEY,binding TEXT NOT NULL,session TEXT NOT NULL,turn TEXT NOT NULL,route TEXT NOT NULL,model TEXT NOT NULL,status INTEGER,outcome TEXT NOT NULL,created TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS service_route_intents(binding TEXT NOT NULL,session TEXT NOT NULL,turn TEXT NOT NULL,classifier_scope TEXT,outcome TEXT CHECK(outcome IS NULL OR (classifier_scope IS NOT NULL AND json_valid(outcome) AND length(CAST(outcome AS BLOB))<=256)),PRIMARY KEY(binding,session,turn),FOREIGN KEY(binding,session,turn) REFERENCES gateway_route_leases(binding,session,turn) ON DELETE CASCADE);
      CREATE TABLE IF NOT EXISTS service_route_cooldowns(scope TEXT PRIMARY KEY,until_ms INTEGER NOT NULL,reason TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS service_cooldown_probe_sends(
        scope TEXT PRIMARY KEY REFERENCES service_route_cooldowns(scope) ON DELETE CASCADE,
        owner TEXT NOT NULL,checked_at INTEGER NOT NULL,cooldown_ms INTEGER NOT NULL CHECK(cooldown_ms BETWEEN 1000 AND 3600000));
      CREATE INDEX IF NOT EXISTS service_cooldown_probe_send_owner ON service_cooldown_probe_sends(owner);
      CREATE TABLE IF NOT EXISTS service_account_rotation(version TEXT NOT NULL REFERENCES model_service_versions(id),model TEXT NOT NULL,scores TEXT NOT NULL CHECK(json_valid(scores)),PRIMARY KEY(version,model));
      CREATE TABLE IF NOT EXISTS service_openrouter_admission_v1(id INTEGER PRIMARY KEY CHECK(id=1),composition_revision INTEGER NOT NULL,value TEXT NOT NULL CHECK(length(CAST(value AS BLOB))<=1048576 AND json_valid(value)));
      CREATE TABLE IF NOT EXISTS service_openrouter_reply_budget(credential TEXT NOT NULL,scope TEXT NOT NULL,observed_at INTEGER NOT NULL,exhausted INTEGER NOT NULL CHECK(exhausted IN(0,1)),reset_at INTEGER,PRIMARY KEY(credential,scope));
      CREATE TABLE IF NOT EXISTS service_upstream_balance(credential TEXT PRIMARY KEY,observed_at INTEGER NOT NULL,exhausted INTEGER NOT NULL CHECK(exhausted IN(0,1)),recovery_at INTEGER);
      CREATE TABLE IF NOT EXISTS service_balance_probes(scope TEXT PRIMARY KEY,composition_revision INTEGER NOT NULL,owner TEXT NOT NULL,reset_at INTEGER,state TEXT NOT NULL CHECK(state IN('active','recovered','cooling')),valid_until INTEGER NOT NULL,until_ms INTEGER NOT NULL,cooldown_ms INTEGER NOT NULL);
      CREATE INDEX IF NOT EXISTS service_balance_probe_owner ON service_balance_probes(owner);
      CREATE TABLE IF NOT EXISTS service_openrouter_probes(scope TEXT PRIMARY KEY,composition_revision INTEGER NOT NULL,owner TEXT NOT NULL,reset_at INTEGER,state TEXT NOT NULL CHECK(state IN('active','recovered','cooling')),valid_until INTEGER NOT NULL,until_ms INTEGER NOT NULL,cooldown_ms INTEGER NOT NULL);
      CREATE INDEX IF NOT EXISTS service_openrouter_probe_owner ON service_openrouter_probes(owner);
      CREATE TABLE IF NOT EXISTS service_minimax_allowance(scope TEXT NOT NULL,window TEXT NOT NULL,observed_at INTEGER NOT NULL,exhausted INTEGER NOT NULL CHECK(exhausted IN(0,1)),reset_at INTEGER,PRIMARY KEY(scope,window));
      CREATE TABLE IF NOT EXISTS service_minimax_probes(scope TEXT PRIMARY KEY,composition_revision INTEGER NOT NULL,owner TEXT NOT NULL,reset_at INTEGER,state TEXT NOT NULL CHECK(state IN('active','recovered','cooling')),valid_until INTEGER NOT NULL,until_ms INTEGER NOT NULL,cooldown_ms INTEGER NOT NULL);
      CREATE INDEX IF NOT EXISTS service_minimax_probe_owner ON service_minimax_probes(owner);
      COMMIT;").map_err(error)
}

#[derive(Clone)]
pub struct IntentTurn {
    pub binding: String,
    pub session: String,
    pub digest: String,
}

pub enum IntentReceipt {
    Unused,
    Recorded(IntentOutcome),
    Unavailable,
}

impl Store {
    /// Only a lease created with an unused slot can start a classifier request.
    pub fn service_intent_outcome(
        &self,
        turn: &IntentTurn,
        scope: &str,
    ) -> Result<IntentReceipt, ConfigError> {
        let saved: Option<(Option<String>, Option<String>)> = self
            .connection
            .query_row(
                "SELECT classifier_scope,outcome FROM service_route_intents
             WHERE binding=?1 AND session=?2 AND turn=?3",
                params![turn.binding, turn.session, turn.digest],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(error)?;
        match saved {
            Some((None, None)) => Ok(IntentReceipt::Unused),
            Some((Some(saved_scope), outcome)) if saved_scope == scope => {
                Ok(IntentReceipt::Recorded(
                    outcome
                        .as_deref()
                        .map(serde_json::from_str)
                        .transpose()
                        .map_err(error)?
                        .unwrap_or(IntentOutcome::Unavailable),
                ))
            }
            _ => Ok(IntentReceipt::Unavailable),
        }
    }

    pub fn service_claim_intent(
        &self,
        turn: &IntentTurn,
        scope: &str,
    ) -> Result<bool, ConfigError> {
        let changed = self
            .connection
            .execute(
                "UPDATE service_route_intents SET classifier_scope=?4
             WHERE binding=?1 AND session=?2 AND turn=?3 AND classifier_scope IS NULL",
                params![turn.binding, turn.session, turn.digest, scope],
            )
            .map_err(error)?;
        Ok(changed == 1)
    }

    pub fn service_complete_intent(
        &self,
        turn: &IntentTurn,
        scope: &str,
        outcome: &IntentOutcome,
    ) -> Result<(), ConfigError> {
        let changed = self.connection.execute(
            "UPDATE service_route_intents SET outcome=?5
             WHERE binding=?1 AND session=?2 AND turn=?3 AND classifier_scope=?4 AND outcome IS NULL",
            params![turn.binding, turn.session, turn.digest, scope, serde_json::to_string(outcome).map_err(error)?],
        ).map_err(error)?;
        if changed != 1 {
            return fail("conflict", "意图分类归属已变化");
        }
        Ok(())
    }

    fn quota_source_revision(&self) -> Result<i64, ConfigError> {
        Ok(self
            .builtin_composition(crate::plugin::builtin::Profile::Desktop)
            .map_err(error)?
            .revision
            .value() as i64)
    }

    pub fn openrouter_admission_evidence(&self) -> Result<Option<String>, ConfigError> {
        let revision = self.quota_source_revision()?;
        self.connection
            .query_row("SELECT value FROM service_openrouter_admission_v1 WHERE id=1 AND composition_revision=?1", [revision], |row| row.get(0))
            .optional()
            .map_err(error)
    }

    pub fn save_openrouter_admission_evidence(&self, value: &str) -> Result<(), ConfigError> {
        let revision = self.quota_source_revision()?;
        self.connection.execute(
            "INSERT INTO service_openrouter_admission_v1(id,composition_revision,value) VALUES(1,?1,?2) ON CONFLICT(id) DO UPDATE SET composition_revision=excluded.composition_revision,value=excluded.value",
            params![revision, value],
        ).map_err(error)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Dispatch {
    pub owner: String,
    pub binding: String,
    pub session: String,
    pub turn: String,
    pub policy: RoutingPolicy,
    pub candidates: Vec<Selection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<RouteDecision>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deprioritized: Vec<RouteDecision>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    children: BTreeMap<String, ChildDispatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<FallbackCheckpoint>,
}

pub enum FailedAttemptAdmission<'a> {
    Cooldown(&'a str, i64, &'a str),
    OpenRouterBudget(&'a OpenRouterBudgetRejection),
    Balance(&'a BalanceRejection),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FallbackCheckpoint {
    pub through: i64,
    pub progress: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChildDispatch {
    pub policy: RoutingPolicy,
    pub candidates: Vec<Selection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<RouteDecision>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deprioritized: Vec<RouteDecision>,
}

struct PlannedCandidates {
    candidates: Vec<Selection>,
    skipped: Vec<RouteDecision>,
    deprioritized: Vec<RouteDecision>,
}

struct PlanningScope<'a> {
    binding: &'a str,
    session: &'a str,
    published: &'a ModelService,
    service: &'a ServiceSelection,
    boundary: &'a str,
}

pub fn child_path(group: &ServiceSelection) -> String {
    std::iter::once(group.model_id.as_str())
        .chain(group.nested_route.iter().map(|hop| hop.model_id.as_str()))
        .collect::<Vec<_>>()
        .join("/")
}

fn owned_dispatch(db: &Connection, plan: &Dispatch) -> Result<Dispatch, ConfigError> {
    let value: Option<String> = db
        .query_row(
            "SELECT value FROM service_route_dispatch WHERE binding=?1 AND session=?2
         AND turn=?3 AND owner=?4 AND state='pending'",
            params![plan.binding, plan.session, plan.turn, plan.owner],
            |row| row.get(0),
        )
        .optional()
        .map_err(error)?;
    let value = value.ok_or_else(|| ConfigError::new("conflict", "请求路由归属已变化"))?;
    serde_json::from_str(&value).map_err(error)
}

fn fallback_crossed_strict_account(
    db: &Connection,
    plan: &Dispatch,
    checkpoint: &FallbackCheckpoint,
) -> Result<bool, ConfigError> {
    let pinned: HashSet<_> = plan
        .candidates
        .iter()
        .chain(plan.children.values().flat_map(|child| &child.candidates))
        .filter(|target| {
            target
                .service
                .as_ref()
                .is_some_and(ServiceSelection::has_strict_account)
        })
        .map(|target| target.version.id.as_str())
        .collect();
    if pinned.is_empty() {
        return Ok(false);
    }
    let mut statement = db
        .prepare(
            "SELECT DISTINCT route FROM service_route_attempts
             WHERE binding=?1 AND session=?2 AND turn=?3 AND id<=?4
               AND outcome IN ('rate_limited','temporary_failure')",
        )
        .map_err(error)?;
    let routes = statement
        .query_map(
            params![plan.binding, plan.session, plan.turn, checkpoint.through],
            |row| row.get::<_, String>(0),
        )
        .map_err(error)?;
    for route in routes {
        if pinned.contains(route.map_err(error)?.as_str()) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn finish_dispatch(
    db: &Connection,
    plan: &Dispatch,
    committed: bool,
    keep_fallback: bool,
) -> Result<(), ConfigError> {
    let retry_at = chrono::Utc::now().timestamp_millis()
        + i64::try_from(plan.policy.cooldown_seconds * 1000).unwrap_or(3_600_000);
    let state = if committed { "committed" } else { "failed" };
    let changed = db
        .execute(
            "UPDATE service_route_dispatch SET state=?5,retry_at=?6,
             value=CASE WHEN ?7 THEN value ELSE json_remove(value,'$.fallback') END
         WHERE binding=?1 AND session=?2 AND turn=?3 AND owner=?4 AND state='pending'",
            params![
                plan.binding,
                plan.session,
                plan.turn,
                plan.owner,
                state,
                retry_at,
                keep_fallback,
            ],
        )
        .map_err(error)?;
    if changed != 1 {
        return fail("conflict", "请求归属变化，未提交模型输出");
    }
    Ok(())
}

impl Store {
    pub fn model_service_child(
        &self,
        child: &super::ChildServiceRef,
    ) -> Result<ModelService, ConfigError> {
        nested::load(&self.connection, child)
    }

    pub fn service_saved_child_plan(
        &self,
        root: &Dispatch,
        group: &ServiceSelection,
    ) -> Result<Option<ChildDispatch>, ConfigError> {
        Ok(owned_dispatch(&self.connection, root)?
            .children
            .remove(&child_path(group)))
    }

    pub fn service_child_dispatch(
        &self,
        root: &Dispatch,
        group: &Selection,
        facts: impl FnOnce(bool) -> RuleFacts,
        hint: impl Fn(&crate::Provider, &AccountRef, &ModelLimit) -> AccountPlanningHint,
    ) -> Result<ChildDispatch, ConfigError> {
        let selection = group
            .service
            .as_ref()
            .ok_or_else(|| error("missing group selection"))?;
        let child = selection
            .pending_child
            .as_ref()
            .ok_or_else(|| error("missing child reference"))?;
        if selection.nested_route.len() >= 2 {
            return fail("nested_service_depth", "最多支持三层模型服务");
        }
        let tx = self.connection.unchecked_transaction().map_err(error)?;
        let mut owned = owned_dispatch(&tx, root)?;
        let path = child_path(selection);
        if let Some(existing) = owned.children.remove(&path) {
            return Ok(existing);
        }
        let published = nested::load(&tx, child)?;
        let mut preferred = selection.clone();
        preferred.service_id.clone_from(&published.id);
        preferred
            .service_version_id
            .clone_from(&published.version_id);
        preferred.service_name.clone_from(&published.name);
        preferred.model_id.clone_from(&published.default_model_id);
        preferred.preferred_model_id = Some(published.default_model_id.clone());
        preferred.mode = "default".into();
        preferred.nested_scope = Some(path.clone());
        preferred.nested_route.clear();
        let PlannedCandidates {
            candidates,
            mut skipped,
            mut deprioritized,
        } = self.plan_candidates(
            &tx,
            PlanningScope {
                binding: &root.binding,
                session: &root.session,
                published: &published,
                service: &preferred,
                boundary: &group.reason,
            },
            facts,
            hint,
        )?;
        let candidates = candidates
            .into_iter()
            .map(|mut candidate| {
                let leaf = candidate
                    .service
                    .take()
                    .expect("service planning supplies evidence");
                let mut captured = selection.clone();
                captured.nested_route.push(NestedHop {
                    service_id: leaf.service_id,
                    version_id: leaf.service_version_id,
                    service_name: leaf.service_name,
                    model_id: leaf.model_id,
                    model_name: leaf.model_name,
                    routing_rule: leaf.routing_rule,
                    routing_intent: leaf.routing_intent,
                });
                captured.nested_scope = leaf.nested_scope;
                captured.pending_child = leaf.pending_child;
                captured.upstream_account = leaf.upstream_account;
                captured.account_selection_reason = leaf.account_selection_reason;
                candidate.service = Some(captured);
                candidate
            })
            .collect();
        for decision in skipped.iter_mut().chain(&mut deprioritized) {
            decision.prepend(selection);
        }
        let scope = ChildDispatch {
            policy: published.routing_policy,
            candidates,
            skipped,
            deprioritized,
        };
        owned.children.insert(path, scope.clone());
        tx.execute(
            "UPDATE service_route_dispatch SET value=?5
             WHERE binding=?1 AND session=?2 AND turn=?3 AND owner=?4 AND state='pending'",
            params![
                root.binding,
                root.session,
                root.turn,
                root.owner,
                serde_json::to_string(&owned).map_err(error)?
            ],
        )
        .map_err(error)?;
        tx.commit().map_err(error)?;
        Ok(scope)
    }

    fn plan_candidates(
        &self,
        tx: &Connection,
        scope: PlanningScope<'_>,
        facts: impl FnOnce(bool) -> RuleFacts,
        hint: impl Fn(&crate::Provider, &AccountRef, &ModelLimit) -> AccountPlanningHint,
    ) -> Result<PlannedCandidates, ConfigError> {
        let PlanningScope {
            binding,
            session,
            published,
            service,
            boundary,
        } = scope;
        let preferred = service
            .preferred_model_id
            .as_ref()
            .unwrap_or(&service.model_id);
        use RouteDecisionReason as Reason;
        let describe = |model: &super::ServiceModel, account_position, reason| {
            let mut selected = service.clone();
            selected.model_id.clone_from(&model.id);
            selected.model_name.clone_from(&model.name);
            RouteDecision::new(&selected, account_position, reason)
        };
        let mut skipped = Vec::new();
        let mut deprioritized = Vec::new();
        let eligible: Vec<_> = published
            .models
            .iter()
            .filter(|model| {
                let reason = if let Some(reason) = published
                    .routing_policy
                    .candidate_exclusion(&model.id, &published.default_model_id)
                {
                    reason
                } else if !model.capabilities.covers(&service.capabilities) {
                    Reason::CapabilityMismatch
                } else {
                    return true;
                };
                skipped.push(describe(model, None, reason));
                false
            })
            .collect();
        let rules = &published.routing_policy.rules;
        let mut intent = None;
        let matched =
            if published.routing_policy.enabled && service.mode == "default" && !rules.is_empty() {
                let facts = facts(rules.iter().any(|rule| rule.project_id.is_some()));
                let matched = rules.iter().find(|rule| {
                    eligible.iter().any(|model| model.id == rule.model_id) && rule.matches(&facts)
                });
                intent = facts.intent;
                matched
            } else {
                None
            };
        let preferred = matched.map_or(preferred, |rule| &rule.model_id);
        let eligible: Vec<_> = eligible
            .into_iter()
            .filter(|model| {
                if published
                    .routing_policy
                    .in_fallback_scope(preferred, &model.id)
                {
                    true
                } else {
                    skipped.push(describe(model, None, Reason::CrossModelFallbackDisabled));
                    false
                }
            })
            .collect();
        let allowed: HashSet<_> = eligible.iter().map(|model| model.id.as_str()).collect();
        let request = RankRequest {
            service_id: published.id.clone(),
            service_version_id: published.version_id.clone(),
            preferred_model_id: allowed
                .contains(preferred.as_str())
                .then(|| preferred.clone()),
            default_model_id: allowed
                .contains(published.default_model_id.as_str())
                .then(|| published.default_model_id.clone()),
            configured_order: published
                .routing_policy
                .order
                .iter()
                .filter(|id| allowed.contains(id.as_str()))
                .cloned()
                .collect(),
            candidates: eligible
                .iter()
                .map(|model| Candidate {
                    model_id: model.id.clone(),
                    capabilities: (&model.capabilities).into(),
                    // The old native path never reads health during ordering. Runtime
                    // cooldown and capacity checks remain immediately before sends.
                    health: CandidateHealth::Unknown,
                })
                .collect(),
        };
        let rank_error = |e: crate::domain::routing_policy::RankError| {
            ConfigError::new(e.code(), "路由排序无法核查，未开始新的发送")
        };
        let ranked = BuiltinPriority.rank(&request).map_err(rank_error)?;
        let ranked = BuiltinPriority
            .inspect_result(&request, ranked)
            .map_err(rank_error)?;
        let mut candidates = vec![];
        let mut remaining = ranked.order.into_iter();
        while let Some(id) = remaining.next() {
            let Some(model) = published.models.iter().find(|m| m.id == id) else {
                continue;
            };
            let route: Option<String> = tx.query_row("SELECT route FROM model_service_targets WHERE binding=?1 AND version=?2 AND model=?3", params![binding,published.version_id,id], |r| r.get(0)).optional().map_err(error)?;
            let target = if let Some(route) = route {
                crate::routing::version(tx, &route)?
            } else {
                let target = crate::routing::insert_version(tx, binding, &model.provider)?;
                tx.execute(
                    "INSERT INTO model_service_targets VALUES(?1,?2,?3,?4)",
                    params![binding, published.version_id, id, target.id],
                )
                .map_err(error)?;
                target
            };
            let mut choice = service.clone();
            choice.model_id = id;
            choice.model_name = model.name.clone();
            choice.routing_rule = matched.map(RouteRule::evidence);
            choice.routing_intent = intent.clone();
            choice.pending_child = model.child.clone();
            let candidate = Selection {
                fresh: false,
                version: target,
                reason: boundary.into(),
                service: Some(choice),
                uncommitted_dispatch: None,
            };
            if let Some(pool) = published.routing_policy.pool(&model.id) {
                let order = account_order::order(
                    self,
                    tx,
                    &published.routing_policy,
                    &candidate,
                    binding,
                    session,
                    &hint,
                )?;
                deprioritized.extend(
                    order
                        .deprioritized
                        .into_iter()
                        .map(|(position, reason)| describe(model, Some(position), reason)),
                );
                if order.strict && order.accounts.is_empty() && candidates.is_empty() {
                    return fail(
                        "account_affinity_unavailable",
                        "此会话固定的上游账号已变更，请开启新会话",
                    );
                }
                if order.strict {
                    for (index, account) in pool.accounts.iter().enumerate() {
                        if !order.accounts.contains(account) {
                            skipped.push(describe(
                                model,
                                Some(index as u8 + 1),
                                Reason::StrictAffinity,
                            ));
                        }
                    }
                    skipped.extend(remaining.by_ref().filter_map(|id| {
                        published
                            .models
                            .iter()
                            .find(|model| model.id == id)
                            .map(|model| describe(model, None, Reason::StrictAffinity))
                    }));
                }
                candidates.extend(order.accounts.into_iter().map(|account| {
                    let mut target = candidate.clone();
                    if let Some(service) = target.service.as_mut() {
                        service.upstream_account = Some(account);
                        service.account_selection_reason = order.reason.map(str::to_owned);
                    }
                    target
                }));
                if order.strict {
                    break;
                }
            } else {
                candidates.push(candidate);
            }
        }
        Ok(PlannedCandidates {
            candidates,
            skipped,
            deprioritized,
        })
    }

    /// The gateway must hold the exclusive turn guard. None means a committed or legacy route.
    pub fn service_dispatch(
        &self,
        binding: &str,
        turn: &Turn,
        selected: &Selection,
        facts: impl FnOnce(bool) -> RuleFacts,
        hint: impl Fn(&crate::Provider, &AccountRef, &ModelLimit) -> AccountPlanningHint,
    ) -> Result<Option<Dispatch>, ConfigError> {
        if let Some(block) = selected.uncommitted_dispatch {
            return match block {
                UncommittedDispatch::Pending => fail(
                    "dispatch_uncertain",
                    "原请求仍在执行或结果尚未确认；未重复发送，请等待完成或开始新轮次",
                ),
                UncommittedDispatch::NoCommittedTarget => fail(
                    "dispatch_not_committed",
                    "原轮次尚无可续调的上游，请等待结果、重试原轮次或开启新会话",
                ),
            };
        }
        let Some(service) = &selected.service else {
            return Ok(None);
        };
        let published = version(&self.connection, &service.service_version_id)?;
        if !published.routing_policy.enabled {
            return Ok(None);
        }
        let session = turn.session.as_deref().unwrap_or("unknown");
        let tx = self.connection.unchecked_transaction().map_err(error)?;
        let old = tx.query_row("SELECT state,value,retry_at FROM service_route_dispatch WHERE binding=?1 AND session=?2 AND turn=?3", params![binding,session,turn.digest], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?))).optional().map_err(error)?;
        let mut plan = if let Some((state, value, retry_at)) = old {
            if state == "committed" {
                if selected.fresh {
                    return fail(
                        "dispatch_already_committed",
                        "原轮次已经选定上游，本次没有开始新的发送",
                    );
                }
                return Ok(None);
            }
            let saved: Dispatch = serde_json::from_str(&value).map_err(error)?;
            if state == "pending" || saved.fallback.is_some() {
                let last_sent: Option<i64> = tx
                    .query_row(
                        "SELECT max(id) FROM service_route_attempts
                     WHERE binding=?1 AND session=?2 AND turn=?3
                     AND outcome NOT IN ('preparing','not_sent','deadline_before_send')",
                        params![binding, session, turn.digest],
                        |row| row.get(0),
                    )
                    .map_err(error)?;
                if turn.continuation
                    || !turn.portable
                    || turn.session.is_none()
                    || last_sent != saved.fallback.as_ref().map(|progress| progress.through)
                {
                    return fail(
                        "dispatch_uncertain",
                        "原请求仍在执行或结果尚未确认；未重复发送，请等待完成或开始新轮次",
                    );
                }
                // Older hosts could checkpoint a parent fallback after a
                // strict child account failed. Keep that turn pinned.
                if let Some(checkpoint) = &saved.fallback
                    && fallback_crossed_strict_account(&tx, &saved, checkpoint)?
                {
                    return fail(
                        "account_affinity_unavailable",
                        "原轮次固定的上游账号已失败，请开启新会话",
                    );
                }
                tx.execute(
                    "UPDATE service_route_attempts SET outcome='not_sent'
                     WHERE binding=?1 AND session=?2 AND turn=?3 AND outcome='preparing'",
                    params![binding, session, turn.digest],
                )
                .map_err(error)?;
            }
            if state != "pending" && retry_at > chrono::Utc::now().timestamp_millis() {
                return fail("policy_cooling", "本轮候选暂不可用，请等待冷却后重试");
            }
            saved
        } else {
            if !selected.fresh {
                return Ok(None);
            }
            let PlannedCandidates {
                candidates,
                skipped,
                deprioritized,
            } = self.plan_candidates(
                &tx,
                PlanningScope {
                    binding,
                    session,
                    published: &published,
                    service,
                    boundary: &selected.reason,
                },
                facts,
                hint,
            )?;
            Dispatch {
                owner: String::new(),
                binding: binding.into(),
                session: session.into(),
                turn: turn.digest.clone(),
                policy: published.routing_policy,
                candidates,
                skipped,
                deprioritized,
                children: BTreeMap::new(),
                fallback: None,
            }
        };
        plan.owner = uuid::Uuid::new_v4().to_string();
        tx.execute("INSERT INTO service_route_dispatch(binding,session,turn,owner,state,value) VALUES(?1,?2,?3,?4,'pending',?5) ON CONFLICT(binding,session,turn) DO UPDATE SET owner=excluded.owner,state='pending',value=excluded.value",params![binding,session,turn.digest,plan.owner,serde_json::to_string(&plan).map_err(error)?]).map_err(error)?;
        tx.commit().map_err(error)?;
        Ok(Some(plan))
    }

    pub fn service_attempt(
        &self,
        plan: &Dispatch,
        selected: &Selection,
    ) -> Result<i64, ConfigError> {
        if selected
            .service
            .as_ref()
            .is_some_and(|selection| selection.pending_child.is_some())
        {
            return fail("invalid_child_service", "子服务尚未选定实际模型");
        }
        let tx = self.connection.unchecked_transaction().map_err(error)?;
        let owns: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM service_route_dispatch WHERE binding=?1 AND session=?2 AND turn=?3 AND owner=?4 AND state='pending')",params![plan.binding,plan.session,plan.turn,plan.owner],|r|r.get(0)).map_err(error)?;
        if !owns {
            return fail("conflict", "请求路由归属已变化，未重复发送");
        }
        let count: i64 = tx
            .query_row("SELECT count(*) FROM service_route_attempts", [], |r| {
                r.get(0)
            })
            .map_err(error)?;
        if count >= 32768 {
            return fail("capacity", "路由尝试记录已满，保留现有历史");
        }
        let changed=tx.execute("UPDATE gateway_route_leases SET version=?4,service_selection=?5 WHERE binding=?1 AND session=?2 AND turn=?3",params![plan.binding,plan.session,plan.turn,selected.version.id,serde_json::to_string(&selected.service).map_err(error)?]).map_err(error)?;
        if changed != 1 {
            return fail("conflict", "轮次原路由无法匹配，未发送新目标请求");
        }
        tx.execute("INSERT INTO service_route_attempts(binding,session,turn,route,model,outcome,created) VALUES(?1,?2,?3,?4,?5,'preparing',?6)",params![plan.binding,plan.session,plan.turn,selected.version.id,selected.service.as_ref().map(|s|s.model_id.as_str()).unwrap_or(""),chrono::Utc::now().to_rfc3339()]).map_err(error)?;
        let id = tx.last_insert_rowid();
        tx.commit().map_err(error)?;
        Ok(id)
    }

    /// Persist the point past which the upstream may have received this attempt.
    pub fn service_attempt_begin_send(&self, plan: &Dispatch, id: i64) -> Result<(), ConfigError> {
        let changed = self
            .connection
            .execute(
                "UPDATE service_route_attempts AS a SET outcome='pending'
             WHERE a.id=?1 AND a.binding=?2 AND a.session=?3 AND a.turn=?4 AND a.outcome='preparing'
             AND EXISTS(SELECT 1 FROM service_route_dispatch d WHERE d.binding=a.binding
                 AND d.session=a.session AND d.turn=a.turn AND d.owner=?5 AND d.state='pending')",
                params![id, plan.binding, plan.session, plan.turn, plan.owner],
            )
            .map_err(error)?;
        if changed != 1 {
            return fail("conflict", "请求路由归属已变化，未重复发送");
        }
        Ok(())
    }

    pub fn service_attempt_result(
        &self,
        id: i64,
        status: u16,
        outcome: &str,
    ) -> Result<(), ConfigError> {
        self.connection
            .execute(
                "UPDATE service_route_attempts SET status=?2,outcome=?3 WHERE id=?1",
                params![id, status, outcome],
            )
            .map_err(error)?;
        Ok(())
    }

    /// Commit the definite failure, its admission restriction, and any authorized continuation together.
    pub fn service_record_failed_attempt(
        &self,
        plan: &Dispatch,
        attempt: i64,
        status: u16,
        outcome: &str,
        checkpoint: Option<&FallbackCheckpoint>,
        admission: FailedAttemptAdmission<'_>,
    ) -> Result<(), ConfigError> {
        let encoded = checkpoint
            .map(serde_json::to_string)
            .transpose()
            .map_err(error)?;
        if encoded
            .as_ref()
            .is_some_and(|value| value.len() > 128 * 1024)
        {
            return fail("capacity", "路由恢复记录超过限制");
        }
        if checkpoint.is_some_and(|saved| saved.through != attempt)
            || !matches!(
                outcome,
                "rate_limited"
                    | "temporary_failure"
                    | "upstream_credential_rejected"
                    | "upstream_model_access_denied"
                    | "upstream_quota_exhausted"
            )
            || (outcome == "upstream_quota_exhausted")
                != matches!(
                    &admission,
                    FailedAttemptAdmission::OpenRouterBudget(_)
                        | FailedAttemptAdmission::Balance(_)
                )
        {
            return fail("conflict", "此尝试缺少明确的回退结果");
        }
        let tx = self.connection.unchecked_transaction().map_err(error)?;
        let changed = tx
            .execute(
                "UPDATE service_route_attempts AS a SET status=?2,outcome=?3
                 WHERE a.id=?1 AND a.binding=?4 AND a.session=?5 AND a.turn=?6 AND a.outcome='pending'
                 AND EXISTS(SELECT 1 FROM service_route_dispatch d WHERE d.binding=a.binding
                     AND d.session=a.session AND d.turn=a.turn AND d.owner=?7 AND d.state='pending')",
                params![attempt,status,outcome,plan.binding,plan.session,plan.turn,plan.owner],
            )
            .map_err(error)?;
        if changed != 1 {
            return fail("conflict", "路由恢复记录与本轮尝试不匹配");
        }
        if let Some(encoded) = encoded {
            let changed = tx
                .execute(
                    "UPDATE service_route_dispatch SET value=json_set(value,'$.fallback',json(?5))
                 WHERE binding=?1 AND session=?2 AND turn=?3 AND owner=?4 AND state='pending'",
                    params![plan.binding, plan.session, plan.turn, plan.owner, encoded],
                )
                .map_err(error)?;
            if changed != 1 {
                return fail("conflict", "路由恢复进度未保存，未发送备用请求");
            }
        }
        match admission {
            FailedAttemptAdmission::Cooldown(scope, until, reason) => {
                set_cooldown(&tx, scope, until, reason)?;
            }
            FailedAttemptAdmission::OpenRouterBudget(rejection) => {
                openrouter_rejections::record(&tx, rejection)?;
            }
            FailedAttemptAdmission::Balance(rejection) => {
                upstream_balance::record(&tx, &rejection.credential, rejection.observed_at, false)?;
            }
        }
        tx.commit().map_err(error)
    }

    pub fn service_dispatch_finish(
        &self,
        plan: &Dispatch,
        committed: bool,
    ) -> Result<(), ConfigError> {
        finish_dispatch(&self.connection, plan, committed, false)
    }
    /// Call only after dropping the send future and confirming it never reached HTTP send.
    pub fn service_deadline_before_send(
        &self,
        plan: &Dispatch,
        attempt: i64,
    ) -> Result<(), ConfigError> {
        let tx = self.connection.unchecked_transaction().map_err(error)?;
        let changed = tx
            .execute(
                "UPDATE service_route_attempts SET status=504,outcome='deadline_before_send'
             WHERE id=?1 AND binding=?2 AND session=?3 AND turn=?4
             AND outcome IN ('preparing','pending')",
                params![attempt, plan.binding, plan.session, plan.turn],
            )
            .map_err(error)?;
        if changed != 1 {
            return fail("conflict", "路由尝试记录已变化，未释放原轮次");
        }
        finish_dispatch(&tx, plan, false, true)?;
        tx.commit().map_err(error)
    }
    pub fn service_cooldown_probe(&self, scope: &str) -> Result<bool, ConfigError> {
        self.connection.query_row("SELECT EXISTS(SELECT 1 FROM service_route_cooldowns WHERE scope=?1 AND until_ms<=?2)",params![scope,chrono::Utc::now().timestamp_millis()],|r|r.get(0)).map_err(error)
    }
    /// Recovery is eligible only when a matching marker exists and all
    /// restrictions for this target have expired.
    pub fn service_recovery_pending(
        &self,
        [account, model, access_account, access_model]: [&str; 4],
        now: i64,
    ) -> Result<bool, ConfigError> {
        self.connection
            .query_row(
                "SELECT COALESCE(max(until_ms)<=?5,0) FROM service_route_cooldowns
                 WHERE scope IN (?1,?2,?3,?4)",
                params![account, model, access_account, access_model, now],
                |row| row.get(0),
            )
            .map_err(error)
    }
    pub fn service_recovered(&self, scope: &str) -> Result<(), ConfigError> {
        self.service_recover_cooldowns(&[scope], chrono::Utc::now().timestamp_millis())
    }
    pub fn service_recover_cooldowns(
        &self,
        scopes: &[&str],
        checked_at: i64,
    ) -> Result<(), ConfigError> {
        let tx = self.connection.unchecked_transaction().map_err(error)?;
        for scope in scopes {
            tx.execute(
                "DELETE FROM service_route_cooldowns WHERE scope=?1 AND until_ms<=?2",
                params![scope, checked_at],
            )
            .map_err(error)?;
        }
        tx.commit().map_err(error)
    }
    pub fn service_cooldown(&self, scope: &str) -> Result<Option<(i64, String)>, ConfigError> {
        self.connection.query_row("SELECT until_ms,reason FROM service_route_cooldowns WHERE scope=?1 AND until_ms>?2",params![scope,chrono::Utc::now().timestamp_millis()],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(error)
    }
    pub fn service_cooldowns(&self) -> Result<BTreeMap<String, (i64, String)>, ConfigError> {
        self.connection
            .prepare("SELECT scope,until_ms,reason FROM service_route_cooldowns")
            .map_err(error)?
            .query_map([], |row| Ok((row.get(0)?, (row.get(1)?, row.get(2)?))))
            .map_err(error)?
            .collect::<Result<_, _>>()
            .map_err(error)
    }
    pub fn active_service_cooldowns(&self) -> Result<BTreeMap<String, (i64, String)>, ConfigError> {
        let now = chrono::Utc::now().timestamp_millis();
        let mut rows = self.service_cooldowns()?;
        rows.retain(|_, (until, _)| *until > now);
        Ok(rows)
    }
    pub fn service_set_cooldown(
        &self,
        scope: &str,
        until: i64,
        reason: &str,
    ) -> Result<(), ConfigError> {
        set_cooldown(&self.connection, scope, until, reason)
    }
}

fn set_cooldown(db: &Connection, scope: &str, until: i64, reason: &str) -> Result<(), ConfigError> {
    // Expiration makes a target eligible for a probe. An unrelated failure
    // must not remove the still-unverified recovery requirement.
    let count: i64 = db
        .query_row(
            "SELECT count(*) FROM service_route_cooldowns WHERE scope<>?1",
            [scope],
            |r| r.get(0),
        )
        .map_err(error)?;
    if count >= 4096 {
        return fail("capacity", "冷却状态容量已满");
    }
    db.execute("INSERT INTO service_route_cooldowns VALUES(?1,?2,?3) ON CONFLICT(scope) DO UPDATE SET until_ms=max(until_ms,excluded.until_ms),reason=excluded.reason",params![scope,until,reason]).map_err(error)?;
    Ok(())
}
