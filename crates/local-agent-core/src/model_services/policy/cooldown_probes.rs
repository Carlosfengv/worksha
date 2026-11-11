use super::{ConfigError, Dispatch, QuotaProbeClaim, Store, error, fail};
use rusqlite::params;

pub struct HttpProbeClaim<'a> {
    pub owner: &'a str,
    pub scopes: [&'a str; 4],
    pub checked_at: i64,
    pub cooldown_ms: i64,
}

impl Store {
    /// Record all evidence for a possible upstream send in one transaction.
    pub fn service_begin_send(
        &self,
        attempt: Option<(&Dispatch, i64)>,
        http: Option<HttpProbeClaim<'_>>,
        quota: Option<QuotaProbeClaim<'_>>,
    ) -> Result<(), ConfigError> {
        if http.is_none() && quota.is_none() {
            return attempt.map_or(Ok(()), |(plan, id)| {
                self.service_attempt_begin_send(plan, id)
            });
        }
        let tx = self.connection.unchecked_transaction().map_err(error)?;
        if let Some((plan, attempt)) = attempt {
            self.service_attempt_begin_send(plan, attempt)?;
        }
        if let Some(http) = http {
            self.record_service_http_probe(http)?;
        }
        if let Some(quota) = quota {
            self.record_service_quota_probe(quota)?;
        }
        tx.commit().map_err(error)
    }

    fn record_service_http_probe(&self, claim: HttpProbeClaim<'_>) -> Result<(), ConfigError> {
        let HttpProbeClaim {
            owner,
            scopes: [account, model, access_account, access_model],
            checked_at,
            cooldown_ms,
        } = claim;
        let unsettled: bool = self
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM service_cooldown_probe_sends
                 WHERE scope IN (?2,?3,?4,?5) AND owner<>?1)",
                params![owner, account, model, access_account, access_model],
                |row| row.get(0),
            )
            .map_err(error)?;
        if unsettled {
            return fail("probe_unsettled", "上次恢复探测尚未完成核查，本次未发送");
        }
        self.connection
            .execute(
                "INSERT INTO service_cooldown_probe_sends(scope,owner,checked_at,cooldown_ms)
             SELECT scope,?1,?2,?3 FROM service_route_cooldowns
             WHERE scope IN (?4,?5,?6,?7) AND until_ms<=?2
             ON CONFLICT(scope) DO UPDATE SET owner=excluded.owner,
               checked_at=excluded.checked_at,cooldown_ms=excluded.cooldown_ms",
                params![
                    owner,
                    checked_at,
                    cooldown_ms,
                    account,
                    model,
                    access_account,
                    access_model
                ],
            )
            .map_err(error)?;
        Ok(())
    }

    pub fn service_clear_cooldown_probe(&self, owner: &str) -> Result<(), ConfigError> {
        self.connection
            .execute(
                "DELETE FROM service_cooldown_probe_sends WHERE owner=?1",
                [owner],
            )
            .map_err(error)?;
        Ok(())
    }

    /// Called under the directory lock before serving requests. A newer
    /// cooldown has better evidence and keeps its original deadline.
    pub fn interrupt_service_cooldown_probes(&self, now_ms: i64) -> Result<(), ConfigError> {
        self.interrupt_cooldown_probes(None, now_ms)
    }

    pub fn interrupt_service_cooldown_probe(
        &self,
        owner: &str,
        now_ms: i64,
    ) -> Result<(), ConfigError> {
        self.interrupt_cooldown_probes(Some(owner), now_ms)
    }

    fn interrupt_cooldown_probes(
        &self,
        owner: Option<&str>,
        now_ms: i64,
    ) -> Result<(), ConfigError> {
        let tx = self.connection.unchecked_transaction().map_err(error)?;
        tx.execute(
            "UPDATE service_route_cooldowns AS cooldown
             SET until_ms=max(cooldown.until_ms,?1+probe.cooldown_ms)
             FROM service_cooldown_probe_sends AS probe
             WHERE cooldown.scope=probe.scope AND cooldown.until_ms<=probe.checked_at
               AND (?2 IS NULL OR probe.owner=?2)",
            params![now_ms, owner],
        )
        .map_err(error)?;
        tx.execute(
            "DELETE FROM service_cooldown_probe_sends WHERE ?1 IS NULL OR owner=?1",
            [owner],
        )
        .map_err(error)?;
        tx.commit().map_err(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checkpoint(
        store: &Store,
        owner: &str,
        scopes: [&str; 4],
        checked_at: i64,
        cooldown_ms: i64,
    ) -> Result<(), ConfigError> {
        store.service_begin_send(
            None,
            Some(HttpProbeClaim {
                owner,
                scopes,
                checked_at,
                cooldown_ms,
            }),
            None,
        )
    }

    #[test]
    fn reopening_cools_only_unsettled_probe_scopes_once() {
        let dir = tempfile::tempdir().unwrap();
        let scopes = ["account", "model", "access-account", "access-model"];
        let store = Store::open(dir.path()).unwrap();
        store
            .service_set_cooldown("account", 800, "upstream_rate_limit")
            .unwrap();
        store
            .service_set_cooldown("model", 900, "upstream_failure")
            .unwrap();
        store
            .service_set_cooldown("access-model", 950, "upstream_model_access_denied")
            .unwrap();
        store
            .service_set_cooldown("unrelated", 700, "upstream_failure")
            .unwrap();
        checkpoint(&store, "probe", scopes, 1_000, 3_000).unwrap();
        store
            .service_set_cooldown("model", 2_100, "upstream_failure")
            .unwrap();
        store.service_clear_cooldown_probe("other-owner").unwrap();
        drop(store);

        let store = Store::open(dir.path()).unwrap();
        store.interrupt_service_cooldown_probes(2_000).unwrap();
        store.interrupt_service_cooldown_probes(2_500).unwrap();
        let rows = store.service_cooldowns().unwrap();
        assert_eq!(rows["account"], (5_000, "upstream_rate_limit".into()));
        assert_eq!(rows["model"], (2_100, "upstream_failure".into()));
        assert_eq!(
            rows["access-model"],
            (5_000, "upstream_model_access_denied".into())
        );
        assert_eq!(rows["unrelated"].0, 700);
    }

    #[test]
    fn unsettled_probe_cannot_be_replaced_and_interruption_is_owner_scoped() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let scopes = ["account", "model", "", ""];
        store
            .service_set_cooldown("account", 900, "upstream_rate_limit")
            .unwrap();
        checkpoint(&store, "old", scopes, 1_000, 3_000).unwrap();
        assert_eq!(
            checkpoint(&store, "new", scopes, 1_100, 4_000)
                .unwrap_err()
                .code,
            "probe_unsettled"
        );
        store
            .service_set_cooldown("other", 800, "upstream_failure")
            .unwrap();
        checkpoint(&store, "other", ["other", "", "", ""], 1_100, 4_000).unwrap();
        store
            .interrupt_service_cooldown_probe("old", 2_000)
            .unwrap();
        assert_eq!(store.service_cooldowns().unwrap()["account"].0, 5_000);
        assert_eq!(store.service_cooldowns().unwrap()["other"].0, 800);

        checkpoint(&store, "completed", scopes, 5_001, 3_000).unwrap();
        store.service_clear_cooldown_probe("completed").unwrap();
        store
            .interrupt_service_cooldown_probe("old", 6_000)
            .unwrap();
        store.interrupt_service_cooldown_probes(7_000).unwrap();
        assert_eq!(store.service_cooldowns().unwrap()["account"].0, 5_000);
        assert_eq!(store.service_cooldowns().unwrap()["other"].0, 11_000);
    }
}
