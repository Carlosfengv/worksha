use super::loop_guard;
use super::{ConfigError, Store, db_error, decode, encode, fail};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub(crate) fn initialize(conn: &Connection) -> Result<(), ConfigError> {
    conn.execute_batch(
        "BEGIN IMMEDIATE;
         CREATE TABLE IF NOT EXISTS agent_budget_settings(
             binding TEXT PRIMARY KEY REFERENCES gateway_bindings(id), revision INTEGER NOT NULL,
             time_zone TEXT NOT NULL, fallback_service TEXT REFERENCES model_services(id), warn_ratio INTEGER NOT NULL);
         CREATE TABLE IF NOT EXISTS agent_budget_policies(
             binding TEXT NOT NULL REFERENCES agent_budget_settings(binding), window_kind TEXT NOT NULL,
             limits TEXT NOT NULL CHECK(json_valid(limits)), PRIMARY KEY(binding,window_kind));
         CREATE TABLE IF NOT EXISTS agent_budget_ledgers(
             binding TEXT NOT NULL REFERENCES gateway_bindings(id), tier TEXT NOT NULL,
             window_kind TEXT NOT NULL, window_key TEXT NOT NULL, value TEXT NOT NULL CHECK(json_valid(value)),
             PRIMARY KEY(binding,tier,window_kind,window_key));
         CREATE TABLE IF NOT EXISTS agent_budget_reservations(
             attempt_key TEXT PRIMARY KEY, binding TEXT NOT NULL REFERENCES gateway_bindings(id),
             observation TEXT NOT NULL UNIQUE, state TEXT NOT NULL, value TEXT NOT NULL CHECK(json_valid(value)));
         CREATE INDEX IF NOT EXISTS agent_budget_attempt_lookup ON agent_budget_reservations(binding,state,attempt_key);
         CREATE TABLE IF NOT EXISTS agent_budget_reviews(
             operation TEXT PRIMARY KEY, request TEXT NOT NULL CHECK(json_valid(request)),
             receipt TEXT NOT NULL CHECK(json_valid(receipt)));
         CREATE TABLE IF NOT EXISTS agent_budget_route_states(
             binding TEXT NOT NULL REFERENCES gateway_bindings(id), session TEXT NOT NULL, turn TEXT NOT NULL,
             seq INTEGER NOT NULL REFERENCES gateway_route_leases(seq) ON DELETE CASCADE, tier TEXT, request_id TEXT,
             output_confirmed INTEGER NOT NULL DEFAULT 0 CHECK(output_confirmed IN (0,1)),
             PRIMARY KEY(binding,session,turn));
         CREATE TABLE IF NOT EXISTS agent_budget_warnings(
             id TEXT PRIMARY KEY, binding TEXT NOT NULL REFERENCES gateway_bindings(id), tier TEXT NOT NULL,
             window_kind TEXT NOT NULL, window_key TEXT NOT NULL, created TEXT NOT NULL, notified INTEGER NOT NULL DEFAULT 0,
             UNIQUE(binding,tier,window_kind,window_key));
         COMMIT;"
    ).map_err(db_error)
}

const MAX_AMOUNT: u64 = 1_000_000_000_000_000;
const MAX_PENDING: i64 = 32_768;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WindowKind {
    Session,
    Daily,
    Monthly,
}
impl WindowKind {
    fn key(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Daily => "daily",
            Self::Monthly => "monthly",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Tier {
    Primary,
    Fallback,
}
impl Tier {
    fn key(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Fallback => "fallback",
        }
    }
    fn parse(value: &str) -> Result<Self, ConfigError> {
        match value {
            "primary" => Ok(Self::Primary),
            "fallback" => Ok(Self::Fallback),
            _ => fail("invalid_record", "预算扣账池记录损坏"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Amounts {
    pub tokens: Option<u64>,
    pub usd_micros: Option<u64>,
}
impl Amounts {
    pub fn empty(self) -> bool {
        self.tokens.is_none() && self.usd_micros.is_none()
    }

    fn valid(self) -> bool {
        [self.tokens, self.usd_micros]
            .into_iter()
            .flatten()
            .all(|amount| amount <= MAX_AMOUNT)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WindowPolicy {
    pub kind: WindowKind,
    pub primary: Amounts,
    pub fallback: Amounts,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Policy {
    pub revision: u32,
    pub timezone: String,
    pub fallback_service_id: Option<String>,
    pub warn_percent: u8,
    pub windows: Vec<WindowPolicy>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Counters {
    pub committed: u64,
    pub reserved: u64,
    pub uncertain: u64,
}
impl Counters {
    fn usage(self) -> u128 {
        u128::from(self.committed) + u128::from(self.reserved) + u128::from(self.uncertain)
    }
    fn take(&mut self, state: AttemptState, reserved: u64) -> Result<(), ConfigError> {
        let value = if state == AttemptState::Uncertain {
            &mut self.uncertain
        } else {
            &mut self.reserved
        };
        *value = value.checked_sub(reserved).ok_or_else(invalid)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ledger {
    pub tokens: Counters,
    pub usd_micros: Counters,
}
impl Ledger {
    fn fits(self, reserve: Amounts, limits: Amounts) -> bool {
        let fits = |counter: Counters, reserve: Option<u64>, limit: Option<u64>| {
            limit.is_none_or(|limit| {
                reserve.is_some_and(|n| counter.usage() + u128::from(n) <= u128::from(limit))
            })
        };
        fits(self.tokens, reserve.tokens, limits.tokens)
            && fits(self.usd_micros, reserve.usd_micros, limits.usd_micros)
    }
    fn reserve(&mut self, amount: Amounts) -> Result<(), ConfigError> {
        for (counter, amount) in [
            (&mut self.tokens, amount.tokens),
            (&mut self.usd_micros, amount.usd_micros),
        ] {
            if let Some(amount) = amount {
                counter.reserved = counter.reserved.checked_add(amount).ok_or_else(invalid)?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AttemptState {
    Preparing,
    Sending,
    Uncertain,
    Committed,
    Released,
}

impl AttemptState {
    fn key(self) -> &'static str {
        match self {
            Self::Preparing => "preparing",
            Self::Sending => "sending",
            Self::Uncertain => "uncertain",
            Self::Committed => "committed",
            Self::Released => "released",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChargeWindow {
    pub kind: WindowKind,
    pub key: String,
    pub reserved: Amounts,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attempt {
    pub attempt_key: String,
    pub observation_id: String,
    pub binding_id: String,
    pub session: Option<String>,
    pub turn: String,
    pub request_id: String,
    pub tier: Tier,
    pub grace: bool,
    pub provider_id: String,
    pub model: String,
    pub upstream_request_id: Option<String>,
    pub created_at: String,
    pub state: AttemptState,
    #[serde(default)]
    pub sent: bool,
    pub revision: u32,
    pub windows: Vec<ChargeWindow>,
    pub frozen: Amounts,
    pub actual: Amounts,
    pub late_evidence: Amounts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    loop_pending: Option<(loop_guard::Scope, Amounts)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    loop_charge: Option<loop_guard::Charge>,
}

pub struct Prepare<'a> {
    pub binding_id: &'a str,
    pub session: Option<&'a str>,
    pub turn: &'a str,
    pub request_id: &'a str,
    pub observation_id: &'a str,
    pub provider_id: &'a str,
    pub model: &'a str,
    pub selected_tier: Tier,
    pub continuation: bool,
    pub now: DateTime<Utc>,
    pub estimate: Amounts,
    pub fallback: Option<(&'a super::Selection, Amounts)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ReviewMetric {
    Settle { amount: u64 },
    Release,
}
impl ReviewMetric {
    fn amount(&self) -> u64 {
        match self {
            Self::Settle { amount } => *amount,
            Self::Release => 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Review {
    pub store_id: String,
    pub operation_id: String,
    pub binding_id: String,
    pub attempt_key: String,
    pub expected_revision: u32,
    pub upstream_request_id: String,
    pub source: String,
    pub tokens: Option<ReviewMetric>,
    pub usd_micros: Option<ReviewMetric>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewReceipt {
    pub operation_id: String,
    pub actor: String,
    pub source: String,
    pub upstream_request_id: String,
    pub confirmed: Amounts,
    pub attempt: Attempt,
}

fn invalid() -> ConfigError {
    ConfigError::new("invalid_record", "预算记录不一致，未发送或修改账本")
}
fn exhausted(code: &str) -> ConfigError {
    let message = match code {
        "budget_fallback_exhausted" => "备用服务预算不足",
        "budget_grace_exceeded" => "本轮工具续调宽限已用尽",
        _ => "本机绑定预算不足",
    };
    ConfigError::new(code, message)
}
fn ledger(
    conn: &Connection,
    binding: &str,
    tier: Tier,
    kind: WindowKind,
    window: &str,
) -> Result<Ledger, ConfigError> {
    let raw: Option<String> = conn.query_row(
        "SELECT value FROM agent_budget_ledgers WHERE binding=?1 AND tier=?2 AND window_kind=?3 AND window_key=?4",
        params![binding,tier.key(),kind.key(),window], |r|r.get(0)
    ).optional().map_err(db_error)?;
    raw.map_or(Ok(Ledger::default()), |raw| decode(&raw))
}

fn write_ledger(
    conn: &Connection,
    binding: &str,
    tier: Tier,
    window: &ChargeWindow,
    value: &Ledger,
) -> Result<(), ConfigError> {
    conn.execute(
        "INSERT INTO agent_budget_ledgers VALUES(?1,?2,?3,?4,?5)
         ON CONFLICT(binding,tier,window_kind,window_key) DO UPDATE SET value=excluded.value",
        params![
            binding,
            tier.key(),
            window.kind.key(),
            window.key,
            encode(value)?
        ],
    )
    .map_err(db_error)?;
    if let Some((limits, warn))=conn.query_row(
        "SELECT p.limits,s.warn_ratio FROM agent_budget_policies p JOIN agent_budget_settings s ON p.binding=s.binding WHERE p.binding=?1 AND p.window_kind=?2",
        params![binding,window.kind.key()],|row|Ok((row.get::<_,String>(0)?,row.get::<_,u8>(1)?))
    ).optional().map_err(db_error)? {
        let limits=decode::<WindowPolicy>(&limits)?;
        let limit=if tier==Tier::Primary {limits.primary} else {limits.fallback};
        if warning(*value,limit,warn) {
            conn.execute("INSERT OR IGNORE INTO agent_budget_warnings(id,binding,tier,window_kind,window_key,created) VALUES(?1,?2,?3,?4,?5,?6)",
                params![uuid::Uuid::new_v4().to_string(),binding,tier.key(),window.kind.key(),window.key,Utc::now().to_rfc3339()]).map_err(db_error)?;
        }
    }
    Ok(())
}

fn warning(value: Ledger, limit: Amounts, percent: u8) -> bool {
    [
        (limit.tokens, value.tokens),
        (limit.usd_micros, value.usd_micros),
    ]
    .into_iter()
    .any(|(limit, value)| {
        limit.is_some_and(|limit| value.usage() * 100 >= u128::from(limit) * u128::from(percent))
    })
}

fn policy(conn: &Connection, id: &str) -> Result<Option<Policy>, ConfigError> {
    let mut settings = conn.query_row(
        "SELECT revision,time_zone,fallback_service,warn_ratio FROM agent_budget_settings WHERE binding=?1", [id],
        |row| Ok(Policy { revision:row.get(0)?, timezone:row.get(1)?, fallback_service_id:row.get(2)?,
            warn_percent:row.get(3)?,windows:Vec::new() })
    ).optional().map_err(db_error)?;
    if let Some(settings) = &mut settings {
        let mut statement = conn
            .prepare(
                "SELECT limits FROM agent_budget_policies WHERE binding=?1 ORDER BY window_kind",
            )
            .map_err(db_error)?;
        let rows = statement
            .query_map([id], |row| row.get::<_, String>(0))
            .map_err(db_error)?;
        settings.windows = rows
            .map(|row| decode(&row.map_err(db_error)?))
            .collect::<Result<_, _>>()?;
    }
    Ok(settings)
}

pub(super) fn is_active_local(conn: &Connection, id: &str) -> Result<(), ConfigError> {
    let binding = super::required(conn, id)?;
    if binding.scope.starts_with("revoked:") || binding.agent_id == super::chat::AGENT && !conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM gateway_chat_keys WHERE binding=?1 AND digest IS NOT NULL)",[id],|row|row.get(0)
    ).map_err(db_error)? {
        return fail("conflict", "该绑定已撤销");
    }
    Ok(())
}

fn work(conn: &Connection, key: &str) -> Result<Attempt, ConfigError> {
    let record: Option<String> = conn
        .query_row(
            "SELECT value FROM agent_budget_reservations WHERE attempt_key=?1",
            [key],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error)?;
    let record = record.ok_or_else(|| ConfigError::new("not_found", "未找到预算尝试"))?;
    decode(&record)
}

fn write_attempt(conn: &Connection, item: &Attempt) -> Result<(), ConfigError> {
    if conn
        .execute(
            "UPDATE agent_budget_reservations SET state=?2,value=?3 WHERE attempt_key=?1",
            params![item.attempt_key, item.state.key(), encode(item)?],
        )
        .map_err(db_error)?
        != 1
    {
        return Err(invalid());
    }
    Ok(())
}

fn revision(item: &mut Attempt) -> Result<(), ConfigError> {
    item.revision = item.revision.checked_add(1).ok_or_else(invalid)?;
    Ok(())
}

fn keys(
    policy: &Policy,
    now: DateTime<Utc>,
    session: Option<&str>,
) -> Result<Vec<(WindowPolicy, String)>, ConfigError> {
    let local = now.with_timezone(
        &policy
            .timezone
            .parse::<chrono_tz::Tz>()
            .map_err(|_| invalid())?,
    );
    policy
        .windows
        .iter()
        .map(|window| {
            let key = match window.kind {
                WindowKind::Session => session
                    .ok_or_else(|| {
                        ConfigError::new("budget_session_required", "会话预算要求有效的会话 ID")
                    })?
                    .to_owned(),
                WindowKind::Daily => local.format("%Y-%m-%d").to_string(),
                WindowKind::Monthly => local.format("%Y-%m").to_string(),
            };
            Ok((window.clone(), key))
        })
        .collect()
}

fn attempts(
    conn: &Connection,
    sql: &str,
    parameters: impl rusqlite::Params,
) -> Result<Vec<Attempt>, ConfigError> {
    let mut stmt = conn.prepare(sql).map_err(db_error)?;
    let rows = stmt
        .query_map(parameters, |row| row.get::<_, String>(0))
        .map_err(db_error)?;
    rows.map(|row| decode(&row.map_err(db_error)?)).collect()
}

fn windows(
    conn: &Connection,
    p: &Prepare<'_>,
    windows: &[(WindowPolicy, String)],
    tier: Tier,
) -> Result<Vec<(ChargeWindow, Ledger)>, ConfigError> {
    windows
        .iter()
        .map(|(window, key)| {
            let limits = if tier == Tier::Primary {
                window.primary
            } else {
                window.fallback
            };
            let reserve = Amounts {
                tokens: limits.tokens.and(p.estimate.tokens),
                usd_micros: limits.usd_micros.and(p.estimate.usd_micros),
            };
            if limits.usd_micros.is_some() && reserve.usd_micros.is_none() {
                return fail(
                    "budget_pricing_unavailable",
                    "当前候选没有可核实的 USD 单价",
                );
            }
            let value = ledger(conn, p.binding_id, tier, window.kind, key)?;
            if !value.fits(reserve, limits) {
                return Err(exhausted(if tier == Tier::Primary {
                    "local_agent_budget_exceeded"
                } else {
                    "budget_fallback_exhausted"
                }));
            }
            Ok((
                ChargeWindow {
                    kind: window.kind,
                    key: key.clone(),
                    reserved: reserve,
                },
                value,
            ))
        })
        .collect()
}

fn grace(
    conn: &Connection,
    p: &Prepare<'_>,
    windows: &[(WindowPolicy, String)],
) -> Result<(), ConfigError> {
    let entries = attempts(conn,
        "SELECT value FROM agent_budget_reservations WHERE binding=?1
         AND json_extract(value,'$.session')=?2 AND json_extract(value,'$.turn')=?3
         AND json_extract(value,'$.grace')=1 AND (state<>'released' OR json_extract(value,'$.sent')=1)",
         params![p.binding_id,p.session,p.turn])?;
    let requests: BTreeSet<_> = entries.iter().map(|row| row.request_id.as_str()).collect();
    if requests.len() >= 3 && !requests.contains(p.request_id) {
        return Err(exhausted("budget_grace_exceeded"));
    }
    for (rule, _) in windows {
        for (token_metric, limit, extra, cap) in [
            (true, rule.primary.tokens, p.estimate.tokens, 64_000),
            (
                false,
                rule.primary.usd_micros,
                p.estimate.usd_micros,
                500_000,
            ),
        ] {
            let Some(limit) = limit else {
                continue;
            };
            let metric = |a: Amounts| if token_metric { a.tokens } else { a.usd_micros };
            let used: u128 = entries
                .iter()
                .filter_map(|row| {
                    let window = row.windows.iter().find(|window| window.kind == rule.kind)?;
                    metric(row.actual)
                        .or_else(|| match row.state {
                            AttemptState::Preparing | AttemptState::Sending => {
                                metric(window.reserved)
                            }
                            AttemptState::Uncertain => metric(row.frozen),
                            _ => None,
                        })
                        .map(u128::from)
                })
                .sum();
            if extra
                .is_none_or(|extra| used + u128::from(extra) > u128::from((limit / 10).min(cap)))
            {
                return Err(exhausted("budget_grace_exceeded"));
            }
        }
    }
    Ok(())
}

impl Store {
    pub fn budget_policy(&self, binding: &str) -> Result<Option<Policy>, ConfigError> {
        policy(&self.connection, binding)
    }

    pub fn save_budget_policy(
        &self,
        store_id: &str,
        binding: &str,
        input: Policy,
    ) -> Result<Policy, ConfigError> {
        if self.database_id()? != store_id {
            return fail("conflict", "本机数据目录已变化");
        }
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
            .map_err(db_error)?;
        is_active_local(&tx, binding)?;
        let existing = policy(&tx, binding)?;
        if existing.as_ref().map_or(0, |p| p.revision) != input.revision
            || existing
                .as_ref()
                .is_some_and(|p| p.timezone != input.timezone)
        {
            return fail("conflict", "预算版本已变化，已创建策略的计费时区保持固定");
        }
        if !(1..=100).contains(&input.warn_percent)
            || input.timezone.parse::<chrono_tz::Tz>().is_err()
            || input.windows.len() > 3
        {
            return fail("invalid_input", "预算时区、阈值或周期无效");
        }
        if let Some(fallback) = &input.fallback_service_id {
            let attached = crate::model_services::attachment(&tx, binding)?
                .ok_or_else(|| ConfigError::new("invalid_input", "只有模型服务绑定支持备用服务"))?;
            if &attached.service_id == fallback {
                return fail("invalid_input", "备用服务必须不同于主服务");
            }
            crate::model_services::current(&tx, fallback)?;
        }
        let mut kinds = BTreeSet::new();
        for w in &input.windows {
            let positive = |amount: Amounts| {
                [amount.tokens, amount.usd_micros]
                    .into_iter()
                    .flatten()
                    .all(|n| n > 0)
                    && amount.valid()
            };
            if !kinds.insert(w.kind)
                || w.primary.empty()
                || !positive(w.primary)
                || !positive(w.fallback)
                || if input.fallback_service_id.is_some() {
                    w.primary.tokens.is_some() != w.fallback.tokens.is_some()
                        || w.primary.usd_micros.is_some() != w.fallback.usd_micros.is_some()
                } else {
                    !w.fallback.empty()
                }
            {
                return fail("invalid_input", "主备预算须按已启用指标填写正数上限");
            }
        }
        let revision = input.revision.checked_add(1).ok_or_else(invalid)?;
        tx.execute("INSERT INTO agent_budget_settings VALUES(?1,?2,?3,?4,?5)
            ON CONFLICT(binding) DO UPDATE SET revision=excluded.revision,fallback_service=excluded.fallback_service,warn_ratio=excluded.warn_ratio",
            params![binding,revision,input.timezone,input.fallback_service_id,input.warn_percent]).map_err(db_error)?;
        tx.execute(
            "DELETE FROM agent_budget_policies WHERE binding=?1",
            [binding],
        )
        .map_err(db_error)?;
        for window in &input.windows {
            tx.execute(
                "INSERT INTO agent_budget_policies VALUES(?1,?2,?3)",
                params![binding, window.kind.key(), encode(window)?],
            )
            .map_err(db_error)?;
        }
        tx.commit().map_err(db_error)?;
        Ok(Policy { revision, ..input })
    }

    pub fn budget_prepare(&self, p: &Prepare<'_>) -> Result<Option<Attempt>, ConfigError> {
        self.budget_prepare_monitored(p, None)
    }

    pub fn budget_prepare_monitored(
        &self,
        p: &Prepare<'_>,
        monitor: Option<loop_guard::Scope>,
    ) -> Result<Option<Attempt>, ConfigError> {
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let policy = policy(&tx, p.binding_id)?.filter(|p| !p.windows.is_empty());
        if policy.is_none() && monitor.is_none() {
            return Ok(None);
        }
        is_active_local(&tx, p.binding_id)?;
        let mut tier = p.selected_tier;
        let mut use_grace = false;
        let rows = if let Some(policy) = policy {
            let keys = keys(&policy, p.now, p.session)?;
            if tier == Tier::Fallback && policy.fallback_service_id.is_none() {
                return Err(exhausted("budget_fallback_exhausted"));
            }
            select_route(&tx, p, &policy, &keys)?;
            match windows(&tx, p, &keys, tier) {
                Err(error)
                    if error.code == "local_agent_budget_exceeded"
                        && p.continuation
                        && p.session.is_some()
                        && policy.fallback_service_id.is_some() =>
                {
                    grace(&tx, p, &keys)?;
                    use_grace = true;
                    tier = Tier::Fallback;
                    windows(&tx, p, &keys, tier).map_err(|error| {
                        if error.code == "budget_fallback_exhausted" {
                            exhausted("budget_grace_exceeded")
                        } else {
                            error
                        }
                    })?
                }
                other => other?,
            }
        } else {
            Vec::new()
        };
        let count: i64 = tx.query_row("SELECT count(*) FROM agent_budget_reservations WHERE state IN('preparing','sending','uncertain')",[],|row|row.get(0)).map_err(db_error)?;
        if count >= MAX_PENDING {
            return fail("capacity", "待核查或在途预算记录已达上限");
        }
        let mut item = Attempt {
            attempt_key: uuid::Uuid::new_v4().to_string(),
            observation_id: p.observation_id.into(),
            binding_id: p.binding_id.into(),
            session: p.session.map(str::to_owned),
            turn: p.turn.into(),
            request_id: p.request_id.into(),
            tier,
            grace: use_grace,
            provider_id: p.provider_id.into(),
            model: p.model.into(),
            upstream_request_id: None,
            created_at: p.now.to_rfc3339(),
            state: AttemptState::Preparing,
            sent: false,
            revision: 1,
            windows: Vec::new(),
            frozen: Amounts::default(),
            actual: Amounts::default(),
            late_evidence: Amounts::default(),
            loop_pending: monitor.map(|scope| (scope, p.estimate)),
            loop_charge: None,
        };
        for (window, mut value) in rows {
            value.reserve(window.reserved)?;
            write_ledger(&tx, p.binding_id, tier, &window, &value)?;
            item.windows.push(window);
        }
        tx.execute(
            "INSERT INTO agent_budget_reservations VALUES(?1,?2,?3,?4,?5)",
            params![
                item.attempt_key,
                item.binding_id,
                item.observation_id,
                item.state.key(),
                encode(&item)?
            ],
        )
        .map_err(db_error)?;
        tx.commit().map_err(db_error)?;
        Ok(Some(item))
    }

    pub(crate) fn budget_mark_sending(&self, attempt_key: &str) -> Result<(), ConfigError> {
        let mut item = work(&self.connection, attempt_key)?;
        if item.state != AttemptState::Preparing || self.connection.is_autocommit() {
            return Err(invalid());
        }
        is_active_local(&self.connection, &item.binding_id)?;
        if let Some((scope, estimate)) = item.loop_pending.take() {
            let session = item.session.as_deref().ok_or_else(invalid)?;
            item.loop_charge =
                loop_guard::begin(&self.connection, &item.binding_id, session, scope, estimate)?;
        }
        item.state = AttemptState::Sending;
        item.sent = true;
        revision(&mut item)?;
        write_attempt(&self.connection, &item)
    }

    pub fn budget_complete(
        &self,
        key: &str,
        evidence: Amounts,
        upstream_id: Option<&str>,
    ) -> Result<Attempt, ConfigError> {
        if !evidence.valid() {
            return Err(invalid());
        }
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let mut item = work(&tx, key)?;
        if let Some(id) = upstream_id.filter(|id| valid_source(id, 200))
            && item.upstream_request_id.is_none()
        {
            item.upstream_request_id = Some(id.into());
        }
        settle(&tx, &mut item, evidence, true)?;
        tx.commit().map_err(db_error)?;
        Ok(item)
    }

    pub fn budget_record_response(
        &self,
        key: &str,
        upstream: Option<&str>,
        confirmed: bool,
    ) -> Result<(), ConfigError> {
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let mut item = work(&tx, key)?;
        if !item.sent {
            return Err(invalid());
        }
        if let Some(id) = upstream.filter(|id| valid_source(id, 200))
            && item.upstream_request_id.is_none()
        {
            item.upstream_request_id = Some(id.into());
            write_attempt(&tx, &item)?;
        }
        if confirmed && let Some(session) = item.session.as_deref() {
            tx.execute("UPDATE agent_budget_route_states SET output_confirmed=1 WHERE binding=?1 AND session=?2 AND turn=?3 AND tier IS NOT NULL",
                params![item.binding_id,session,item.turn]).map_err(db_error)?;
        }
        tx.commit().map_err(db_error)
    }

    pub fn budget_recover(&self) -> Result<(), ConfigError> {
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let rows = attempts(
            &tx,
            "SELECT value FROM agent_budget_reservations WHERE state IN('preparing','sending')",
            [],
        )?;
        for mut item in rows {
            settle(&tx, &mut item, Amounts::default(), true)?;
        }
        tx.commit().map_err(db_error)
    }

    pub fn budget_uncertain(&self, binding: &str) -> Result<Vec<Attempt>, ConfigError> {
        attempts(
            &self.connection,
            "SELECT value FROM agent_budget_reservations WHERE binding=?1 AND state='uncertain'
             ORDER BY json_extract(value,'$.createdAt') DESC,attempt_key LIMIT 200",
            [binding],
        )
    }

    pub fn budget_attempt_for_request(
        &self,
        request: &str,
    ) -> Result<Option<Attempt>, ConfigError> {
        let value: Option<String> = self.connection.query_row(
            "SELECT value FROM agent_budget_reservations WHERE observation=?1 AND state='uncertain'",
            [request], |row|row.get(0)
        ).optional().map_err(db_error)?;
        value.map(|value| decode(&value)).transpose()
    }

    pub fn budget_review(&self, input: &Review, actor: &str) -> Result<ReviewReceipt, ConfigError> {
        if self.database_id()? != input.store_id
            || !uuid::Uuid::parse_str(&input.operation_id)
                .is_ok_and(|uuid| uuid.get_version_num() == 4)
        {
            return fail("conflict", "预算核查操作编号或数据目录已变化");
        }
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
            .map_err(db_error)?;
        if let Some((request, receipt)) = tx
            .query_row(
                "SELECT request,receipt FROM agent_budget_reviews WHERE operation=?1",
                [&input.operation_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(db_error)?
        {
            if decode::<Review>(&request)? != *input {
                return fail("conflict", "同一预算核查操作的参数已变化");
            }
            return decode(&receipt);
        }
        let mut item = work(&tx, &input.attempt_key)?;
        if item.binding_id != input.binding_id
            || item.state != AttemptState::Uncertain
            || item.revision != input.expected_revision
        {
            return fail("conflict", "待核查尝试状态已变化");
        }
        if !valid_source(&input.source, 320)
            || !valid_source(&input.upstream_request_id, 200)
            || item
                .upstream_request_id
                .as_ref()
                .is_some_and(|id| id != &input.upstream_request_id)
        {
            return fail(
                "invalid_input",
                "提交同一次上游请求的标识及已核实的证据出处",
            );
        }
        let confirmed = Amounts {
            tokens: input.tokens.as_ref().map(ReviewMetric::amount),
            usd_micros: input.usd_micros.as_ref().map(ReviewMetric::amount),
        };
        if confirmed.empty()
            || !confirmed.valid()
            || confirmed.tokens.is_some() && item.frozen.tokens.is_none()
            || confirmed.usd_micros.is_some() && item.frozen.usd_micros.is_none()
        {
            return fail("conflict", "本次提交包含无效或已核清的指标");
        }
        item.upstream_request_id
            .get_or_insert(input.upstream_request_id.clone());
        settle(&tx, &mut item, confirmed, false)?;
        let receipt = ReviewReceipt {
            operation_id: input.operation_id.clone(),
            actor: actor.into(),
            source: input.source.clone(),
            upstream_request_id: input.upstream_request_id.clone(),
            confirmed,
            attempt: item,
        };
        tx.execute(
            "INSERT INTO agent_budget_reviews VALUES(?1,?2,?3)",
            params![input.operation_id, encode(input)?, encode(&receipt)?],
        )
        .map_err(db_error)?;
        tx.commit().map_err(db_error)?;
        Ok(receipt)
    }
}

fn valid_source(s: &str, max: usize) -> bool {
    !s.trim().is_empty() && s.len() <= max && !s.chars().any(char::is_control)
}

fn settle(
    conn: &Connection,
    item: &mut Attempt,
    evidence: Amounts,
    automatic: bool,
) -> Result<(), ConfigError> {
    let previous = item.state;
    let was_sent = previous != AttemptState::Preparing;
    let first = matches!(previous, AttemptState::Sending | AttemptState::Preparing);
    let estimate = item
        .loop_charge
        .map_or_else(Amounts::default, |charge| charge.estimate);
    let enabled = Amounts {
        tokens: item
            .windows
            .iter()
            .find_map(|w| w.reserved.tokens)
            .or(estimate.tokens),
        usd_micros: item
            .windows
            .iter()
            .find_map(|w| w.reserved.usd_micros)
            .or(estimate.usd_micros),
    };
    let enabled = if first { enabled } else { item.frozen };
    let mut confirmed = Amounts::default();
    let mut frozen = Amounts::default();
    for (reserve, known, actual, saved_conflict, next, waiting) in [
        (
            enabled.tokens,
            evidence.tokens,
            &mut item.actual.tokens,
            &mut item.late_evidence.tokens,
            &mut confirmed.tokens,
            &mut frozen.tokens,
        ),
        (
            enabled.usd_micros,
            evidence.usd_micros,
            &mut item.actual.usd_micros,
            &mut item.late_evidence.usd_micros,
            &mut confirmed.usd_micros,
            &mut frozen.usd_micros,
        ),
    ] {
        if let Some(reserve) = reserve {
            if let Some(amount) = if was_sent { known } else { Some(0) } {
                *actual = Some(amount);
                *next = Some(amount);
            } else {
                *waiting = Some(reserve);
            }
        } else if automatic
            && was_sent
            && item.loop_charge.is_some()
            && actual.is_none()
            && known.is_some()
        {
            *actual = known;
            *next = known;
        } else if automatic
            && let (Some(old), Some(new)) = (*actual, known)
            && old != new
        {
            *saved_conflict = Some(new);
        }
    }
    if first || !confirmed.empty() {
        for window in &item.windows {
            let mut value = ledger(conn, &item.binding_id, item.tier, window.kind, &window.key)?;
            for (counter, reserved, amount) in [
                (&mut value.tokens, window.reserved.tokens, confirmed.tokens),
                (
                    &mut value.usd_micros,
                    window.reserved.usd_micros,
                    confirmed.usd_micros,
                ),
            ] {
                if let Some(reserve) = reserved {
                    if first || amount.is_some() {
                        counter.take(previous, reserve)?;
                    }
                    if let Some(actual) = amount {
                        counter.committed =
                            counter.committed.checked_add(actual).ok_or_else(invalid)?;
                    } else if first {
                        counter.uncertain =
                            counter.uncertain.checked_add(reserve).ok_or_else(invalid)?;
                    }
                }
            }
            write_ledger(conn, &item.binding_id, item.tier, window, &value)?;
        }
        item.frozen = frozen;
        item.state = if !frozen.empty() {
            AttemptState::Uncertain
        } else if [item.actual.tokens, item.actual.usd_micros]
            .into_iter()
            .flatten()
            .any(|n| n > 0)
        {
            AttemptState::Committed
        } else {
            AttemptState::Released
        };
        revision(item)?;
    }
    if let Some(charge) = &mut item.loop_charge {
        loop_guard::settle(
            conn,
            &item.binding_id,
            item.session.as_deref().ok_or_else(invalid)?,
            charge,
            item.actual,
            item.frozen,
        )?;
    }
    write_attempt(conn, item)
}

/// Called in the lease transaction only after trusted history advances the head.
pub(super) fn note_head(
    conn: &Connection,
    binding: &str,
    session: &str,
    turn: &str,
    seq: i64,
) -> Result<(), ConfigError> {
    conn.execute(
        "INSERT OR IGNORE INTO agent_budget_route_states(binding,session,turn,seq)
         SELECT ?1,?2,?3,?4 WHERE EXISTS(SELECT 1 FROM agent_budget_policies WHERE binding=?1)",
        params![binding, session, turn, seq],
    )
    .map_err(db_error)?;
    Ok(())
}

type Route = (i64, Option<String>, Option<String>, bool);
fn saved_route(
    conn: &Connection,
    binding: &str,
    session: &str,
    turn: &str,
) -> Result<Option<Route>, ConfigError> {
    conn.query_row(
        "SELECT seq,tier,request_id,output_confirmed FROM agent_budget_route_states WHERE binding=?1 AND session=?2 AND turn=?3",
        params![binding,session,turn],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
    ).optional().map_err(db_error)
}

fn eligible(
    conn: &Connection,
    binding: &str,
    session: &str,
    turn: &str,
    seq: i64,
) -> Result<bool, ConfigError> {
    let head:Option<i64>=conn.query_row(
        "SELECT head_seq FROM gateway_session_watermarks WHERE binding=?1 AND session=?2 AND archived=0",
        params![binding,session],|row|row.get(0)
    ).optional().map_err(db_error)?;
    if head != Some(seq) {
        return fail(
            "route_turn_superseded",
            "此待选路轮次已经被新的用户请求取代",
        );
    }
    conn.query_row(
        "SELECT NOT EXISTS(SELECT 1 FROM agent_budget_reservations
         WHERE binding=?1 AND json_extract(value,'$.session')=?2 AND json_extract(value,'$.turn')=?3
           AND json_extract(value,'$.sent')=1)
           AND NOT EXISTS(SELECT 1 FROM service_route_attempts WHERE binding=?1 AND session=?2 AND turn=?3
           AND outcome NOT IN ('preparing','not_sent','deadline_before_send'))",
        params![binding,session,turn],|row|row.get(0),
    ).map_err(db_error)
}

pub enum Quote {
    Primary,
    Fallback,
    TryFallback(String),
}

impl Store {
    pub fn budget_quote(&self, p: &Prepare<'_>) -> Result<Quote, ConfigError> {
        let Some(policy) = self
            .budget_policy(p.binding_id)?
            .filter(|p| !p.windows.is_empty())
        else {
            return Ok(Quote::Primary);
        };
        let keys = keys(&policy, p.now, p.session)?;
        if let Some(session) = p.session
            && let Some((seq, tier, _, _)) =
                saved_route(&self.connection, p.binding_id, session, p.turn)?
        {
            if let Some(tier) = tier {
                return Ok(if Tier::parse(&tier)? == Tier::Fallback {
                    Quote::Fallback
                } else {
                    Quote::Primary
                });
            }
            if p.continuation {
                return fail(
                    "dispatch_not_committed",
                    "本轮尚未完成预算选路，工具续调不能恢复首次决定",
                );
            }
            if let Some(fallback) = policy.fallback_service_id
                && eligible(&self.connection, p.binding_id, session, p.turn, seq)?
                && windows(&self.connection, p, &keys, Tier::Primary)
                    .is_err_and(|e| e.code == "local_agent_budget_exceeded")
            {
                return Ok(Quote::TryFallback(fallback));
            }
        }
        Ok(Quote::Primary)
    }

    pub fn budget_fallback_candidate(
        &self,
        binding: &str,
        session: &str,
        id: &str,
    ) -> Result<super::Selection, ConfigError> {
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let service = crate::model_services::current(&tx, id)?;
        let attachment = crate::model_services::ServiceAttachment {
            binding_id: binding.into(),
            service_id: id.into(),
            installed: service.clone(),
            previous_endpoint: None,
            previous_codex_endpoints: Vec::new(),
        };
        let (_, target, mut selection) = attachment.resolve(&tx, super::ALIAS)?;
        let version:Option<String>=tx.query_row(
            "SELECT route FROM model_service_targets WHERE binding=?1 AND version=?2 AND model=?3",
            params![binding,service.version_id,target.id],|row|row.get(0)
        ).optional().map_err(db_error)?;
        let version = if let Some(version) = version {
            super::version(&tx, &version)?
        } else {
            let route = super::insert_version_for_turn(&tx, binding, &target.provider, session)?;
            tx.execute(
                "INSERT INTO model_service_targets VALUES(?1,?2,?3,?4)",
                params![binding, service.version_id, target.id, route.id],
            )
            .map_err(db_error)?;
            route
        };
        selection.fallback_reason = Some("budget_downgraded".into());
        tx.commit().map_err(db_error)?;
        Ok(super::Selection {
            fresh: service.routing_policy.enabled,
            advances_session_head: false,
            version,
            reason: "budget_downgraded".into(),
            service: Some(selection),
            uncommitted_dispatch: None,
        })
    }
}

pub(crate) fn can_replan_fallback(
    conn: &Connection,
    binding: &str,
    session: &str,
    turn: &str,
) -> Result<bool, ConfigError> {
    if let Some((seq, None, _, _)) = saved_route(conn, binding, session, turn)? {
        return eligible(conn, binding, session, turn, seq);
    }
    Ok(false)
}

fn select_route(
    conn: &Connection,
    p: &Prepare<'_>,
    policy: &Policy,
    keys: &[(WindowPolicy, String)],
) -> Result<(), ConfigError> {
    let Some(session) = p.session else {
        return if p.selected_tier == Tier::Fallback {
            Err(exhausted("local_agent_budget_exceeded"))
        } else {
            Ok(())
        };
    };
    let state = saved_route(conn, p.binding_id, session, p.turn)?;
    if let Some((seq, Some(ref tier), ref owner, confirmed)) = state {
        if Tier::parse(tier)? != p.selected_tier || seq == 0 {
            return Err(invalid());
        }
        if p.continuation && !confirmed {
            return fail(
                "dispatch_not_committed",
                "上游尚未确认本轮输出，不能开始工具续调",
            );
        }
        if !p.continuation
            && p.selected_tier == Tier::Fallback
            && owner.as_deref() != Some(p.request_id)
            && !eligible(conn, p.binding_id, session, p.turn, seq)?
        {
            return fail(
                "dispatch_uncertain",
                "原轮次已可能发送，不能重新派发备用服务",
            );
        }
        return Ok(());
    }
    let Some((seq, None, _, _)) = state else {
        return if p.selected_tier == Tier::Fallback {
            Err(exhausted("local_agent_budget_exceeded"))
        } else {
            Ok(())
        };
    };
    if p.continuation || !eligible(conn, p.binding_id, session, p.turn, seq)? {
        return fail(
            "dispatch_not_committed",
            "本轮已有发送证据或缺少首次预算选路权限",
        );
    }
    if p.selected_tier == Tier::Fallback {
        let (selection, primary_estimate) = p.fallback.as_ref().ok_or_else(invalid)?;
        let service = selection.service.as_ref().ok_or_else(invalid)?;
        if policy.fallback_service_id.as_deref() != Some(service.service_id.as_str())
            || crate::model_services::current(conn, &service.service_id)?.version_id
                != service.service_version_id
        {
            return fail("conflict", "备用服务版本已变化，请重新读取");
        }
        let primary = Prepare {
            estimate: *primary_estimate,
            ..*p
        };
        if !windows(conn, &primary, keys, Tier::Primary)
            .is_err_and(|error| error.code == "local_agent_budget_exceeded")
        {
            return fail("conflict", "主服务现已可准入，请重试原轮次");
        }
        if conn
            .execute(
                "UPDATE gateway_route_leases SET version=?2,service_selection=?3 WHERE seq=?1",
                params![seq, selection.version.id, encode(service)?],
            )
            .map_err(db_error)?
            != 1
        {
            return Err(invalid());
        }
    }
    if conn
        .execute(
            "UPDATE agent_budget_route_states SET tier=?4,request_id=?5
       WHERE binding=?1 AND session=?2 AND turn=?3 AND tier IS NULL",
            params![
                p.binding_id,
                session,
                p.turn,
                p.selected_tier.key(),
                p.request_id
            ],
        )
        .map_err(db_error)?
        != 1
    {
        return Err(invalid());
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowStatus {
    pub kind: WindowKind,
    pub key: String,
    pub tier: Tier,
    pub limit: Amounts,
    pub ledger: Ledger,
    pub warned: bool,
}

impl Store {
    pub fn budget_window_status(
        &self,
        binding: &str,
        now: DateTime<Utc>,
    ) -> Result<Vec<WindowStatus>, ConfigError> {
        let Some(policy) = self.budget_policy(binding)? else {
            return Ok(Vec::new());
        };
        let local = now.with_timezone(
            &policy
                .timezone
                .parse::<chrono_tz::Tz>()
                .map_err(|_| invalid())?,
        );
        let mut output = Vec::new();
        for window in policy.windows {
            for (tier, limit) in [
                (Tier::Primary, window.primary),
                (Tier::Fallback, window.fallback),
            ] {
                if limit.empty() {
                    continue;
                }
                let keys = match window.kind {
                    WindowKind::Daily => vec![local.format("%Y-%m-%d").to_string()],
                    WindowKind::Monthly => vec![local.format("%Y-%m").to_string()],
                    WindowKind::Session => {
                        let mut stmt=self.connection.prepare("SELECT window_key FROM agent_budget_ledgers
                            WHERE binding=?1 AND tier=?2 AND window_kind='session' ORDER BY rowid DESC LIMIT 200").map_err(db_error)?;
                        let keys = stmt
                            .query_map(params![binding, tier.key()], |row| row.get::<_, String>(0))
                            .map_err(db_error)?;
                        keys.collect::<Result<Vec<_>, _>>().map_err(db_error)?
                    }
                };
                for key in keys {
                    let value = ledger(&self.connection, binding, tier, window.kind, &key)?;
                    let warned = warning(value, limit, policy.warn_percent);
                    output.push(WindowStatus {
                        kind: window.kind,
                        key,
                        tier,
                        limit,
                        ledger: value,
                        warned,
                    });
                }
            }
        }
        Ok(output)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Warning {
    pub id: String,
    pub binding_id: String,
    pub tier: String,
    pub window_kind: String,
    pub window_key: String,
    pub created_at: String,
}

impl Store {
    pub fn budget_warnings(&self, pending: bool) -> Result<Vec<Warning>, ConfigError> {
        let mut stmt = self
            .connection
            .prepare(
                "SELECT id,binding,tier,window_kind,window_key,created FROM agent_budget_warnings
             WHERE NOT ?1 OR notified=0 ORDER BY rowid DESC LIMIT 64",
            )
            .map_err(db_error)?;
        let rows = stmt
            .query_map([pending], |row| {
                Ok(Warning {
                    id: row.get(0)?,
                    binding_id: row.get(1)?,
                    tier: row.get(2)?,
                    window_kind: row.get(3)?,
                    window_key: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })
            .map_err(db_error)?;
        rows.collect::<Result<_, _>>().map_err(db_error)
    }

    pub fn budget_warnings_notified(
        &self,
        store_id: &str,
        ids: &[String],
    ) -> Result<(), ConfigError> {
        if self.database_id()? != store_id
            || ids.len() > 64
            || ids.iter().any(|id| uuid::Uuid::parse_str(id).is_err())
        {
            return fail("conflict", "预算预警确认范围已变化");
        }
        self.connection.execute("UPDATE agent_budget_warnings SET notified=1 WHERE id IN (SELECT value FROM json_each(?1))",
            [encode(&ids)?]).map_err(db_error)?;
        Ok(())
    }
}
