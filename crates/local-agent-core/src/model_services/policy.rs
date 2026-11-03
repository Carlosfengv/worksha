//! Service policy snapshots and durable dispatch ownership. No request bodies.
use super::{error, version};
use crate::{
    Store,
    domain::routing_policy::{
        BuiltinPriority, Candidate, CandidateHealth, RankRequest, RoutingPolicy as PolicyAdapter,
    },
    providers::{ConfigError, fail},
    routing::{Selection, Turn},
};
use rusqlite::{Connection, OptionalExtension, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct ModelLimit {
    pub model_id: String,
    pub max_concurrent: u32,
    pub requests_per_minute: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct RoutingPolicy {
    pub enabled: bool,
    pub order: Vec<String>,
    pub disabled_models: Vec<String>,
    pub fallback_on_failure: bool,
    pub fallback_on_rate_limit: bool,
    pub max_attempts: u8,
    pub deadline_seconds: u64,
    pub cooldown_seconds: u64,
    pub limits: Vec<ModelLimit>,
}
impl Default for RoutingPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            order: vec![],
            disabled_models: vec![],
            fallback_on_failure: true,
            fallback_on_rate_limit: true,
            max_attempts: 3,
            deadline_seconds: 120,
            cooldown_seconds: 30,
            limits: vec![],
        }
    }
}
impl RoutingPolicy {
    pub fn validate(&self, ids: &HashSet<String>, default: &str) -> Result<(), ConfigError> {
        let order: HashSet<_> = self.order.iter().cloned().collect();
        let disabled: HashSet<_> = self.disabled_models.iter().cloned().collect();
        let limits: HashSet<_> = self.limits.iter().map(|m| m.model_id.clone()).collect();
        if !(1..=5).contains(&self.max_attempts)
            || !(5..=300).contains(&self.deadline_seconds)
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
}

pub(crate) fn initialize(db: &Connection) -> Result<(), ConfigError> {
    db.execute_batch("BEGIN IMMEDIATE;
      CREATE TABLE IF NOT EXISTS service_route_dispatch(binding TEXT NOT NULL,session TEXT NOT NULL,turn TEXT NOT NULL,owner TEXT NOT NULL,state TEXT NOT NULL CHECK(state IN('pending','committed','failed')),value TEXT NOT NULL CHECK(json_valid(value)),retry_at INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(binding,session,turn));
      CREATE TABLE IF NOT EXISTS service_route_attempts(id INTEGER PRIMARY KEY,binding TEXT NOT NULL,session TEXT NOT NULL,turn TEXT NOT NULL,route TEXT NOT NULL,model TEXT NOT NULL,status INTEGER,outcome TEXT NOT NULL,created TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS service_route_cooldowns(scope TEXT PRIMARY KEY,until_ms INTEGER NOT NULL,reason TEXT NOT NULL);
      COMMIT;").map_err(error)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Dispatch {
    pub owner: String,
    pub binding: String,
    pub session: String,
    pub turn: String,
    pub policy: RoutingPolicy,
    pub candidates: Vec<Selection>,
}
impl Store {
    /// None means an already committed/legacy turn. It must retain its route.
    pub fn service_dispatch(
        &self,
        binding: &str,
        turn: &Turn,
        selected: &Selection,
    ) -> Result<Option<Dispatch>, ConfigError> {
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
                return Ok(None);
            }
            if state == "pending" {
                return fail(
                    "dispatch_uncertain",
                    "原请求仍在执行或结果尚未确认；未重复发送，请等待完成或开始新轮次",
                );
            }
            if retry_at > chrono::Utc::now().timestamp_millis() {
                return fail("policy_cooling", "本轮候选暂不可用，请等待冷却后重试");
            }
            serde_json::from_str::<Dispatch>(&value).map_err(error)?
        } else {
            if !selected.fresh {
                return Ok(None);
            }
            let eligible: Vec<_> = published
                .models
                .iter()
                .filter(|model| {
                    !published.routing_policy.disabled_models.contains(&model.id)
                        && model.capabilities.covers(&service.capabilities)
                })
                .collect();
            let allowed: HashSet<_> = eligible.iter().map(|model| model.id.as_str()).collect();
            let preferred = service
                .preferred_model_id
                .as_ref()
                .unwrap_or(&service.model_id);
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
            for id in ranked.order {
                let Some(model) = published.models.iter().find(|m| m.id == id) else {
                    continue;
                };
                let route: Option<String> = tx.query_row("SELECT route FROM model_service_targets WHERE binding=?1 AND version=?2 AND model=?3", params![binding,published.version_id,id], |r| r.get(0)).optional().map_err(error)?;
                let target = if let Some(route) = route {
                    crate::routing::version(&tx, &route)?
                } else {
                    let target = crate::routing::insert_version(&tx, binding, &model.provider)?;
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
                candidates.push(Selection {
                    fresh: false,
                    version: target,
                    reason: selected.reason.clone(),
                    service: Some(choice),
                });
            }
            Dispatch {
                owner: String::new(),
                binding: binding.into(),
                session: session.into(),
                turn: turn.digest.clone(),
                policy: published.routing_policy,
                candidates,
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
        tx.execute("INSERT INTO service_route_attempts(binding,session,turn,route,model,outcome,created) VALUES(?1,?2,?3,?4,?5,'pending',?6)",params![plan.binding,plan.session,plan.turn,selected.version.id,selected.service.as_ref().map(|s|s.model_id.as_str()).unwrap_or(""),chrono::Utc::now().to_rfc3339()]).map_err(error)?;
        let id = tx.last_insert_rowid();
        tx.commit().map_err(error)?;
        Ok(id)
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
    pub fn service_dispatch_finish(
        &self,
        plan: &Dispatch,
        committed: bool,
    ) -> Result<(), ConfigError> {
        let n = self.connection.execute("UPDATE service_route_dispatch SET state=?5,retry_at=?6 WHERE binding=?1 AND session=?2 AND turn=?3 AND owner=?4 AND state='pending'",params![plan.binding,plan.session,plan.turn,plan.owner,if committed {"committed"} else {"failed"},chrono::Utc::now().timestamp_millis()+i64::try_from(plan.policy.cooldown_seconds*1000).unwrap_or(3_600_000)]).map_err(error)?;
        if n != 1 {
            return fail("conflict", "请求归属变化，未提交模型输出");
        }
        Ok(())
    }
    pub fn service_cooldown_probe(&self, scope: &str) -> Result<bool, ConfigError> {
        self.connection.query_row("SELECT EXISTS(SELECT 1 FROM service_route_cooldowns WHERE scope=?1 AND until_ms<=?2)",params![scope,chrono::Utc::now().timestamp_millis()],|r|r.get(0)).map_err(error)
    }
    pub fn service_recovered(&self, scope: &str) -> Result<(), ConfigError> {
        self.connection
            .execute(
                "DELETE FROM service_route_cooldowns WHERE scope=?1 AND until_ms<=?2",
                params![scope, chrono::Utc::now().timestamp_millis()],
            )
            .map_err(error)?;
        Ok(())
    }
    pub fn service_cooldown(&self, scope: &str) -> Result<Option<(i64, String)>, ConfigError> {
        self.connection.query_row("SELECT until_ms,reason FROM service_route_cooldowns WHERE scope=?1 AND until_ms>?2",params![scope,chrono::Utc::now().timestamp_millis()],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(error)
    }
    pub fn service_set_cooldown(
        &self,
        scope: &str,
        until: i64,
        reason: &str,
    ) -> Result<(), ConfigError> {
        self.connection
            .execute(
                "DELETE FROM service_route_cooldowns WHERE until_ms<=?1",
                [chrono::Utc::now().timestamp_millis()],
            )
            .map_err(error)?;
        let count: i64 = self
            .connection
            .query_row("SELECT count(*) FROM service_route_cooldowns", [], |r| {
                r.get(0)
            })
            .map_err(error)?;
        if count >= 4096 {
            return fail("capacity", "冷却状态容量已满");
        }
        self.connection.execute("INSERT INTO service_route_cooldowns VALUES(?1,?2,?3) ON CONFLICT(scope) DO UPDATE SET until_ms=max(until_ms,excluded.until_ms),reason=excluded.reason",params![scope,until,reason]).map_err(error)?;
        Ok(())
    }
}
