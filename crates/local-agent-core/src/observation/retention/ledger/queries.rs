use super::*;
use crate::observation::retention::{AttributionCoverage, PartDimension, key};
use serde_json::Value;
mod filtered;

impl FactLedger<'_> {
    /// Scope links are statistical pseudonyms. The service must independently
    /// verify current native/project authorization before allowing these filters.
    pub fn staging_scope_summary(
        &self,
        session: Option<&str>,
        project: Option<&str>,
    ) -> Result<Summary, LedgerError> {
        let tx = self.db.unchecked_transaction().map_err(unavailable)?;
        self.require_attribution()?;
        let mut filter = String::new();
        for (field, original) in [("session", session), ("project", project)] {
            if let Some(id) = original {
                filter.push_str(&format!(
                    " AND json_extract(value,'$.attribution.{field}')='{}'",
                    key(&self.namespace, field, id)
                ));
            }
        }
        let from = format!(
            "(SELECT rowid fact_order,0 sub_order,json_extract(value,'$.stats') stats,json_extract(value,'$.cacheIn') cache_in,json_extract(value,'$.cacheRead') cache_read,json_extract(value,'$.stats.ttft.p50') ttft_sample,json_extract(value,'$.stats.duration.p50') duration_sample,json_extract(value,'$.stats.throughput.p50') throughput_sample FROM retention_fact_staging WHERE state='staged'{filter})"
        );
        let result = aggregates::ordered_summary(&tx, &from).map_err(invalid)?;
        self.check_identity()?;
        tx.commit().map_err(unavailable)?;
        Ok(result)
    }
    pub(super) fn checked_coverage(&self) -> Result<AttributionCoverage, LedgerError> {
        self.check_identity()?;
        if self.immutable_snapshot {
            let (complete,summary):(i64,i64)=self.db.query_row("SELECT count(CASE WHEN json_type(value,'$.attribution')='object' THEN 1 END),count(CASE WHEN json_type(value,'$.attribution') IS NULL THEN 1 END) FROM retention_fact_staging WHERE state='staged'",[],|r|Ok((r.get(0)?,r.get(1)?))).map_err(invalid)?;
            return Ok(AttributionCoverage {
                complete_facts: usize::try_from(complete).map_err(invalid)?,
                summary_only_facts: usize::try_from(summary).map_err(invalid)?,
            });
        }
        self.validation_cache
            .try_borrow_mut()
            .map_err(|_| LedgerError::Unavailable)?
            .coverage(self.db, &self.namespace)
    }
    fn require_attribution(&self) -> Result<(), LedgerError> {
        if self.checked_coverage()?.summary_only_facts > 0 {
            return Err(LedgerError::IncompleteFacts);
        }
        Ok(())
    }
    pub fn attribution_coverage(&self) -> Result<AttributionCoverage, LedgerError> {
        let tx = self.db.unchecked_transaction().map_err(unavailable)?;
        let result = self.checked_coverage()?;
        self.check_identity()?;
        tx.commit().map_err(unavailable)?;
        Ok(result)
    }

    /// Contributions over the staged set, not a full page/report query. The key
    /// is the recorded Provider key or routed model name used by request groups.
    /// Use these parts for token/cost attribution. Their touched-request counts
    /// cannot be summed into a global call count; call/timing charts use root facts.
    pub fn staging_part_summary(
        &self,
        dimension: PartDimension,
        original_key: &str,
    ) -> Result<Summary, LedgerError> {
        let tx = self.db.unchecked_transaction().map_err(unavailable)?;
        self.require_attribution()?;
        let (dimension, group) = match dimension {
            PartDimension::Provider => ("provider", key(&self.namespace, "provider", original_key)),
            PartDimension::Model => ("model", key(&self.namespace, "model", original_key)),
        };
        // Only host-generated 64-byte hex digests and a fixed enum enter this SQL.
        let from = format!(
            "(SELECT f.rowid fact_order,p.key sub_order,json_extract(p.value,'$.stats') stats,json_extract(p.value,'$.cacheIn') cache_in,json_extract(p.value,'$.cacheRead') cache_read,json_extract(p.value,'$.stats.ttft.p50') ttft_sample,json_extract(p.value,'$.stats.duration.p50') duration_sample,json_extract(p.value,'$.stats.throughput.p50') throughput_sample FROM retention_fact_staging f,json_each(f.value,'$.attribution.parts') p WHERE f.state='staged' AND json_extract(p.value,'$.dimension')='{dimension}' AND json_extract(p.value,'$.key')='{group}')"
        );
        let summary = aggregates::ordered_summary(&tx, &from).map_err(invalid)?;
        self.check_identity()?;
        tx.commit().map_err(unavailable)?;
        Ok(summary)
    }

    /// Provider usage counts attempts and uses the separately calculated Provider
    /// cost channel. `_unknown` selects unattributed attempts, matching live usage.
    /// Date/source-authorization filtering remains the controller's responsibility.
    pub fn staging_provider_contribution(
        &self,
        provider: Option<&str>,
    ) -> Result<Value, LedgerError> {
        let tx = self.db.unchecked_transaction().map_err(unavailable)?;
        self.require_attribution()?;
        let filter = match provider {
            None => String::new(),
            Some("_unknown") => " AND json_extract(a.value,'$.attributedProvider') IS NULL".into(),
            Some(id) => format!(
                " AND json_extract(a.value,'$.attributedProvider')='{}'",
                key(&self.namespace, "provider", id)
            ),
        };
        let from = format!(
            "(SELECT f.rowid fact_order,a.key sub_order,json_extract(a.value,'$.providerStats') stats,json_extract(a.value,'$.providerStats.latencyP95Ms') latency_sample,json_extract(a.value,'$.providerStats.ttftP95Ms') ttft_sample FROM retention_fact_staging f,json_each(f.value,'$.attribution.attempts') a WHERE f.state='staged'{filter})"
        );
        let mut result = merge_stats(
            &tx,
            &from,
            serde_json::to_value(crate::provider_usage::Totals::default()).map_err(invalid)?,
        )?;
        for (column, rank, samples) in [
            ("latency_sample", "latencyP95Ms", "latencySamples"),
            ("ttft_sample", "ttftP95Ms", "ttftSamples"),
        ] {
            let (n, p95) = aggregates::percentile(&tx, &from, column, 95).map_err(invalid)?;
            result[rank] = serde_json::json!(p95);
            result[samples] = serde_json::json!(n);
        }
        self.check_identity()?;
        tx.commit().map_err(unavailable)?;
        Ok(result)
    }

    /// The original model-history connection identity is the recorded
    /// provider/endpoint/revision tuple digest, not the mutable Provider catalog ID.
    pub fn staging_model_contribution(
        &self,
        original_provider_scope: Option<&str>,
    ) -> Result<Value, LedgerError> {
        let tx = self.db.unchecked_transaction().map_err(unavailable)?;
        self.require_attribution()?;
        let filter = original_provider_scope
            .map(|id| {
                format!(
                    " AND json_extract(a.value,'$.modelProviderGroup')='{}'",
                    key(&self.namespace, "model-provider", id)
                )
            })
            .unwrap_or_default();
        let from = format!(
            "(SELECT f.rowid fact_order,a.key sub_order,json_extract(a.value,'$.modelStats') stats FROM retention_fact_staging f,json_each(f.value,'$.attribution.attempts') a WHERE f.state='staged'{filter})"
        );
        let result = merge_stats(
            &tx,
            &from,
            serde_json::to_value(crate::model_usage::UsageTotals::default()).map_err(invalid)?,
        )?;
        self.check_identity()?;
        tx.commit().map_err(unavailable)?;
        Ok(result)
    }
}

fn merge_stats(db: &Connection, from: &str, mut prototype: Value) -> Result<Value, LedgerError> {
    let raw: String = db
        .query_row(
            &format!("SELECT stats_sum(stats ORDER BY fact_order,sub_order) FROM {from}"),
            [],
            |r| r.get(0),
        )
        .map_err(invalid)?;
    let merged: Value = serde_json::from_str(&raw).map_err(invalid)?;
    for (key, value) in merged.as_object().ok_or(LedgerError::InvalidRecord)? {
        prototype[key] = value.clone();
    }
    Ok(prototype)
}

#[cfg(test)]
mod tests;
