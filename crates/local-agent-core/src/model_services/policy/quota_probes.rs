//! Single-flight recovery claims for confirmed upstream quota resets.
use super::{ConfigError, MAX_ROUTE_DEADLINE_SECONDS, Store, error};
use rusqlite::{OptionalExtension, params};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuotaProbeState {
    Active,
    Recovered(Option<i64>),
    Cooling,
}

#[derive(Clone, Copy, Default)]
pub enum QuotaProbeSource {
    #[default]
    OpenRouter,
    MiniMax,
    Balance,
}

pub struct QuotaProbeClaim<'a> {
    pub source: QuotaProbeSource,
    pub owner: &'a str,
    pub scopes: &'a [(&'a str, Option<i64>, i64)],
    pub now_ms: i64,
    pub cooldown_ms: i64,
    pub stream_window: Duration,
}

impl QuotaProbeSource {
    fn table(self) -> &'static str {
        match self {
            Self::OpenRouter => "service_openrouter_probes",
            Self::MiniMax => "service_minimax_probes",
            Self::Balance => "service_balance_probes",
        }
    }

    fn revision(self, store: &Store) -> Result<i64, ConfigError> {
        match self {
            Self::OpenRouter => store.quota_source_revision(),
            Self::MiniMax | Self::Balance => Ok(0),
        }
    }
}

impl Store {
    pub fn service_quota_probes<'a>(
        &self,
        source: QuotaProbeSource,
        scopes: impl Iterator<Item = &'a str>,
        now_ms: i64,
    ) -> Result<Vec<Option<QuotaProbeState>>, ConfigError> {
        let revision = source.revision(self)?;
        let table = source.table();
        let mut read = self.connection.prepare_cached(&format!(
            "SELECT state,reset_at,until_ms FROM {table} WHERE scope=?1 AND composition_revision=?2",
        )).map_err(error)?;
        scopes
            .map(|scope| {
                read.query_row(params![scope, revision], |row| {
                    Ok(match row.get::<_, String>(0)?.as_str() {
                        "active" if row.get::<_, i64>(2)? > now_ms => Some(QuotaProbeState::Active),
                        "recovered" => Some(QuotaProbeState::Recovered(row.get(1)?)),
                        "cooling" if row.get::<_, i64>(2)? > now_ms => {
                            Some(QuotaProbeState::Cooling)
                        }
                        _ => None,
                    })
                })
                .optional()
                .map(Option::flatten)
                .map_err(error)
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn reserve_service_quota_probe(
        &self,
        source: QuotaProbeSource,
        owner: &str,
        scopes: &[(&str, Option<i64>, i64)],
        now_ms: i64,
        cooldown_ms: i64,
    ) -> Result<(), ConfigError> {
        self.service_begin_send(
            None,
            None,
            Some(QuotaProbeClaim {
                source,
                owner,
                scopes,
                now_ms,
                cooldown_ms,
                stream_window: Duration::ZERO,
            }),
        )
    }

    // Called by service_begin_send within its transaction, under the same
    // Store mutex as the service's quota admission check.
    pub(super) fn record_service_quota_probe(
        &self,
        claim: QuotaProbeClaim<'_>,
    ) -> Result<(), ConfigError> {
        let QuotaProbeClaim {
            source,
            owner,
            scopes,
            now_ms,
            cooldown_ms,
            stream_window,
        } = claim;
        let revision = source.revision(self)?;
        let table = source.table();
        let count_sql = format!("SELECT count(*) FROM {table}");
        let count = || {
            self.connection
                .query_row(&count_sql, [], |r| r.get::<_, i64>(0))
        };
        if count().map_err(error)? + scopes.len() as i64 > 4096 {
            self.connection.execute(
                &format!("DELETE FROM {table} WHERE composition_revision<>?1 OR (valid_until<?2 AND until_ms<=?2)"),
                params![revision, now_ms],
            ).map_err(error)?;
        }
        // Keep the hold through first-output preflight and the entire bounded
        // stream. Settlement normally releases it at the protocol terminal.
        let stream_ms = i64::try_from(stream_window.as_millis()).unwrap_or(i64::MAX);
        let until_ms = now_ms
            .saturating_add((MAX_ROUTE_DEADLINE_SECONDS * 1000) as i64)
            .saturating_add(stream_ms)
            .saturating_add(cooldown_ms);
        let mut insert = self.connection.prepare_cached(&format!(
                "INSERT INTO {table}(scope,composition_revision,owner,reset_at,state,valid_until,until_ms,cooldown_ms)
                 VALUES(?1,?2,?3,?4,'active',?5,?6,?7) ON CONFLICT(scope) DO UPDATE SET
                 composition_revision=excluded.composition_revision,owner=excluded.owner,reset_at=excluded.reset_at,
                 state=excluded.state,valid_until=excluded.valid_until,until_ms=excluded.until_ms,cooldown_ms=excluded.cooldown_ms
                 WHERE {table}.composition_revision<>excluded.composition_revision OR {table}.until_ms<=?8",
        )).map_err(error)?;
        for (scope, reset, valid_until) in scopes {
            let written = insert
                .execute(params![
                    scope,
                    revision,
                    owner,
                    reset,
                    valid_until,
                    until_ms,
                    cooldown_ms,
                    now_ms
                ])
                .map_err(error)?;
            if written == 0 {
                return Err(ConfigError::new(
                    "quota_probe_busy",
                    "账号额度恢复资格已变化",
                ));
            }
        }
        drop(insert);
        if count().map_err(error)? > 4096 {
            return Err(ConfigError::new(
                "quota_probe_capacity",
                "账号额度恢复记录已达上限",
            ));
        }
        Ok(())
    }

    pub fn settle_service_quota_probe(
        &self,
        source: QuotaProbeSource,
        owner: &str,
        recovered: bool,
        now_ms: i64,
        cooldown_ms: i64,
    ) -> Result<(), ConfigError> {
        let table = source.table();
        self.connection
            .execute(
                &format!(
                    "UPDATE {table} SET state=?1,until_ms=?2 WHERE owner=?3 AND state='active'"
                ),
                params![
                    if recovered { "recovered" } else { "cooling" },
                    if recovered {
                        0
                    } else {
                        now_ms.saturating_add(cooldown_ms)
                    },
                    owner,
                ],
            )
            .map_err(error)?;
        Ok(())
    }

    pub fn retain_service_quota_recovery(
        &self,
        source: QuotaProbeSource,
        scope: &str,
        reset_at: i64,
        valid_until: i64,
    ) -> Result<(), ConfigError> {
        // Keep a still-used recovery out of capacity pruning. Refresh in
        // batches of 30 seconds to avoid writing for every ordinary request.
        let table = source.table();
        self.connection.execute(
            &format!("UPDATE {table} SET valid_until=?1 WHERE scope=?2 AND reset_at=?3 AND state='recovered' AND valid_until+30000<?1"),
            params![valid_until, scope, reset_at],
        ).map_err(error)?;
        Ok(())
    }

    // Invoked once under the service's directory lock, before the gateway starts.
    pub fn interrupt_service_quota_probes(&self, now_ms: i64) -> Result<(), ConfigError> {
        let tx = self.connection.unchecked_transaction().map_err(error)?;
        for source in [
            QuotaProbeSource::OpenRouter,
            QuotaProbeSource::MiniMax,
            QuotaProbeSource::Balance,
        ] {
            let table = source.table();
            tx.execute(
                &format!("UPDATE {table} SET state='cooling',until_ms=min(until_ms,?1+cooldown_ms) WHERE state='active'"),
                [now_ms],
            ).map_err(error)?;
        }
        tx.commit().map_err(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reopening_preserves_recovery_and_cools_interrupted_scopes_once() {
        for source in [
            QuotaProbeSource::OpenRouter,
            QuotaProbeSource::MiniMax,
            QuotaProbeSource::Balance,
        ] {
            verify_reopening(source);
        }
    }

    fn verify_reopening(source: QuotaProbeSource) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let scopes = ["key", "member", "workspace", "other"];
        let claim = [("key", Some(10), 400_000), ("member", Some(11), 400_000)];
        store
            .reserve_service_quota_probe(source, "sent", &claim, 20_000, 30_000)
            .unwrap();
        assert!(
            store
                .reserve_service_quota_probe(
                    source,
                    "competing",
                    &[("workspace", Some(12), 400_000), claim[1]],
                    21_000,
                    30_000
                )
                .is_err()
        );
        assert_eq!(
            store
                .service_quota_probes(source, scopes.into_iter(), 21_000)
                .unwrap(),
            [
                Some(QuotaProbeState::Active),
                Some(QuotaProbeState::Active),
                None,
                None
            ]
        );
        assert_eq!(
            store
                .service_quota_probes(source, ["key", "member"].into_iter(), 380_000)
                .unwrap(),
            [None, None],
            "ordinary sends retain a bounded non-stream hold"
        );
        store
            .reserve_service_quota_probe(
                source,
                "success",
                &[("workspace", Some(12), 400_000)],
                20_000,
                30_000,
            )
            .unwrap();
        store
            .settle_service_quota_probe(source, "success", true, 21_000, 0)
            .unwrap();
        drop(store);
        let store = Store::open(dir.path()).unwrap();
        store.interrupt_service_quota_probes(22_000).unwrap();
        assert_eq!(
            store
                .service_quota_probes(source, scopes.into_iter(), 23_000)
                .unwrap(),
            [
                Some(QuotaProbeState::Cooling),
                Some(QuotaProbeState::Cooling),
                Some(QuotaProbeState::Recovered(Some(12))),
                None
            ]
        );
        // A late settlement from a cancelled owner cannot override the gate.
        store
            .settle_service_quota_probe(source, "sent", true, 25_000, 0)
            .unwrap();
        drop(store);
        let store = Store::open(dir.path()).unwrap();
        store.interrupt_service_quota_probes(50_000).unwrap();
        assert_eq!(
            store
                .service_quota_probes(source, scopes.into_iter(), 52_000)
                .unwrap(),
            [None, None, Some(QuotaProbeState::Recovered(Some(12))), None]
        );
        store
            .reserve_service_quota_probe(source, "retry", &claim, 52_000, 30_000)
            .unwrap();
        store
            .settle_service_quota_probe(source, "sent", false, 53_000, 60_000)
            .unwrap();
        assert_eq!(
            store
                .service_quota_probes(source, scopes.into_iter(), 53_000)
                .unwrap()[0],
            Some(QuotaProbeState::Active)
        );
    }
}
