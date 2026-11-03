//! Whole draft-set filtering. Original/native authorization is not inferred here.
use super::*;

fn check_key(value: Option<&str>) -> Result<(), LedgerError> {
    if value.is_some_and(|v| v.len() > 1024) {
        return Err(LedgerError::InvalidQuery);
    }
    Ok(())
}

impl FactLedger<'_> {
    fn filter_coverage(&self, filter: &GatewayFilter, attempts: bool) -> Result<(), LedgerError> {
        let coverage = self.checked_coverage()?;
        if (attempts || filter.needs_attribution()) && coverage.summary_only_facts > 0 {
            return Err(LedgerError::IncompleteFacts);
        }
        if filter.agent.is_some() {
            let missing: bool = self.db.query_row("SELECT EXISTS(SELECT 1 FROM retention_fact_staging WHERE state='staged' AND json_extract(value,'$.version')!=3)", [], |r| r.get(0)).map_err(invalid)?;
            if missing {
                return Err(LedgerError::IncompleteFacts);
            }
        }
        Ok(())
    }

    /// Logical request date/Agent; scope restrictions intersect. No detail access.
    pub fn staging_summary_filtered(&self, filter: &GatewayFilter) -> Result<Summary, LedgerError> {
        filter.validate()?;
        let tx = self.db.unchecked_transaction().map_err(unavailable)?;
        self.filter_coverage(filter, false)?;
        let where_sql = filter.sql(&self.namespace, "f.value");
        let from = format!(
            "(SELECT f.rowid fact_order,0 sub_order,json_extract(f.value,'$.stats') stats,json_extract(f.value,'$.cacheIn') cache_in,json_extract(f.value,'$.cacheRead') cache_read,json_extract(f.value,'$.stats.ttft.p50') ttft_sample,json_extract(f.value,'$.stats.duration.p50') duration_sample,json_extract(f.value,'$.stats.throughput.p50') throughput_sample FROM retention_fact_staging f WHERE f.state='staged'{where_sql})"
        );
        let result = aggregates::ordered_summary(&tx, &from).map_err(invalid)?;
        self.check_identity()?;
        tx.commit().map_err(unavailable)?;
        Ok(result)
    }

    /// Root request selection, then the requested Provider/routed-model fragment.
    pub fn staging_part_summary_filtered(
        &self,
        dimension: PartDimension,
        original_key: &str,
        filter: &GatewayFilter,
    ) -> Result<Summary, LedgerError> {
        filter.validate()?;
        check_key(Some(original_key))?;
        let tx = self.db.unchecked_transaction().map_err(unavailable)?;
        self.filter_coverage(filter, true)?;
        let (dimension, group) = match dimension {
            PartDimension::Provider => ("provider", key(&self.namespace, "provider", original_key)),
            PartDimension::Model => ("model", key(&self.namespace, "model", original_key)),
        };
        let where_sql = filter.sql(&self.namespace, "f.value");
        let from = format!(
            "(SELECT f.rowid fact_order,p.key sub_order,json_extract(p.value,'$.stats') stats,json_extract(p.value,'$.cacheIn') cache_in,json_extract(p.value,'$.cacheRead') cache_read,json_extract(p.value,'$.stats.ttft.p50') ttft_sample,json_extract(p.value,'$.stats.duration.p50') duration_sample,json_extract(p.value,'$.stats.throughput.p50') throughput_sample FROM retention_fact_staging f,json_each(f.value,'$.attribution.parts') p WHERE f.state='staged' AND json_extract(p.value,'$.dimension')='{dimension}' AND json_extract(p.value,'$.key')='{group}'{where_sql})"
        );
        let result = aggregates::ordered_summary(&tx, &from).map_err(invalid)?;
        self.check_identity()?;
        tx.commit().map_err(unavailable)?;
        Ok(result)
    }

    /// Each attempt's date/Agent, then its recorded attributed Provider identity.
    /// `_unknown` retains the existing Provider contribution selector semantics.
    pub fn staging_provider_contribution_filtered(
        &self,
        provider: Option<&str>,
        filter: &GatewayFilter,
    ) -> Result<Value, LedgerError> {
        filter.validate()?;
        check_key(provider)?;
        let tx = self.db.unchecked_transaction().map_err(unavailable)?;
        self.filter_coverage(filter, true)?;
        let mut where_sql = filter.sql(&self.namespace, "a.value");
        match provider {
            None => {}
            Some("_unknown") => {
                where_sql.push_str(" AND json_extract(a.value,'$.attributedProvider') IS NULL")
            }
            Some(id) => where_sql.push_str(&format!(
                " AND json_extract(a.value,'$.attributedProvider')='{}'",
                key(&self.namespace, "provider", id)
            )),
        }
        let from = format!(
            "(SELECT f.rowid fact_order,a.key sub_order,json_extract(a.value,'$.providerStats') stats,json_extract(a.value,'$.providerStats.latencyP95Ms') latency_sample,json_extract(a.value,'$.providerStats.ttftP95Ms') ttft_sample FROM retention_fact_staging f,json_each(f.value,'$.attribution.attempts') a WHERE f.state='staged'{where_sql})"
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

    /// Each attempt's date/Agent; provider scope is the original model-history tuple.
    pub fn staging_model_contribution_filtered(
        &self,
        provider_scope: Option<&str>,
        filter: &GatewayFilter,
    ) -> Result<Value, LedgerError> {
        filter.validate()?;
        check_key(provider_scope)?;
        let tx = self.db.unchecked_transaction().map_err(unavailable)?;
        self.filter_coverage(filter, true)?;
        let mut where_sql = filter.sql(&self.namespace, "a.value");
        if let Some(id) = provider_scope {
            where_sql.push_str(&format!(
                " AND json_extract(a.value,'$.modelProviderGroup')='{}'",
                key(&self.namespace, "model-provider", id)
            ));
        }
        let from = format!(
            "(SELECT f.rowid fact_order,a.key sub_order,json_extract(a.value,'$.modelStats') stats FROM retention_fact_staging f,json_each(f.value,'$.attribution.attempts') a WHERE f.state='staged'{where_sql})"
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

#[cfg(test)]
mod tests;
