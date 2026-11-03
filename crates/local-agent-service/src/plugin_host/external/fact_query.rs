//! Bounded in-memory query leases, isolated from persisted package and fact schemas.
use super::{Checked, Manager, Result, domain_store::FrozenFacts, fail};
use local_agent_core::domain::facts::query::{FactOrder, FactQuery};
use std::time::{Duration, Instant};
impl Manager {
    pub(crate) fn frozen_facts(&self, token: &str) -> Result<(FactQuery, String)> {
        let host = self.domain.lock().checked("unavailable")?;
        let frozen = host
            .facts
            .get(token)
            .filter(|q| q.created.elapsed() < Duration::from_secs(300))
            .ok_or(fail("stale_snapshot"))?;
        Ok((frozen.query.clone(), frozen.signature.clone()))
    }
    pub(crate) fn freeze_facts(
        &self,
        query: &FactQuery,
        signature: &str,
    ) -> Result<(String, String, Option<FactOrder>)> {
        let mut host = self.domain.lock().checked("unavailable")?;
        host.facts
            .retain(|_, q| q.created.elapsed() < Duration::from_secs(300));
        if let Some(token) = &query.snapshot_token {
            let frozen = host.facts.get(token).ok_or(fail("stale_snapshot"))?;
            if frozen.signature != signature || !frozen.query.same_selection(query) {
                return Err(fail("stale_snapshot"));
            }
            let after = match &query.cursor {
                None => None,
                Some(s) => {
                    let tail = s
                        .strip_prefix(&format!("{token}."))
                        .ok_or(fail("stale_snapshot"))?;
                    if !tail.len().is_multiple_of(2) {
                        return Err(fail("stale_snapshot"));
                    }
                    let bytes = (0..tail.len())
                        .step_by(2)
                        .map(|i| {
                            u8::from_str_radix(
                                tail.get(i..i + 2).ok_or(fail("stale_snapshot"))?,
                                16,
                            )
                            .checked("stale_snapshot")
                        })
                        .collect::<Result<Vec<_>>>()?;
                    Some(serde_json::from_slice(&bytes).checked("stale_snapshot")?)
                }
            };
            return Ok((token.clone(), frozen.as_of.clone(), after));
        }
        if query.cursor.is_some() {
            return Err(fail("stale_snapshot"));
        }
        if host.facts.len() >= 64
            && let Some(id) = host
                .facts
                .iter()
                .min_by_key(|(_, q)| q.created)
                .map(|(id, _)| id.clone())
        {
            host.facts.remove(&id);
        }
        let token = uuid::Uuid::new_v4().to_string();
        let as_of = chrono::Utc::now().to_rfc3339();
        host.facts.insert(
            token.clone(),
            FrozenFacts {
                query: query.clone(),
                signature: signature.into(),
                as_of: as_of.clone(),
                created: Instant::now(),
            },
        );
        Ok((token, as_of, None))
    }
    pub(crate) fn check_facts(&self, token: &str, signature: &str) -> Result<()> {
        let host = self.domain.lock().checked("unavailable")?;
        if !host.facts.get(token).is_some_and(|q| {
            q.signature == signature && q.created.elapsed() < Duration::from_secs(300)
        }) {
            return Err(fail("stale_snapshot"));
        }
        Ok(())
    }
}
