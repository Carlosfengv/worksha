//! Stable authenticated bindings point at immutable versions. Leases are durable
//! before a request is sent, including failed or cancelled requests.
use crate::{
    Provider, Store,
    providers::{ConfigError, fail, fingerprint, validate_source},
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

pub const ALIAS: &str = "local-agent-default";
const MAX_LEASES: i64 = 4096;
const MAX_VERSIONS: i64 = 4096;

fn db(_: impl std::fmt::Display) -> ConfigError {
    ConfigError::new(
        "routing_unavailable",
        "动态路由存储不可用；未改变已提交目标",
    )
}
fn encode<T: Serialize>(value: &T) -> Result<String, ConfigError> {
    serde_json::to_string(value).map_err(db)
}
fn decode<T: for<'a> Deserialize<'a>>(value: &str) -> Result<T, ConfigError> {
    serde_json::from_str(value).map_err(db)
}
pub(crate) fn initialize(db: &Connection) -> Result<(), ConfigError> {
    db.execute_batch("BEGIN IMMEDIATE;
      CREATE TABLE IF NOT EXISTS gateway_bindings(id TEXT PRIMARY KEY,agent TEXT NOT NULL,scope TEXT NOT NULL,credential TEXT NOT NULL,target TEXT NOT NULL REFERENCES gateway_route_versions(id) DEFERRABLE INITIALLY DEFERRED,generation INTEGER NOT NULL CHECK(generation>0),created TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS gateway_route_versions(id TEXT PRIMARY KEY,binding TEXT NOT NULL REFERENCES gateway_bindings(id) DEFERRABLE INITIALLY DEFERRED,value TEXT NOT NULL CHECK(json_valid(value)),UNIQUE(id,binding));
      CREATE INDEX IF NOT EXISTS gateway_versions_binding ON gateway_route_versions(binding);
      CREATE TABLE IF NOT EXISTS gateway_route_changes(id TEXT PRIMARY KEY,binding TEXT NOT NULL REFERENCES gateway_bindings(id),state TEXT NOT NULL CHECK(state IN ('pending','applied','cancelled')),value TEXT NOT NULL CHECK(json_valid(value)));
      CREATE UNIQUE INDEX IF NOT EXISTS gateway_pending_change ON gateway_route_changes(binding) WHERE state='pending';
      CREATE TABLE IF NOT EXISTS gateway_route_leases(seq INTEGER PRIMARY KEY,binding TEXT NOT NULL REFERENCES gateway_bindings(id),session TEXT NOT NULL,turn TEXT NOT NULL,version TEXT NOT NULL,chain TEXT NOT NULL CHECK(json_valid(chain)),trusted INTEGER NOT NULL CHECK(trusted IN (0,1)),reason TEXT NOT NULL,UNIQUE(binding,session,turn),FOREIGN KEY(version,binding) REFERENCES gateway_route_versions(id,binding));
      CREATE INDEX IF NOT EXISTS gateway_session_leases ON gateway_route_leases(binding,session,seq DESC);
      COMMIT;").map_err(db_error)?;
    Ok(())
}
fn db_error(error: rusqlite::Error) -> ConfigError {
    db(error)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Version {
    pub id: String,
    pub binding_id: String,
    pub provider: Provider,
    pub created_at: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Binding {
    pub id: String,
    pub agent_id: String,
    pub scope: String,
    pub credential_ref: String,
    pub generation: String,
    pub target: Version,
    pub created_at: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub id: String,
    pub binding_id: String,
    pub expected_generation: String,
    pub state: String,
    pub before: Version,
    pub target: Provider,
    pub published: Option<Version>,
    pub created_at: String,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaseSummary {
    pub binding_id: String,
    pub route_version_id: String,
    pub turns: i64,
}
#[derive(Clone)]
pub struct Turn {
    pub session: Option<String>,
    pub digest: String,
    pub chain: Vec<String>,
    pub continuation: bool,
    pub portable: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Selection {
    #[serde(skip)]
    pub fresh: bool,
    pub version: Version,
    pub reason: String,
    pub service: Option<crate::model_services::ServiceSelection>,
}
pub(crate) fn version(db: &Connection, id: &str) -> Result<Version, ConfigError> {
    let value: String = db
        .query_row(
            "SELECT value FROM gateway_route_versions WHERE id=?1",
            [id],
            |r| r.get(0),
        )
        .map_err(db_error)?;
    decode(&value)
}
fn binding(db: &Connection, id: &str) -> Result<Option<Binding>, ConfigError> {
    let raw = db.query_row("SELECT agent,scope,credential,target,generation,created FROM gateway_bindings WHERE id=?1", [id], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,i64>(4)?,r.get::<_,String>(5)?))).optional().map_err(db_error)?;
    raw.map(|(agent, scope, credential, target, generation, created)| {
        Ok(Binding {
            id: id.into(),
            agent_id: agent,
            scope,
            credential_ref: credential,
            generation: generation.to_string(),
            target: version(db, &target)?,
            created_at: created,
        })
    })
    .transpose()
}
fn required(db: &Connection, id: &str) -> Result<Binding, ConfigError> {
    binding(db, id)?.ok_or_else(|| ConfigError::new("not_found", "动态绑定不存在"))
}
pub(crate) fn insert_version(
    db: &Connection,
    binding_id: &str,
    provider: &Provider,
) -> Result<Version, ConfigError> {
    let count: i64 = db
        .query_row("SELECT count(*) FROM gateway_route_versions", [], |r| {
            r.get(0)
        })
        .map_err(db_error)?;
    if count >= MAX_VERSIONS {
        return fail("capacity", "动态路由版本已达上限；旧版本与恢复引用保留");
    }
    let v = Version {
        id: uuid::Uuid::new_v4().to_string(),
        binding_id: binding_id.into(),
        provider: provider.clone(),
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    if db
        .execute(
            "INSERT INTO gateway_route_versions VALUES(?1,?2,?3)",
            params![v.id, binding_id, encode(&v)?],
        )
        .map_err(db_error)?
        != 1
    {
        return Err(self::db("version not inserted"));
    }
    verify_version(db, &v)?;
    Ok(v)
}
fn verify_version(db: &Connection, expected: &Version) -> Result<(), ConfigError> {
    let (owner, value): (String, String) = db
        .query_row(
            "SELECT binding,value FROM gateway_route_versions WHERE id=?1",
            [&expected.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(db_error)?;
    if owner != expected.binding_id || value != encode(expected)? {
        return Err(self::db("version readback changed"));
    }
    Ok(())
}
fn verify_binding(db: &Connection, expected: &Binding) -> Result<(), ConfigError> {
    verify_version(db, &expected.target)?;
    if encode(&required(db, &expected.id)?)? != encode(expected)? {
        return Err(self::db("binding readback changed"));
    }
    Ok(())
}
fn create_binding(
    db: &Connection,
    id: &str,
    agent: &str,
    scope: &str,
    credential: &str,
    provider: &Provider,
) -> Result<Binding, ConfigError> {
    uuid::Uuid::parse_str(id).map_err(self::db)?;
    if !crate::model_services::CONNECTION_AGENTS.contains(&agent) {
        return fail("unsupported", "此 Agent 尚未支持模型服务接入");
    }
    uuid::Uuid::parse_str(credential).map_err(self::db)?;
    validate_source(provider)?;
    let n: i64 = db
        .query_row("SELECT count(*) FROM gateway_bindings", [], |r| r.get(0))
        .map_err(db_error)?;
    if n >= 64 {
        return fail("capacity", "动态绑定已达 64 项；不会删除旧配置引用");
    }
    let v = insert_version(db, id, provider)?;
    if db
        .execute(
            "INSERT INTO gateway_bindings VALUES(?1,?2,?3,?4,?5,1,?6)",
            params![id, agent, scope, credential, v.id, v.created_at],
        )
        .map_err(db_error)?
        != 1
    {
        return Err(self::db("binding not inserted"));
    }
    let expected = Binding {
        id: id.into(),
        agent_id: agent.into(),
        scope: scope.into(),
        credential_ref: credential.into(),
        generation: "1".into(),
        created_at: v.created_at.clone(),
        target: v,
    };
    verify_binding(db, &expected)?;
    Ok(expected)
}
impl Store {
    pub fn dynamic_bindings(&self) -> Result<Vec<Binding>, ConfigError> {
        let mut stmt = self
            .connection
            .prepare("SELECT id FROM gateway_bindings ORDER BY created DESC")
            .map_err(db_error)?;
        let ids = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?;
        ids.iter()
            .map(|id| required(&self.connection, id))
            .collect()
    }
    pub fn dynamic_binding(&self, id: &str) -> Result<Option<Binding>, ConfigError> {
        binding(&self.connection, id)
    }
    pub fn dynamic_create(
        &self,
        agent: &str,
        scope: &str,
        credential: &str,
        provider: &Provider,
    ) -> Result<Binding, ConfigError> {
        if !["claude", "opencode"].contains(&agent) {
            return fail("unsupported", "此 Agent 尚未支持单模型动态接入");
        }
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let binding = create_binding(
            &tx,
            &uuid::Uuid::new_v4().to_string(),
            agent,
            scope,
            credential,
            provider,
        )?;
        tx.commit().map_err(db_error)?;
        Ok(binding)
    }
    /// Create the binding, initial route and service attachment together. This
    /// covers SQL ownership only; the caller still owns the external Vault write.
    pub fn create_model_service_binding(
        &self,
        agent: &str,
        scope: &str,
        credential: &str,
        service: &crate::model_services::ModelService,
    ) -> Result<(Binding, crate::model_services::ServiceAttachment), ConfigError> {
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let model = service
            .models
            .iter()
            .find(|m| m.id == service.default_model_id)
            .ok_or_else(|| db("missing service default model"))?;
        let binding = create_binding(
            &tx,
            &uuid::Uuid::new_v4().to_string(),
            agent,
            scope,
            credential,
            &model.provider,
        )?;
        let attachment = crate::model_services::attach_binding(&tx, &binding.id, service)?;
        // An attachment trigger may have rewritten an earlier binding/version.
        verify_binding(&tx, &binding)?;
        tx.commit().map_err(db_error)?;
        Ok((binding, attachment))
    }
    /// Reuse only the exact stable owner recorded before the local token write.
    pub fn create_owned_local_binding(
        &self,
        operation: &crate::providers::local_tokens::Operation,
    ) -> Result<Binding, ConfigError> {
        use crate::providers::local_tokens::{Purpose, Status};
        if operation.status != Status::Pending || operation.request.purpose == Purpose::FixedRoute {
            return fail("local_token_cancelled", "此本机令牌操作不可再创建动态绑定");
        }
        let request = &operation.request;
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let binding = if let Some(existing) = binding(&tx, &operation.owner_id)? {
            if existing.agent_id != request.agent_id
                || existing.scope != request.configuration_path
                || existing.credential_ref != operation.credential_ref
                || existing.generation != "1"
                || fingerprint(&existing.target.provider) != fingerprint(&request.provider)
            {
                return fail("conflict", "原本机入口绑定已变化，未重建或覆盖");
            }
            verify_binding(&tx, &existing)?;
            existing
        } else {
            create_binding(
                &tx,
                &operation.owner_id,
                &request.agent_id,
                &request.configuration_path,
                &operation.credential_ref,
                &request.provider,
            )?
        };
        match &request.service {
            Some(service) => {
                let current = crate::model_services::current(&tx, &service.id)?;
                if current.version_id != service.version_id {
                    return fail("conflict", "原模型服务版本已变化");
                }
                if let Some(saved) = crate::model_services::attachment(&tx, &operation.owner_id)? {
                    if saved.service_id != service.id
                        || saved.installed.version_id != service.version_id
                    {
                        return fail("conflict", "原模型服务归属已变化，未覆盖");
                    }
                } else {
                    crate::model_services::attach_binding(&tx, &operation.owner_id, &current)?;
                }
            }
            None => {
                if crate::model_services::attachment(&tx, &operation.owner_id)?.is_some() {
                    return fail("conflict", "原动态绑定已属于模型服务，未改变用途");
                }
            }
        }
        verify_binding(&tx, &binding)?;
        crate::providers::local_tokens::save_binding_owner(&tx, operation)?;
        verify_binding(&tx, &binding)?;
        let saved = crate::model_services::attachment(&tx, &operation.owner_id)?;
        if match (&request.service, saved) {
            (Some(expected), Some(actual)) => {
                actual.service_id != expected.id
                    || actual.installed.version_id != expected.version_id
            }
            (None, None) => false,
            _ => true,
        } {
            return fail("conflict", "本机入口最终归属发生变化，未提交绑定");
        }
        tx.commit().map_err(db_error)?;
        Ok(binding)
    }
    pub fn dynamic_changes(&self) -> Result<Vec<Change>, ConfigError> {
        let mut stmt = self
            .connection
            .prepare("SELECT value FROM gateway_route_changes ORDER BY rowid DESC LIMIT 100")
            .map_err(db_error)?;
        let values = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?;
        values.iter().map(|v| decode(v)).collect()
    }
    pub fn dynamic_change(&self, id: &str) -> Result<Change, ConfigError> {
        let value = self
            .connection
            .query_row(
                "SELECT value FROM gateway_route_changes WHERE id=?1",
                [id],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(db_error)?
            .ok_or_else(|| ConfigError::new("not_found", "动态切换操作不存在"))?;
        decode(&value)
    }
    pub fn dynamic_prepare(
        &self,
        id: &str,
        binding_id: &str,
        generation: &str,
        target: &Provider,
    ) -> Result<Change, ConfigError> {
        uuid::Uuid::parse_str(id).map_err(db)?;
        validate_source(target)?;
        if let Ok(old) = self.dynamic_change(id) {
            if old.binding_id == binding_id
                && old.expected_generation == generation
                && fingerprint(&old.target) == fingerprint(target)
            {
                return Ok(old);
            }
            return fail("conflict", "操作 ID 已用于其他目标");
        }
        let b = required(&self.connection, binding_id)?;
        if b.generation != generation {
            return fail("conflict", "动态目标已变化，请重新预览");
        }
        let count: i64 = self
            .connection
            .query_row("SELECT count(*) FROM gateway_route_changes", [], |r| {
                r.get(0)
            })
            .map_err(db_error)?;
        if count >= 4096 {
            return fail("capacity", "动态操作记录已达上限，历史与恢复记录保留");
        }
        let c = Change {
            id: id.into(),
            binding_id: binding_id.into(),
            expected_generation: generation.into(),
            state: "pending".into(),
            before: b.target,
            target: target.clone(),
            published: None,
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        self.connection
            .execute(
                "INSERT INTO gateway_route_changes VALUES(?1,?2,'pending',?3)",
                params![id, binding_id, encode(&c)?],
            )
            .map_err(|_| ConfigError::new("conflict", "此绑定已有待确认操作，请先确认或撤销"))?;
        Ok(c)
    }
    pub fn dynamic_cancel(&self, id: &str) -> Result<Change, ConfigError> {
        let mut c = self.dynamic_change(id)?;
        if c.state == "cancelled" {
            return Ok(c);
        }
        if c.state != "pending" {
            return fail("conflict", "已提交路由请使用恢复操作");
        }
        c.state = "cancelled".into();
        self.connection.execute("UPDATE gateway_route_changes SET state='cancelled',value=?2 WHERE id=?1 AND state='pending'",params![id,encode(&c)?]).map_err(db_error)?;
        Ok(c)
    }
    pub fn dynamic_commit(&self, id: &str) -> Result<Change, ConfigError> {
        let mut c = self.dynamic_change(id)?;
        if c.state == "applied" {
            return Ok(c);
        }
        if c.state != "pending" {
            return fail("conflict", "此动态操作已结束");
        }
        let tx = self.connection.unchecked_transaction().map_err(db_error)?;
        let b = required(&tx, &c.binding_id)?;
        if b.generation != c.expected_generation {
            return fail("conflict", "目标版本已变化，未覆盖");
        }
        let generation = b
            .generation
            .parse::<i64>()
            .ok()
            .and_then(|v| v.checked_add(1))
            .ok_or_else(|| db("generation"))?;
        let v = insert_version(&tx, &b.id, &c.target)?;
        tx.execute(
            "UPDATE gateway_bindings SET target=?2,generation=?3 WHERE id=?1",
            params![b.id, v.id, generation],
        )
        .map_err(db_error)?;
        c.state = "applied".into();
        c.published = Some(v);
        tx.execute(
            "UPDATE gateway_route_changes SET state='applied',value=?2 WHERE id=?1",
            params![id, encode(&c)?],
        )
        .map_err(db_error)?;
        tx.commit().map_err(db_error)?;
        Ok(c)
    }
    pub fn dynamic_lease_summary(&self) -> Result<Vec<LeaseSummary>, ConfigError> {
        let mut s=self.connection.prepare("SELECT binding,version,count(*) FROM gateway_route_leases GROUP BY binding,version").map_err(db_error)?;
        s.query_map([], |r| {
            Ok(LeaseSummary {
                binding_id: r.get(0)?,
                route_version_id: r.get(1)?,
                turns: r.get(2)?,
            })
        })
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)
    }
    pub fn dynamic_admit(&self, id: &str, turn: &Turn) -> Result<Selection, ConfigError> {
        self.admit(id, turn, None)
    }
    pub fn model_service_admit(
        &self,
        id: &str,
        turn: &Turn,
        model: &str,
    ) -> Result<Selection, ConfigError> {
        self.admit(id, turn, Some(model))
    }
    fn admit(&self, id: &str, turn: &Turn, model: Option<&str>) -> Result<Selection, ConfigError> {
        let tx = self.connection.unchecked_transaction().map_err(db_error)?;
        let b = required(&tx, id)?;
        let attachment = crate::model_services::attachment(&tx, id)?;
        if let Some(model) = model
            && !attachment.as_ref().is_some_and(|a| a.accepts(model))
        {
            return fail("invalid_model", "请选择此模型服务已注入的模型");
        }
        let session = turn.session.as_deref().unwrap_or("unknown");
        let digest = if turn.session.is_some() {
            turn.digest.as_str()
        } else {
            "unknown"
        };
        if let Some((v,reason,service))=tx.query_row("SELECT version,reason,service_selection FROM gateway_route_leases WHERE binding=?1 AND session=?2 AND turn=?3",params![id,session,digest],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?))).optional().map_err(db_error)?{
            return Ok(Selection{fresh:false,version:version(&tx,&v)?,reason,service:service.as_deref().map(decode).transpose()?});
        }
        let old=tx.query_row("SELECT version,chain,trusted,service_selection FROM gateway_route_leases WHERE binding=?1 AND session=?2 ORDER BY seq DESC LIMIT 1",params![id,session],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,bool>(2)?,r.get::<_,Option<String>>(3)?))).optional().map_err(db_error)?;
        let mut selected = b.target;
        let mut service = None;
        let mut pinned = false;
        let mut trusted = turn.session.is_some() && !turn.chain.is_empty();
        let mut reason = "new_session".to_string();
        if let Some((old_id, chain, was_trusted, old_service)) = old {
            let chain: Vec<String> = decode(&chain)?;
            let boundary = was_trusted
                && !turn.continuation
                && turn.chain.len() > chain.len()
                && turn.chain.starts_with(&chain);
            trusted = was_trusted && (boundary || turn.chain == chain);
            if !boundary || !turn.portable {
                selected = version(&tx, &old_id)?;
                service = old_service.as_deref().map(decode).transpose()?;
                pinned = true;
                reason = if !turn.portable {
                    "history_requires_original_route"
                } else {
                    "session_boundary_unconfirmed"
                }
                .into();
            } else {
                reason = "new_user_turn".into();
            }
        } else if turn.continuation || !turn.portable {
            return fail(
                "unknown_continuation",
                "工具续调缺少已保存的原路由，请开启新会话",
            );
        }
        if !pinned && let (Some(attachment), Some(model)) = (attachment, model) {
            let (published, target, selection) = attachment.resolve(&tx, model)?;
            let route: Option<String> = tx.query_row("SELECT route FROM model_service_targets WHERE binding=?1 AND version=?2 AND model=?3",params![id,published.version_id,target.id],|r|r.get(0)).optional().map_err(db_error)?;
            selected = if let Some(route) = route {
                version(&tx, &route)?
            } else {
                let v = insert_version(&tx, id, &target.provider)?;
                tx.execute(
                    "INSERT INTO model_service_targets VALUES(?1,?2,?3,?4)",
                    params![id, published.version_id, target.id, v.id],
                )
                .map_err(db_error)?;
                v
            };
            service = Some(selection);
        }
        let count: i64 = tx
            .query_row("SELECT count(*) FROM gateway_route_leases", [], |r| {
                r.get(0)
            })
            .map_err(db_error)?;
        if count >= MAX_LEASES {
            return fail("capacity", "轮次路由记录已达上限；不会驱逐旧工具续调");
        }
        tx.execute("INSERT INTO gateway_route_leases(binding,session,turn,version,chain,trusted,reason,service_selection) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![id,session,digest,selected.id,encode(&turn.chain)?,trusted,reason,service.as_ref().map(encode).transpose()?]).map_err(db_error)?;
        tx.commit().map_err(db_error)?;
        Ok(Selection {
            fresh: trusted && !pinned && !turn.continuation && turn.portable,
            version: selected,
            reason,
            service,
        })
    }
}

#[cfg(test)]
mod binding_creation_tests;
#[cfg(test)]
mod tests;
