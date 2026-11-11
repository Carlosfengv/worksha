//! Persistent availability for official prepaid inference credentials.
use super::{ConfigError, Store, error};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};

#[derive(Clone, Debug)]
pub struct BalanceRejection {
    pub credential: String,
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum BalanceState {
    Open,
    Exhausted,
    Recovery(i64),
}

pub(super) fn record(
    db: &Connection,
    credential: &str,
    observed_at: DateTime<Utc>,
    available: bool,
) -> Result<(), ConfigError> {
    // Keep the first positive read's recovery identity across subsequent
    // refreshes so they cannot restart a successful or in-flight probe.
    db.execute(
        "INSERT INTO service_upstream_balance(credential,observed_at,exhausted,recovery_at)
         VALUES(?1,?2,?3,NULL) ON CONFLICT(credential) DO UPDATE SET
         observed_at=excluded.observed_at, exhausted=excluded.exhausted,
         recovery_at=CASE WHEN excluded.exhausted=1 THEN NULL
                          WHEN service_upstream_balance.exhausted=1 THEN excluded.observed_at
                          ELSE service_upstream_balance.recovery_at END
         WHERE service_upstream_balance.observed_at<excluded.observed_at
            OR (service_upstream_balance.observed_at=excluded.observed_at AND excluded.exhausted=1)",
        params![credential, observed_at.timestamp_micros(), !available],
    ).map_err(error)?;
    Ok(())
}

impl Store {
    pub fn record_upstream_balance(
        &self,
        credential: &str,
        observed_at: DateTime<Utc>,
        available: bool,
    ) -> Result<(), ConfigError> {
        record(&self.connection, credential, observed_at, available)
    }

    pub fn upstream_balance_state(&self, credential: &str) -> Result<BalanceState, ConfigError> {
        let mut read = self
            .connection
            .prepare_cached(
                "SELECT exhausted,recovery_at FROM service_upstream_balance WHERE credential=?1",
            )
            .map_err(error)?;
        Ok(read
            .query_row([credential], |row| {
                if row.get::<_, bool>(0)? {
                    return Ok(BalanceState::Exhausted);
                }
                Ok(row
                    .get::<_, Option<i64>>(1)?
                    .map_or(BalanceState::Open, BalanceState::Recovery))
            })
            .optional()
            .map_err(error)?
            .unwrap_or(BalanceState::Open))
    }
}
