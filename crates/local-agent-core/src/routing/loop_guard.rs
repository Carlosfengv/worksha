use super::{ConfigError, Store, Turn, budget::Amounts, db_error, decode, encode, fail};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const MAX_BATCHES: usize = 256;
const MAX_TOOL_IDS: usize = 512;
const CIRCUIT: &str = "local_agent_loop_circuit_breaker";
const CAPACITY: &str = "local_agent_loop_dedup_capacity";

pub(crate) fn initialize(db: &Connection) -> Result<(), ConfigError> {
    let tx = Transaction::new_unchecked(db, TransactionBehavior::Immediate).map_err(db_error)?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS gateway_loop_guards(
            binding TEXT NOT NULL REFERENCES gateway_bindings(id), session TEXT NOT NULL,
            turn TEXT NOT NULL, turn_seq INTEGER NOT NULL, state TEXT NOT NULL,
            value TEXT NOT NULL CHECK(json_valid(value)), PRIMARY KEY(binding,session));
         CREATE INDEX IF NOT EXISTS gateway_loop_attention ON gateway_loop_guards(state,binding,session)
            WHERE state <> 'normal';
         CREATE TABLE IF NOT EXISTS gateway_loop_usage(
            binding TEXT NOT NULL REFERENCES gateway_bindings(id), session TEXT NOT NULL,
            turn_seq INTEGER NOT NULL, override_epoch INTEGER NOT NULL,
            value TEXT NOT NULL CHECK(json_valid(value)), PRIMARY KEY(binding,session,turn_seq,override_epoch));
         CREATE TABLE IF NOT EXISTS gateway_loop_overrides(
            operation TEXT PRIMARY KEY, request TEXT NOT NULL CHECK(json_valid(request)),
            receipt TEXT NOT NULL CHECK(json_valid(receipt)));",
    ).map_err(db_error)?;
    tx.commit().map_err(db_error)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    #[default]
    Normal,
    Warned,
    Tripped,
}
impl Status {
    fn key(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Warned => "warned",
            Self::Tripped => "tripped",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Reason {
    ToolErrorStreak,
    ActionPingPong,
    UnattendedBurn,
    DedupCapacity,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Guard {
    override_epoch: u32,
    status: Status,
    reason: Option<Reason>,
    errors: u32,
    steps: u32,
    steps_baseline: u32,
    seen_batch_fingerprints: BTreeSet<String>,
    seen_tool_call_ids: BTreeSet<String>,
    actions: BTreeMap<String, u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Batch {
    pub fingerprint: String,
    pub calls: Vec<Call>,
    pub has_error: bool,
    pub has_success: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub id_hash: String,
    pub action_hash: String,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub committed: Amounts,
    pub reserved: Amounts,
    pub frozen: Amounts,
    pub usd_unknown: u64,
}

impl Usage {
    fn replace(&mut self, previous: Usage, next: Usage) -> Result<(), ConfigError> {
        fn update(total: &mut Option<u64>, from: Option<u64>, to: Option<u64>) -> Option<()> {
            let value = total
                .unwrap_or(0)
                .checked_sub(from.unwrap_or(0))?
                .checked_add(to.unwrap_or(0))?;
            *total = (value != 0).then_some(value);
            Some(())
        }
        for (total, from, to) in [
            (&mut self.committed, previous.committed, next.committed),
            (&mut self.reserved, previous.reserved, next.reserved),
            (&mut self.frozen, previous.frozen, next.frozen),
        ] {
            update(&mut total.tokens, from.tokens, to.tokens).ok_or_else(invalid)?;
            update(&mut total.usd_micros, from.usd_micros, to.usd_micros).ok_or_else(invalid)?;
        }
        self.usd_unknown = self
            .usd_unknown
            .checked_sub(previous.usd_unknown)
            .and_then(|n| n.checked_add(next.usd_unknown))
            .ok_or_else(invalid)?;
        Ok(())
    }

    fn totals(self) -> (u64, u64) {
        [self.committed, self.reserved, self.frozen]
            .into_iter()
            .fold((0u64, 0u64), |(tokens, usd), a| {
                (
                    tokens.saturating_add(a.tokens.unwrap_or(0)),
                    usd.saturating_add(a.usd_micros.unwrap_or(0)),
                )
            })
    }
}

fn invalid() -> ConfigError {
    ConfigError::new("routing_unavailable", "本机熔断计量暂不可用")
}

impl Guard {
    fn blocked(&self) -> Option<ConfigError> {
        (self.status == Status::Tripped).then(|| {
            if self.reason == Some(Reason::DedupCapacity) {
                ConfigError::new(CAPACITY, "工具去重集合已满，请发送新用户指令或开启新会话")
            } else {
                ConfigError::new(
                    CIRCUIT,
                    "当前会话轮次已熔断，请发送新用户指令或在桌面端放行本轮",
                )
            }
        })
    }

    fn apply(&mut self, batch: &Batch) {
        if self.seen_batch_fingerprints.contains(&batch.fingerprint) {
            return;
        }
        let incoming: BTreeSet<_> = batch
            .calls
            .iter()
            .map(|call| &call.id_hash)
            .filter(|id| !self.seen_tool_call_ids.contains(*id))
            .collect();
        if self.seen_batch_fingerprints.len() >= MAX_BATCHES
            || self.seen_tool_call_ids.len() + incoming.len() > MAX_TOOL_IDS
        {
            self.status = Status::Tripped;
            self.reason = Some(Reason::DedupCapacity);
            return;
        }
        self.seen_batch_fingerprints
            .insert(batch.fingerprint.clone());
        self.steps += 1;
        match (batch.has_error, batch.has_success) {
            (true, false) => self.errors += 1,
            (false, true) => self.errors = 0,
            _ => (),
        }
        for call in &batch.calls {
            if self.seen_tool_call_ids.insert(call.id_hash.clone()) {
                *self.actions.entry(call.action_hash.clone()).or_default() += 1;
            }
        }
    }

    fn evaluate(&mut self, usage: Usage) {
        if self.status == Status::Tripped {
            return;
        }
        let action = self.actions.values().copied().max().unwrap_or(0);
        let steps = self.steps - self.steps_baseline;
        let (tokens, usd) = usage.totals();
        let burn_tripped = if self.override_epoch == 0 {
            steps >= 45 || tokens >= 500_000 || usd >= 3_000_000
        } else {
            steps >= 20 || tokens >= 250_000 || usd >= 1_500_000
        };
        let trip = [
            (self.errors >= 5, Reason::ToolErrorStreak),
            (action >= 6, Reason::ActionPingPong),
            (burn_tripped, Reason::UnattendedBurn),
        ];
        let warn = [
            (self.errors >= 3, Reason::ToolErrorStreak),
            (action >= 4, Reason::ActionPingPong),
            (
                steps >= 25 || tokens >= 250_000 || usd >= 1_500_000,
                Reason::UnattendedBurn,
            ),
        ];
        (self.status, self.reason) =
            if let Some((_, reason)) = trip.into_iter().find(|(hit, _)| *hit) {
                (Status::Tripped, Some(reason))
            } else if let Some((_, reason)) = warn.into_iter().find(|(hit, _)| *hit) {
                (Status::Warned, Some(reason))
            } else {
                (Status::Normal, None)
            };
    }
}

struct Head {
    digest: String,
    seq: i64,
    guard: Guard,
}

fn head(db: &Connection, binding: &str, session: &str) -> Result<Option<Head>, ConfigError> {
    let row: Option<(String, i64, String)> = db
        .query_row(
            "SELECT turn,turn_seq,value FROM gateway_loop_guards WHERE binding=?1 AND session=?2",
            params![binding, session],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(db_error)?;
    row.map(|(digest, seq, value)| {
        Ok(Head {
            digest,
            seq,
            guard: decode(&value)?,
        })
    })
    .transpose()
}

pub(super) fn tripped_sequence(
    db: &Connection,
    binding: &str,
    session: &str,
) -> Result<Option<i64>, ConfigError> {
    db.query_row(
        "SELECT g.turn_seq FROM gateway_loop_guards g JOIN gateway_session_watermarks w USING(binding,session)
         WHERE g.binding=?1 AND g.session=?2 AND g.state='tripped' AND w.archived=0",
        params![binding, session], |row| row.get(0),
    ).optional().map_err(db_error)
}

pub(super) fn check_advance(
    db: &Connection,
    binding: &str,
    session: &str,
    advancing: Option<i64>,
) -> Result<(), ConfigError> {
    if let Some(seq) = tripped_sequence(db, binding, session)?
        && advancing.is_none_or(|new| new <= seq)
        && let Some(error) = head(db, binding, session)?.and_then(|head| head.guard.blocked())
    {
        return Err(error);
    }
    Ok(())
}

fn save(db: &Connection, binding: &str, session: &str, head: &Head) -> Result<(), ConfigError> {
    db.execute(
        "UPDATE gateway_loop_guards SET state=?3,value=?4 WHERE binding=?1 AND session=?2 AND turn_seq=?5",
        params![binding,session,head.guard.status.key(),encode(&head.guard)?,head.seq],
    ).map_err(db_error)?;
    Ok(())
}

/// Runs in the trusted lease transaction. The strict sequence comparison also
/// protects a previously unvalidated lease that is corrected out of order.
pub(super) fn note_head(
    db: &Connection,
    binding: &str,
    session: &str,
    digest: &str,
    seq: i64,
) -> Result<(), ConfigError> {
    db.execute(
        "INSERT INTO gateway_loop_guards VALUES(?1,?2,?3,?4,'normal',?5)
         ON CONFLICT(binding,session) DO UPDATE SET turn=excluded.turn,turn_seq=excluded.turn_seq,
           state=excluded.state,value=excluded.value WHERE excluded.turn_seq>gateway_loop_guards.turn_seq",
        params![binding,session,digest,seq,encode(&Guard::default())?],
    ).map_err(db_error)?;
    Ok(())
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Scope {
    // None identifies a request with a session but without an attested current turn.
    seq: Option<i64>,
}

impl Store {
    pub fn loop_admit_auxiliary(
        &self,
        binding: &str,
        session: Option<&str>,
    ) -> Result<Option<Scope>, ConfigError> {
        let Some(session) = session else {
            return Ok(None);
        };
        let active = head(&self.connection, binding, session)?;
        if let Some(error) = active.as_ref().and_then(|h| h.guard.blocked()) {
            return Err(error);
        }
        Ok(Some(Scope {
            seq: active.map(|h| h.seq),
        }))
    }

    pub fn loop_rejection(
        &self,
        binding: &str,
        session: Option<&str>,
    ) -> Result<Option<ConfigError>, ConfigError> {
        let Some(session) = session else {
            return Ok(None);
        };
        if tripped_sequence(&self.connection, binding, session)?.is_none() {
            return Ok(None);
        }
        Ok(head(&self.connection, binding, session)?.and_then(|head| head.guard.blocked()))
    }

    pub fn loop_admit(
        &self,
        binding: &str,
        turn: &Turn,
        batch: Option<&Batch>,
    ) -> Result<Option<Scope>, ConfigError> {
        let Some(session) = turn.session.as_deref() else {
            return Ok(None);
        };
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
            .map_err(db_error)?;
        // Existing installations can resume the last validated M3 turn. This
        // bootstrap only inserts a missing guard for the exact saved head.
        tx.execute(
            "INSERT OR IGNORE INTO gateway_loop_guards
               SELECT l.binding,l.session,l.turn,l.seq,'normal',?4
               FROM gateway_session_watermarks w JOIN gateway_route_leases l
                 ON l.seq=w.head_seq AND l.binding=w.binding AND l.session=w.session
               WHERE w.binding=?1 AND w.session=?2 AND l.turn=?3
               AND w.archived=0 AND l.trusted=1 AND l.output_validated=1",
            params![binding, session, turn.digest, encode(&Guard::default())?],
        )
        .map_err(db_error)?;
        let Some(mut current) = head(&tx, binding, session)? else {
            tx.commit().map_err(db_error)?;
            return Ok(Some(Scope { seq: None }));
        };
        if let Some(error) = current.guard.blocked() {
            return Err(error);
        }
        let seq = (current.digest == turn.digest).then_some(current.seq);
        if seq.is_some()
            && turn.continuation
            && let Some(batch) = batch
        {
            current.guard.apply(batch);
            current.guard.evaluate(usage(
                &tx,
                binding,
                session,
                current.seq,
                current.guard.override_epoch,
            )?);
            save(&tx, binding, session, &current)?;
        }
        let blocked = current.guard.blocked();
        tx.commit().map_err(db_error)?;
        match blocked {
            Some(error) => Err(error),
            None => Ok(Some(Scope { seq })),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(crate) struct Charge {
    seq: i64,
    epoch: u32,
    pub estimate: Amounts,
    usage: Usage,
}

fn usage(
    db: &Connection,
    binding: &str,
    session: &str,
    seq: i64,
    epoch: u32,
) -> Result<Usage, ConfigError> {
    let value: Option<String> = db.query_row(
        "SELECT value FROM gateway_loop_usage WHERE binding=?1 AND session=?2 AND turn_seq=?3 AND override_epoch=?4",
        params![binding, session, seq, epoch], |row| row.get(0),
    ).optional().map_err(db_error)?;
    value
        .as_deref()
        .map(decode)
        .transpose()
        .map(Option::unwrap_or_default)
}

fn save_usage(
    db: &Connection,
    binding: &str,
    session: &str,
    charge: &Charge,
    total: &Usage,
) -> Result<(), ConfigError> {
    db.execute(
        "INSERT INTO gateway_loop_usage VALUES(?1,?2,?3,?4,?5)
         ON CONFLICT(binding,session,turn_seq,override_epoch) DO UPDATE SET value=excluded.value",
        params![binding, session, charge.seq, charge.epoch, encode(total)?],
    )
    .map_err(db_error)?;
    Ok(())
}

pub(crate) fn begin(
    db: &Connection,
    binding: &str,
    session: &str,
    scope: Scope,
    estimate: Amounts,
) -> Result<Option<Charge>, ConfigError> {
    let current = head(db, binding, session)?;
    if let Some(error) = current.as_ref().and_then(|h| h.guard.blocked()) {
        return Err(error);
    }
    let Some(seq) = scope.seq else {
        return Ok(None);
    };
    let Some(mut current) = current.filter(|h| h.seq == seq) else {
        return fail("route_turn_superseded", "发送准入前已进入新的用户轮次");
    };
    let epoch = current.guard.override_epoch;
    let charge = Charge {
        seq,
        epoch,
        estimate,
        usage: Usage {
            reserved: estimate,
            usd_unknown: u64::from(estimate.usd_micros.is_none()),
            ..Usage::default()
        },
    };
    let mut total = usage(db, binding, session, seq, epoch)?;
    total.replace(Usage::default(), charge.usage)?;
    refresh(db, binding, session, &mut current, total)?;
    if let Some(error) = current.guard.blocked() {
        // The rejected reservation is never added to the ledger.
        return Err(error);
    }
    save_usage(db, binding, session, &charge, &total)?;
    Ok(Some(charge))
}

pub(crate) fn is_trip(error: &ConfigError) -> bool {
    matches!(error.code.as_str(), CIRCUIT | CAPACITY)
}

pub(crate) fn settle(
    db: &Connection,
    binding: &str,
    session: &str,
    charge: &mut Charge,
    actual: Amounts,
    frozen: Amounts,
) -> Result<(), ConfigError> {
    let next = Usage {
        committed: actual,
        reserved: Amounts::default(),
        frozen,
        usd_unknown: u64::from(charge.estimate.usd_micros.is_none() && actual.usd_micros.is_none()),
    };
    let mut total = usage(db, binding, session, charge.seq, charge.epoch)?;
    total.replace(charge.usage, next)?;
    charge.usage = next;
    save_usage(db, binding, session, charge, &total)?;
    if let Some(mut current) = head(db, binding, session)?
        .filter(|h| h.seq == charge.seq && h.guard.override_epoch == charge.epoch)
    {
        refresh(db, binding, session, &mut current, total)?;
    }
    Ok(())
}

fn refresh(
    db: &Connection,
    binding: &str,
    session: &str,
    head: &mut Head,
    usage: Usage,
) -> Result<(), ConfigError> {
    let previous = (head.guard.status, head.guard.reason);
    head.guard.evaluate(usage);
    if previous != (head.guard.status, head.guard.reason) {
        save(db, binding, session, head)?;
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Override {
    pub store_id: String,
    pub operation_id: String,
    pub binding_id: String,
    pub session_id: String,
    pub expected_turn_digest: String,
    pub expected_override_epoch: u32,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Receipt {
    pub operation_id: String,
    pub override_epoch: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Alert {
    binding_id: String,
    session_id: String,
    agent_id: String,
    binding_name: Option<String>,
    turn_digest: String,
    override_epoch: u32,
    status: Status,
    reason: Reason,
    continuation_steps: u32,
    consecutive_tool_errors: u32,
    repeated_actions: u32,
    usage: Usage,
}

impl Store {
    pub fn loop_alerts(&self) -> Result<Vec<Alert>, ConfigError> {
        let mut stmt = self
            .connection
            .prepare(
                "SELECT g.binding,g.session,b.agent,g.turn,g.turn_seq,g.value,k.name
             FROM gateway_loop_guards g JOIN gateway_bindings b ON b.id=g.binding
             JOIN gateway_session_watermarks w ON w.binding=g.binding AND w.session=g.session
             LEFT JOIN gateway_chat_keys k ON k.binding=g.binding
             WHERE g.state<>'normal' AND w.archived=0 AND b.scope NOT LIKE 'revoked:%'
               AND (k.binding IS NULL OR k.digest IS NOT NULL)
             ORDER BY (g.state='tripped') DESC,g.turn_seq DESC LIMIT 100",
            )
            .map_err(db_error)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            })
            .map_err(db_error)?;
        rows.map(|row| {
            let (binding_id, session_id, agent_id, turn_digest, seq, value, binding_name) =
                row.map_err(db_error)?;
            let guard: Guard = decode(&value)?;
            Ok(Alert {
                usage: usage(
                    &self.connection,
                    &binding_id,
                    &session_id,
                    seq,
                    guard.override_epoch,
                )?,
                binding_id,
                session_id,
                agent_id,
                binding_name,
                turn_digest,
                override_epoch: guard.override_epoch,
                status: guard.status,
                reason: guard.reason.ok_or_else(invalid)?,
                continuation_steps: guard.steps - guard.steps_baseline,
                consecutive_tool_errors: guard.errors,
                repeated_actions: guard.actions.values().copied().max().unwrap_or(0),
            })
        })
        .collect()
    }

    pub fn loop_override(&self, input: &Override) -> Result<Receipt, ConfigError> {
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
            .map_err(db_error)?;
        if self.database_id()? != input.store_id
            || !uuid::Uuid::parse_str(&input.operation_id).is_ok_and(|id| id.get_version_num() == 4)
        {
            return fail("conflict", "放行操作编号或数据目录已变化");
        }
        let request = encode(input)?;
        if let Some((saved, receipt)) = tx
            .query_row(
                "SELECT request,receipt FROM gateway_loop_overrides WHERE operation=?1",
                [&input.operation_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(db_error)?
        {
            if saved != request {
                return fail("conflict", "同一放行操作的参数已变化");
            }
            return decode(&receipt);
        }
        super::budget::is_active_local(&tx, &input.binding_id)?;
        let mut current = head(&tx, &input.binding_id, &input.session_id)?
            .ok_or_else(|| ConfigError::new("conflict", "熔断轮次已变化"))?;
        let active: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM gateway_session_watermarks WHERE binding=?1 AND session=?2 AND archived=0)",
            params![input.binding_id,input.session_id], |row| row.get(0),
        ).map_err(db_error)?;
        let guard = &mut current.guard;
        if !active
            || current.digest != input.expected_turn_digest
            || guard.override_epoch != input.expected_override_epoch
            || guard.status != Status::Tripped
        {
            return fail("conflict", "熔断轮次或放行序号已变化，请重新读取");
        }
        if guard.reason == Some(Reason::DedupCapacity) {
            return Err(guard.blocked().ok_or_else(invalid)?);
        }
        guard.override_epoch = guard.override_epoch.checked_add(1).ok_or_else(invalid)?;
        guard.status = Status::Normal;
        guard.reason = None;
        guard.errors = 0;
        guard.actions.retain(|_, count| *count < 6);
        guard.steps_baseline = guard.steps;
        let receipt = Receipt {
            operation_id: input.operation_id.clone(),
            override_epoch: guard.override_epoch,
        };
        save(&tx, &input.binding_id, &input.session_id, &current)?;
        tx.execute(
            "INSERT INTO gateway_loop_overrides VALUES(?1,?2,?3)",
            params![input.operation_id, request, encode(&receipt)?],
        )
        .map_err(db_error)?;
        tx.commit().map_err(db_error)?;
        Ok(receipt)
    }
}

#[cfg(test)]
mod tests;
