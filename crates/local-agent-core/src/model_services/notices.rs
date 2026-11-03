//! Last confirmed target per session; no prompts, responses, or credentials.
use crate::{
    Store,
    providers::ConfigError,
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
        let action = if recovered {
            "已恢复使用"
        } else {
            "本轮已切换至"
        };
        let cause = match service.fallback_reason.as_deref() {
            Some("upstream_rate_limit" | "rate_limit_cooldown" | "configured_rate_limit") => {
                "首选来源限流，"
            }
            Some("upstream_failure" | "failure_cooldown" | "stream_interrupted_after_commit") => {
                "首选来源暂不可用，"
            }
            _ => "",
        };
        let notice = (allow && changed)
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
