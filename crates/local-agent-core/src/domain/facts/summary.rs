//! Totals retain fact units and missing evidence; no session/response double count.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum UsageUnit {
    SessionCumulative,
    ModelResponse,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnitUsage {
    pub unit: UsageUnit,
    pub coverage: Coverage,
    pub records: Option<Decimal>,
    pub usage: Usage,
    /// At least one matching fact lacks this field. Totals include known values only.
    pub missing_fields: Vec<String>,
}
impl UnitUsage {
    /// Immutable per-generation response statistics. Stream the input so even a
    /// dense native file never needs another collection of converted usages.
    pub(crate) fn model_responses(rows: impl IntoIterator<Item = Usage>) -> Result<Self> {
        let mut unit = accumulate(UsageUnit::ModelResponse, &[], Coverage::Complete)?;
        for usage in rows {
            observe_usage(&mut unit, &usage)?;
        }
        Ok(unit)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FactSummary {
    pub source_id: String,
    pub coverage: Coverage,
    pub projects: Option<Decimal>,
    pub sessions: UnitUsage,
    pub responses: UnitUsage,
    pub activities: Option<Decimal>,
    pub write_reports: Option<Decimal>,
    pub successful_write_reports: Option<Decimal>,
    pub mcp_catalogs: Option<Decimal>,
    pub mcp_executions: Option<Decimal>,
    /// Source-reported IDs do not establish overlap with other sources or the gateway.
    pub cross_source_overlap_unknown: bool,
}

/// Streaming totals for native indexes. Memory use does not grow with call history.
pub struct FactTally(FactSummary);
impl FactTally {
    pub fn new(source: &str, kinds: &[FactKind], coverage: Coverage) -> Result<Self> {
        let unit_coverage = |kind| {
            if kinds.contains(&kind) {
                coverage.clone()
            } else {
                Coverage::Unavailable
            }
        };
        let count = |kind| {
            (kinds.contains(&kind) && coverage != Coverage::Unavailable).then(|| 0u64.into())
        };
        Ok(Self(FactSummary {
            source_id: source.into(),
            coverage: coverage.clone(),
            projects: count(FactKind::Project),
            sessions: accumulate(
                UsageUnit::SessionCumulative,
                &[],
                unit_coverage(FactKind::Session),
            )?,
            responses: accumulate(
                UsageUnit::ModelResponse,
                &[],
                unit_coverage(FactKind::Response),
            )?,
            activities: count(FactKind::Activity),
            write_reports: count(FactKind::Write),
            successful_write_reports: count(FactKind::Write),
            mcp_catalogs: count(FactKind::McpCatalog),
            mcp_executions: count(FactKind::McpExecution),
            cross_source_overlap_unknown: true,
        }))
    }
    pub(crate) fn merge_responses(&mut self, unit: &UnitUsage) -> Result<()> {
        if unit.unit != UsageUnit::ModelResponse || unit.coverage != Coverage::Complete {
            return Err(Failure::new(ErrorCode::InvalidInput));
        }
        let count = unit
            .records
            .as_ref()
            .ok_or_else(|| Failure::new(ErrorCode::InvalidInput))?
            .value();
        if count == 0 {
            return Ok(());
        }
        let target = &mut self.0.responses;
        let first = target.records.as_ref().is_some_and(|v| v.value() == 0);
        add_count(&mut target.records, count)?;
        for (sum, value) in [
            (&mut target.usage.input_tokens, &unit.usage.input_tokens),
            (&mut target.usage.output_tokens, &unit.usage.output_tokens),
            (
                &mut target.usage.cached_input_tokens,
                &unit.usage.cached_input_tokens,
            ),
            (
                &mut target.usage.cache_write_input_tokens,
                &unit.usage.cache_write_input_tokens,
            ),
            (
                &mut target.usage.reasoning_output_tokens,
                &unit.usage.reasoning_output_tokens,
            ),
            (&mut target.usage.total_tokens, &unit.usage.total_tokens),
        ] {
            if let Some(value) = value {
                *sum = Some(
                    sum.as_ref()
                        .map_or(0, Decimal::value)
                        .checked_add(value.value())
                        .ok_or_else(|| Failure::new(ErrorCode::ResourceLimit))?
                        .into(),
                );
            } else if first {
                *sum = None;
            }
        }
        for name in &unit.missing_fields {
            if !target.missing_fields.contains(name) {
                target.missing_fields.push(name.clone());
            }
        }
        Ok(())
    }
    pub(crate) fn observe_mcp_executions(&mut self, count: usize) -> Result<()> {
        add_count(
            &mut self.0.mcp_executions,
            u64::try_from(count).map_err(|_| Failure::new(ErrorCode::ResourceLimit))?,
        )
    }
    pub fn observe(&mut self, value: &FactCandidate) -> Result<()> {
        let s = &mut self.0;
        match value {
            FactCandidate::Project(_) => increment(&mut s.projects),
            FactCandidate::Session(v) => observe_usage(&mut s.sessions, &v.usage),
            FactCandidate::Response(v) => observe_usage(&mut s.responses, &v.usage),
            FactCandidate::Activity(_) => increment(&mut s.activities),
            FactCandidate::Write(v) => {
                increment(&mut s.write_reports)?;
                if v.outcome == Outcome::Success {
                    increment(&mut s.successful_write_reports)?;
                }
                Ok(())
            }
            FactCandidate::McpCatalog(_) => increment(&mut s.mcp_catalogs),
            FactCandidate::McpExecution(_) => increment(&mut s.mcp_executions),
        }
    }
    pub fn finish(mut self) -> FactSummary {
        const FIELDS: [&str; 6] = [
            "inputTokens",
            "outputTokens",
            "cachedInputTokens",
            "cacheWriteInputTokens",
            "reasoningOutputTokens",
            "totalTokens",
        ];
        for unit in [&mut self.0.sessions, &mut self.0.responses] {
            unit.missing_fields
                .sort_by_key(|name| FIELDS.iter().position(|field| name == field));
        }
        self.0
    }
}
fn increment(value: &mut Option<Decimal>) -> Result<()> {
    add_count(value, 1)
}
fn add_count(value: &mut Option<Decimal>, count: u64) -> Result<()> {
    let before = value
        .as_ref()
        .ok_or_else(|| Failure::new(ErrorCode::InvalidInput))?
        .value();
    *value = Some(
        before
            .checked_add(count)
            .ok_or_else(|| Failure::new(ErrorCode::ResourceLimit))?
            .into(),
    );
    Ok(())
}
fn observe_usage(unit: &mut UnitUsage, usage: &Usage) -> Result<()> {
    let first = unit.records.as_ref().is_some_and(|v| v.value() == 0);
    increment(&mut unit.records)?;
    for (sum, value, name) in [
        (
            &mut unit.usage.input_tokens,
            &usage.input_tokens,
            "inputTokens",
        ),
        (
            &mut unit.usage.output_tokens,
            &usage.output_tokens,
            "outputTokens",
        ),
        (
            &mut unit.usage.cached_input_tokens,
            &usage.cached_input_tokens,
            "cachedInputTokens",
        ),
        (
            &mut unit.usage.cache_write_input_tokens,
            &usage.cache_write_input_tokens,
            "cacheWriteInputTokens",
        ),
        (
            &mut unit.usage.reasoning_output_tokens,
            &usage.reasoning_output_tokens,
            "reasoningOutputTokens",
        ),
        (
            &mut unit.usage.total_tokens,
            &usage.total_tokens,
            "totalTokens",
        ),
    ] {
        if let Some(value) = value {
            *sum = Some(
                sum.as_ref()
                    .map_or(0, Decimal::value)
                    .checked_add(value.value())
                    .ok_or_else(|| Failure::new(ErrorCode::ResourceLimit))?
                    .into(),
            );
        } else {
            if first {
                *sum = None;
            }
            if !unit.missing_fields.iter().any(|s| s == name) {
                unit.missing_fields.push(name.into());
            }
        }
    }
    Ok(())
}
fn accumulate(unit: UsageUnit, rows: &[&Usage], coverage: Coverage) -> Result<UnitUsage> {
    if coverage == Coverage::Unavailable && !rows.is_empty() {
        return Err(Failure::new(ErrorCode::InvalidInput));
    }
    let mut missing = Vec::new();
    let mut sum = |name: &str, field: fn(&Usage) -> &Option<Decimal>| -> Result<Option<Decimal>> {
        let mut total = None::<u64>;
        for row in rows {
            if let Some(value) = field(row) {
                total = Some(
                    total
                        .unwrap_or(0)
                        .checked_add(value.value())
                        .ok_or_else(|| Failure::new(ErrorCode::ResourceLimit))?,
                );
            } else if !missing.iter().any(|s| s == name) {
                missing.push(name.to_owned());
            }
        }
        // A complete empty fact set establishes zero; unknown usage does not.
        Ok(if rows.is_empty() && coverage == Coverage::Complete {
            Some(0u64.into())
        } else {
            total.map(Into::into)
        })
    };
    let usage = Usage {
        input_tokens: sum("inputTokens", |v| &v.input_tokens)?,
        output_tokens: sum("outputTokens", |v| &v.output_tokens)?,
        cached_input_tokens: sum("cachedInputTokens", |v| &v.cached_input_tokens)?,
        cache_write_input_tokens: sum("cacheWriteInputTokens", |v| &v.cache_write_input_tokens)?,
        reasoning_output_tokens: sum("reasoningOutputTokens", |v| &v.reasoning_output_tokens)?,
        total_tokens: sum("totalTokens", |v| &v.total_tokens)?,
    };
    Ok(UnitUsage {
        unit,
        records: (coverage != Coverage::Unavailable).then(|| (rows.len() as u64).into()),
        coverage,
        usage,
        missing_fields: missing,
    })
}
impl FactSnapshot {
    /// Callers pass the complete filtered set from one fixed generation, never one UI page.
    pub fn summary<'a>(
        &self,
        records: impl IntoIterator<Item = &'a FactRecord>,
    ) -> Result<FactSummary> {
        let mut projects = 0u64;
        let mut sessions = Vec::new();
        let mut responses = Vec::new();
        let mut activities = 0u64;
        let mut writes = 0u64;
        let mut successful = 0u64;
        let mut catalogs = 0u64;
        let mut executions = 0u64;
        for fact in records {
            if fact.identity != self.identity
                || fact.source_revision != self.source_revision
                || !self.available_kinds.contains(&fact.value.fact_kind())
            {
                return Err(Failure::new(ErrorCode::StaleSnapshot));
            }
            match &fact.value {
                FactCandidate::Project(_) => projects += 1,
                FactCandidate::Session(v) => sessions.push(&v.usage),
                FactCandidate::Response(v) => responses.push(&v.usage),
                FactCandidate::Activity(_) => activities += 1,
                FactCandidate::Write(v) => {
                    writes += 1;
                    successful += u64::from(v.outcome == Outcome::Success);
                }
                FactCandidate::McpCatalog(_) => catalogs += 1,
                FactCandidate::McpExecution(_) => executions += 1,
            }
        }
        let coverage = |kind| {
            if self.available_kinds.contains(&kind) {
                self.coverage.clone()
            } else {
                Coverage::Unavailable
            }
        };
        let count =
            |kind, value: u64| (coverage(kind) != Coverage::Unavailable).then(|| value.into());
        Ok(FactSummary {
            source_id: self.identity.source_id.clone(),
            coverage: self.coverage.clone(),
            projects: count(FactKind::Project, projects),
            sessions: accumulate(
                UsageUnit::SessionCumulative,
                &sessions,
                coverage(FactKind::Session),
            )?,
            responses: accumulate(
                UsageUnit::ModelResponse,
                &responses,
                coverage(FactKind::Response),
            )?,
            activities: count(FactKind::Activity, activities),
            write_reports: count(FactKind::Write, writes),
            successful_write_reports: count(FactKind::Write, successful),
            mcp_catalogs: count(FactKind::McpCatalog, catalogs),
            mcp_executions: count(FactKind::McpExecution, executions),
            cross_source_overlap_unknown: true,
        })
    }
}

#[cfg(test)]
mod response_cache_tests {
    use super::*;
    fn usage(input: Option<u64>, output: Option<u64>) -> Usage {
        Usage {
            input_tokens: input.map(Into::into),
            output_tokens: output.map(Into::into),
            cached_input_tokens: None,
            cache_write_input_tokens: None,
            reasoning_output_tokens: None,
            total_tokens: input
                .zip(output)
                .and_then(|(a, b)| a.checked_add(b))
                .map(Into::into),
        }
    }
    fn response(usage: Usage) -> FactCandidate {
        FactCandidate::Response(ResponseCandidate {
            native_id: "r".into(),
            session_ref: "s".into(),
            product_id: None,
            occurred_at: None,
            model: None,
            outcome: Outcome::Unknown,
            usage,
            reported_response_id: None,
            duration_ms: None,
        })
    }
    #[test]
    fn cached_response_batches_equal_streaming_unknown_zero_and_empty_units() {
        for coverage in [Coverage::Complete, Coverage::Partial] {
            for rows in [
                vec![],
                vec![usage(None, None)],
                vec![
                    usage(None, None),
                    usage(Some(0), None),
                    usage(Some(4), Some(3)),
                    usage(Some(0), Some(0)),
                ],
            ] {
                let mut direct =
                    FactTally::new("native", &[FactKind::Response], coverage.clone()).unwrap();
                for row in &rows {
                    direct.observe(&response(row.clone())).unwrap();
                }
                let expected = direct.finish();
                for size in 1..=5 {
                    let mut cached =
                        FactTally::new("native", &[FactKind::Response], coverage.clone()).unwrap();
                    cached
                        .merge_responses(&UnitUsage::model_responses([]).unwrap())
                        .unwrap();
                    for chunk in rows.chunks(size) {
                        cached
                            .merge_responses(
                                &UnitUsage::model_responses(chunk.iter().cloned()).unwrap(),
                            )
                            .unwrap();
                    }
                    assert_eq!(cached.finish(), expected);
                }
            }
        }
    }
    #[test]
    fn response_cache_and_cross_file_merge_reject_unsigned_overflow() {
        let max = usage(Some(u64::MAX), None);
        let one = usage(Some(1), None);
        assert_eq!(
            UnitUsage::model_responses([max.clone(), one.clone()])
                .unwrap_err()
                .code,
            ErrorCode::ResourceLimit
        );
        let mut tally =
            FactTally::new("native", &[FactKind::Response], Coverage::Complete).unwrap();
        tally
            .merge_responses(&UnitUsage::model_responses([max]).unwrap())
            .unwrap();
        assert_eq!(
            tally
                .merge_responses(&UnitUsage::model_responses([one]).unwrap())
                .unwrap_err()
                .code,
            ErrorCode::ResourceLimit
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn usage(input: Option<u64>) -> Usage {
        Usage {
            input_tokens: input.map(Into::into),
            output_tokens: None,
            cached_input_tokens: None,
            cache_write_input_tokens: None,
            reasoning_output_tokens: None,
            total_tokens: None,
        }
    }
    #[test]
    fn sums_known_evidence_without_rounding_unknowns_or_mixing_fact_units() {
        let known = usage(Some(10));
        let zero = usage(Some(0));
        let unknown = usage(None);
        let sum = accumulate(
            UsageUnit::ModelResponse,
            &[&known, &zero, &unknown],
            Coverage::Complete,
        )
        .unwrap();
        assert_eq!(sum.records.unwrap().value(), 3);
        assert_eq!(sum.usage.input_tokens.unwrap().value(), 10);
        assert!(sum.usage.output_tokens.is_none());
        assert!(sum.missing_fields.contains(&"inputTokens".into()));
        assert!(
            accumulate(
                UsageUnit::SessionCumulative,
                &[&unknown],
                Coverage::Complete
            )
            .unwrap()
            .usage
            .input_tokens
            .is_none()
        );
        let empty = accumulate(UsageUnit::SessionCumulative, &[], Coverage::Complete).unwrap();
        assert_eq!(empty.records.unwrap().value(), 0);
        assert_eq!(empty.usage.input_tokens.unwrap().value(), 0);
        assert!(empty.missing_fields.is_empty());
        let max = usage(Some(u64::MAX));
        assert_eq!(
            accumulate(
                UsageUnit::ModelResponse,
                &[&max, &known],
                Coverage::Complete
            )
            .unwrap_err()
            .code,
            ErrorCode::ResourceLimit
        );
        let missing = accumulate(UsageUnit::ModelResponse, &[], Coverage::Unavailable).unwrap();
        assert!(missing.records.is_none());
        assert!(missing.usage.total_tokens.is_none());
    }
}
