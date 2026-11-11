//! Last confirmed target per session; no prompts, responses, or credentials.
use super::ServiceSelection;
use crate::{
    Store,
    providers::{ConfigError, upstream_accounts::AccountRef},
    routing::{Selection, Turn},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

pub(crate) fn initialize(db: &Connection) -> Result<(), ConfigError> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS service_route_notices (
        binding TEXT NOT NULL, session TEXT NOT NULL, seq INTEGER NOT NULL,
        target TEXT NOT NULL CHECK(json_valid(target)), PRIMARY KEY(binding,session));",
    )
    .map_err(super::error)
}

#[derive(Serialize, Deserialize)]
struct Target {
    fingerprint: String,
    model: String,
    preferred: String,
    mode: String,
    #[serde(default)]
    issued_notices: Vec<String>,
}

fn priority_account(
    db: &Connection,
    selection: &ServiceSelection,
) -> Result<Option<AccountRef>, ConfigError> {
    let (version, model) = selection.nested_route.last().map_or(
        (&selection.service_version_id, &selection.model_id),
        |hop| (&hop.version_id, &hop.model_id),
    );
    let published = super::version(db, version)?;
    Ok(published
        .routing_policy
        .pool(model)
        // Rotating and affinity policies have no fixed first account.
        .filter(|pool| pool.strategy.is_priority() && pool.affinity.is_off())
        .and_then(|pool| pool.accounts.first())
        .cloned())
}

fn account_restored(
    db: &Connection,
    binding: &str,
    session: &str,
    seq: i64,
    current: &ServiceSelection,
) -> Result<bool, ConfigError> {
    let Some(account) = &current.upstream_account else {
        return Ok(false);
    };
    let previous: Option<String> = db
        .query_row(
            "SELECT l.service_selection FROM gateway_route_leases l
             JOIN service_route_dispatch d USING(binding,session,turn)
             WHERE l.seq=?1 AND l.binding=?2 AND l.session=?3 AND l.trusted=1
               AND d.state='committed' AND l.service_selection IS NOT NULL",
            params![seq, binding, session],
            |row| row.get(0),
        )
        .optional()
        .map_err(super::error)?;
    let Some(previous) = previous else {
        return Ok(false);
    };
    let previous: ServiceSelection = serde_json::from_str(&previous).map_err(super::error)?;
    let current_path = current
        .nested_route
        .iter()
        .map(|hop| (&hop.service_id, &hop.model_id));
    let previous_path = previous
        .nested_route
        .iter()
        .map(|hop| (&hop.service_id, &hop.model_id));
    Ok(previous
        .upstream_account
        .as_ref()
        .is_some_and(|old| old != account)
        && current.service_id == previous.service_id
        && current_path.eq(previous_path)
        && priority_account(db, current)?.as_ref() == Some(account)
        && priority_account(db, &previous)?.as_ref() == Some(account))
}

impl Store {
    /// Claim once only after a valid response starts. Old/replayed/tool turns cannot notify.
    /// Recording suppressed JSON responses still keeps the next recovery comparison accurate.
    pub fn model_service_route_notice(
        &self,
        binding: &str,
        turn: &Turn,
        selected: &Selection,
        allow: bool,
    ) -> Result<Option<String>, ConfigError> {
        let (Some(session), Some(service)) = (&turn.session, &selected.service) else {
            return Ok(None);
        };
        if !selected.fresh || turn.continuation || !turn.portable {
            return Ok(None);
        }
        let p = &selected.version.provider;
        let mut target = Target {
            fingerprint: crate::providers::files::hash(
                serde_json::to_string(&(
                    &p.id,
                    &p.endpoint,
                    &p.protocol,
                    &p.model,
                    &p.credential_ref,
                ))
                .map_err(super::error)?
                .as_bytes(),
            ),
            model: service.model_id.clone(),
            preferred: service
                .preferred_model_id
                .as_ref()
                .unwrap_or(&service.model_id)
                .clone(),
            mode: service.mode.clone(),
            issued_notices: vec![],
        };
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(super::error)?;
        let seq: i64 = tx
            .query_row(
                "SELECT seq FROM gateway_route_leases WHERE binding=?1 AND session=?2 AND turn=?3",
                params![binding, session, turn.digest],
                |r| r.get(0),
            )
            .map_err(super::error)?;
        let old: Option<(i64, String)> = tx
            .query_row(
                "SELECT seq,target FROM service_route_notices WHERE binding=?1 AND session=?2",
                params![binding, session],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(super::error)?;
        if old.as_ref().is_some_and(|(previous, _)| *previous >= seq) {
            return Ok(None);
        }
        if old.is_none() {
            let count: i64 = tx
                .query_row("SELECT count(*) FROM service_route_notices", [], |r| {
                    r.get(0)
                })
                .map_err(super::error)?;
            if count >= 100_000 {
                return Ok(None);
            }
        }
        let previous_seq = old.as_ref().map(|(seq, _)| *seq);
        let previous = old
            .map(|(_, s)| serde_json::from_str::<Target>(&s))
            .transpose()
            .map_err(super::error)?;
        let changed = previous
            .as_ref()
            .map_or(target.model != target.preferred, |old| {
                old.fingerprint != target.fingerprint
            });
        let recovered = previous.as_ref().is_some_and(|old| {
            old.preferred == target.preferred
                && old.mode == target.mode
                && old.model != old.preferred
                && target.model == target.preferred
        });
        // Labels are data: collapse controls/newlines and avoid Markdown interpretation.
        let label = |s: &str| -> String {
            s.chars()
                .filter(|c| !c.is_control())
                .map(|c| if "<>`*_\\".contains(c) { ' ' } else { c })
                .take(100)
                .collect::<String>()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        };
        let destination = format!("{} / {}", label(&p.name), label(&p.model));
        let choice = if service.mode == "specified" {
            "您指定的模型选择保持不变。"
        } else {
            ""
        };
        let cause = match service.fallback_reason.as_deref() {
            Some("upstream_rate_limit" | "rate_limit_cooldown" | "configured_rate_limit") => {
                "首选来源限流，"
            }
            Some("upstream_failure" | "failure_cooldown" | "stream_interrupted_after_commit") => {
                "首选来源暂不可用，"
            }
            Some("upstream_credential_rejected") => "上游拒绝首选账号的凭据，",
            Some("upstream_model_access_denied") => "上游拒绝首选账号访问模型，",
            Some("account_quota_exhausted") => "首选来源额度不可用，",
            _ => "",
        };
        let account_recovered = allow
            && cause.is_empty()
            && previous.as_ref().is_some_and(|old| {
                old.fingerprint == target.fingerprint
                    && old.model == target.model
                    && old.preferred == target.preferred
                    && old.mode == target.mode
            })
            && previous_seq.is_some_and(|seq| {
                account_restored(&tx, binding, session, seq, service).unwrap_or(false)
            });
        let action = if account_recovered {
            "已恢复使用首选账号，继续使用"
        } else if recovered {
            "已恢复使用"
        } else if !cause.is_empty()
            && target.model == target.preferred
            && service.upstream_account.is_some()
            && service.nested_route.is_empty()
        {
            "本轮已切换账号，继续使用"
        } else {
            "本轮已切换至"
        };
        // An initial fallback may keep the same model ID. A later turn can
        // try the primary again and fall back to the same backup.
        let fallback_this_turn = !cause.is_empty() && (previous.is_none() || service.attempt > 1);
        let notice = (allow && (changed || fallback_this_turn || account_recovered))
            .then(|| format!("【路由提示】{cause}{action} {destination}。{choice}\n\n"));
        target.issued_notices = previous.map(|p| p.issued_notices).unwrap_or_default();
        if let Some(text) = &notice {
            let hash = crate::providers::files::hash(text.trim().as_bytes());
            if !target.issued_notices.contains(&hash) {
                target.issued_notices.push(hash);
                if target.issued_notices.len() > 256 {
                    target.issued_notices.remove(0);
                }
            }
        }
        tx.execute("INSERT INTO service_route_notices(binding,session,seq,target) VALUES(?1,?2,?3,?4) ON CONFLICT(binding,session) DO UPDATE SET seq=excluded.seq,target=excluded.target",params![binding,session,seq,serde_json::to_string(&target).map_err(super::error)?]).map_err(super::error)?;
        tx.commit().map_err(super::error)?;
        Ok(notice)
    }

    pub fn model_service_notice_hashes(
        &self,
        binding: &str,
        session: &str,
    ) -> Result<Vec<String>, ConfigError> {
        let old: Option<String> = self
            .connection
            .query_row(
                "SELECT target FROM service_route_notices WHERE binding=?1 AND session=?2",
                params![binding, session],
                |r| r.get(0),
            )
            .optional()
            .map_err(super::error)?;
        Ok(old
            .map(|s| serde_json::from_str::<Target>(&s))
            .transpose()
            .map_err(super::error)?
            .map(|p| p.issued_notices)
            .unwrap_or_default())
    }
}
