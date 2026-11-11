//! Durable admission evidence for the two independent MiniMax text windows.
use super::{ConfigError, Store, error};
use crate::billing::{MINIMAX_PLAN_WINDOWS, QuotaWindow};
use rusqlite::params;

impl Store {
    pub fn record_minimax_allowance(
        &self,
        scope: &str,
        windows: &[QuotaWindow],
        observed: chrono::DateTime<chrono::FixedOffset>,
    ) -> Result<(), ConfigError> {
        let tx = self.connection.unchecked_transaction().map_err(error)?;
        for window in windows
            .iter()
            .filter(|w| MINIMAX_PLAN_WINDOWS.contains(&w.id.as_str()))
        {
            let Some(remaining) = window
                .remaining_percent
                .filter(|p| (0.0..=100.0).contains(p))
            else {
                continue;
            };
            if window
                .resets_at
                .is_some_and(|at| at <= observed.timestamp())
            {
                continue;
            }
            // Missing or expired windows cannot clear a previously known hold.
            // Retain the latest positive read as a fence against overlapping reads.
            tx.execute(
                "INSERT INTO service_minimax_allowance(scope,window,observed_at,exhausted,reset_at)
                 VALUES(?1,?2,?3,?4,?5) ON CONFLICT(scope,window) DO UPDATE SET
                 observed_at=excluded.observed_at,exhausted=excluded.exhausted,reset_at=excluded.reset_at
                 WHERE service_minimax_allowance.observed_at<=excluded.observed_at",
                params![scope, window.id, observed.timestamp_micros(), remaining == 0.0, window.resets_at],
            ).map_err(error)?;
        }
        tx.commit().map_err(error)
    }

    pub fn minimax_exhausted_windows(
        &self,
        scope: &str,
    ) -> Result<Vec<(String, Option<i64>)>, ConfigError> {
        let mut read = self.connection.prepare_cached(
            "SELECT window,reset_at FROM service_minimax_allowance WHERE scope=?1 AND exhausted=1",
        ).map_err(error)?;
        read.query_map([scope], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(error)?
            .collect::<Result<_, _>>()
            .map_err(error)
    }
}
