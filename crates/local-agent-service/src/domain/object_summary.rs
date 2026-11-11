//! Host metadata facade. Reading a detail slot never authorizes, refreshes or recovers a source.
use crate::AppState;
use local_agent_core::domain::{self as d, object_summary as o};

pub(crate) struct Source<'a>(pub &'a AppState);
fn unavailable<T>(_: T) -> d::Failure {
    d::Failure::new(d::ErrorCode::Unavailable)
}
fn identity(scope: &str, read: &mut o::ObjectSummaryRead) {
    read.read_identity = local_agent_core::projects::hash(
        &serde_json::json!([scope, read.read_identity]).to_string(),
    );
}
impl o::ObjectSummarySource for Source<'_> {
    fn read_object_summary(
        &self,
        selector: &o::ObjectSelector,
        fields: &[o::SummaryField],
    ) -> d::Result<o::ObjectSummaryRead> {
        selector.validate()?;
        o::validate_fields(selector.target(), fields)?;
        let state = self.0;
        let scope = state
            .store
            .lock()
            .map_err(unavailable)?
            .database_id()
            .map_err(unavailable)?;
        let mut result = match selector {
            o::ObjectSelector::Provider { id } => {
                let provider = state
                    .store
                    .lock()
                    .map_err(unavailable)?
                    .provider_by_id(id)
                    .map_err(unavailable)?
                    .ok_or_else(|| d::Failure::new(d::ErrorCode::NotFound))?;
                provider_summary(selector, fields, &provider)?
            }
            o::ObjectSelector::Account { id } if id.starts_with("upstream-") => {
                crate::billing::upstream_summary(state, id, fields)?
            }
            o::ObjectSelector::Account { id } => {
                // Account identity uses only the selected connection; corruption in
                // another provider cannot force a credential read or widen this scope.
                let providers = if let Some(provider_id) = id.strip_prefix("provider-") {
                    state
                        .store
                        .lock()
                        .map_err(unavailable)?
                        .provider_by_id(provider_id)
                        .map_err(unavailable)?
                        .into_iter()
                        .collect()
                } else {
                    vec![]
                };
                // status() also recovers the write journal. inspect_configs() only
                // reads configuration metadata and never opens the vault or helper.
                let configs = if id.starts_with("native-") {
                    state
                        .providers
                        .lock()
                        .map_err(unavailable)?
                        .inspect_configs()
                } else {
                    vec![]
                };
                state
                    .billing
                    .lock()
                    .map_err(unavailable)?
                    .object_summary(id, &providers, &configs, fields)?
            }
            o::ObjectSelector::Session {
                origin: o::ObjectOrigin::Unified { .. },
                ..
            } => match super::query::object_session_summary(state, selector, fields) {
                Ok(read) => read,
                Err(error) if error.code == "stale_snapshot" => {
                    super::facts::object_summary::read(state, selector, fields)
                        .map_err(external_failure)?
                }
                Err(error) => return Err(external_failure(error)),
            },
            o::ObjectSelector::Project {
                origin: o::ObjectOrigin::Unified { .. },
                ..
            } => super::facts::object_summary::read(state, selector, fields)
                .map_err(external_failure)?,
            o::ObjectSelector::Agent { .. }
            | o::ObjectSelector::Project {
                origin: o::ObjectOrigin::Builtin {},
                ..
            }
            | o::ObjectSelector::Session {
                origin: o::ObjectOrigin::Builtin {},
                ..
            } => {
                let hidden = state
                    .store
                    .lock()
                    .map_err(unavailable)?
                    .hidden_projects()
                    .map_err(unavailable)?
                    .into_iter()
                    .map(|p| p.id)
                    .collect::<Vec<_>>();
                state.sessions.object_summary(selector, fields, &hidden)?
            }
            o::ObjectSelector::Request {
                origin: o::ObjectOrigin::Unified { .. },
                ..
            } => super::requests::object_summary(state, selector, fields)
                .map_err(external_failure)?,
            o::ObjectSelector::Request {
                id,
                origin: o::ObjectOrigin::Builtin {},
                unit,
            } => {
                let native = *unit == d::requests::RequestUnit::ModelResponse;
                let key = native
                    .then(|| {
                        crate::observation::request_query::native_metadata_key(&state.sessions)
                    })
                    .transpose()
                    .map_err(unavailable)?;
                let record = state
                    .observation
                    .cached_sql_query(|history| {
                        Ok(
                            if key.as_ref().is_some_and(|key| !history.native_matches(key)) {
                                Err(d::Failure::new(d::ErrorCode::StaleSnapshot))
                            } else {
                                history.current_object_request(id, *unit)
                            },
                        )
                    })
                    .map_err(unavailable)??;
                let catalog = state.sessions.domain_catalog().map_err(unavailable)?;
                let source = catalog.sources.iter().find(|s| s.id == record.source_id);
                if native
                    && source.is_none_or(|s| {
                        s.status.authorization != d::Authorization::Authorized
                            || !s.status.runtime_active
                    })
                {
                    return Err(d::Failure::new(d::ErrorCode::GrantRevoked));
                }
                let source = d::SourceState {
                    source_id: record.source_id.clone(),
                    authorization: d::Authorization::Authorized,
                    freshness: d::Freshness::Current,
                    coverage: if state.observation.history_query_partial() {
                        d::Coverage::Partial
                    } else {
                        d::Coverage::Complete
                    },
                    problem: None,
                };
                let summary =
                    request_summary(&record, &source, fields).map_err(external_failure)?;
                if native
                    && key
                        != Some(
                            crate::observation::request_query::native_metadata_key(&state.sessions)
                                .map_err(unavailable)?,
                        )
                {
                    return Err(d::Failure::new(d::ErrorCode::StaleSnapshot));
                }
                o::ObjectSummaryRead {
                    selector: selector.clone(),
                    read_identity: local_agent_core::projects::hash(
                        &serde_json::json!([selector, fields, record, source, key,]).to_string(),
                    ),
                    summary,
                }
            }
        };
        identity(&scope, &mut result);
        result.validate(selector, fields)?;
        Ok(result)
    }
    fn ensure_object_summary_current(&self, read: &o::ObjectSummaryRead) -> d::Result<()> {
        let fields: Vec<_> = read.summary.metrics.iter().map(|m| m.field).collect();
        read.validate(&read.selector, &fields)?;
        let current = self.read_object_summary(&read.selector, &fields)?;
        if current.read_identity != read.read_identity {
            return Err(d::Failure::new(d::ErrorCode::StaleSnapshot));
        }
        Ok(())
    }
}

fn external_failure(error: crate::plugin_host::external::Error) -> d::Failure {
    d::Failure::new(match error.code {
        "permission_denied" | "grant_revoked" | "plugin_disabled" => d::ErrorCode::GrantRevoked,
        "stale_snapshot" => d::ErrorCode::StaleSnapshot,
        "not_found" => d::ErrorCode::NotFound,
        "invalid_input" => d::ErrorCode::InvalidInput,
        "unsupported" => d::ErrorCode::Unsupported,
        "resource_limit" => d::ErrorCode::ResourceLimit,
        _ => d::ErrorCode::Unavailable,
    })
}

pub(super) fn session_summary(
    record: &d::SessionRecord,
    source: &d::SourceState,
    fields: &[o::SummaryField],
) -> crate::plugin_host::external::Result<o::ObjectSummary> {
    use o::{SummaryField as F, SummaryValue as V};
    let usage = &record.usage;
    let values = [
        (F::InputTokens, &usage.input_tokens),
        (F::OutputTokens, &usage.output_tokens),
        (F::TotalTokens, &usage.total_tokens),
    ];
    let summary = o::ObjectSummary {
        api_version: o::API_VERSION.into(),
        target: o::ObjectTarget::Session,
        coverage: if record.partial {
            d::Coverage::Partial
        } else {
            source.coverage.clone()
        },
        freshness: source.freshness.clone(),
        captured_at: chrono::Utc::now().to_rfc3339(),
        metrics: fields
            .iter()
            .map(|field| {
                let value = values
                    .iter()
                    .find(|(f, _)| f == field)
                    .and_then(|(_, value)| (*value).clone())
                    .map(|value| V::Tokens { value });
                o::SummaryMetric {
                    field: *field,
                    missing_reason: value.is_none().then_some(o::MissingReason::NotRecorded),
                    value,
                }
            })
            .collect(),
    };
    summary
        .validate()
        .map_err(|_| crate::plugin_host::external::fail("invalid_record"))?;
    Ok(summary)
}

pub(super) fn request_summary(
    record: &d::requests::query::RequestQueryRecord,
    source: &d::SourceState,
    fields: &[o::SummaryField],
) -> crate::plugin_host::external::Result<o::ObjectSummary> {
    use o::{SummaryField as F, SummaryOutcome as O, SummaryValue as V};
    let logical = record.unit == d::requests::RequestUnit::LogicalRequest;
    if !logical
        && (source.authorization != d::Authorization::Authorized
            || source.coverage == d::Coverage::Unavailable)
    {
        return Err(crate::plugin_host::external::fail("grant_revoked"));
    }
    let summary = o::ObjectSummary {
        api_version: o::API_VERSION.into(),
        target: o::ObjectTarget::Request,
        coverage: if source.coverage == d::Coverage::Complete && record.usage_missing.value() == 0 {
            d::Coverage::Complete
        } else {
            d::Coverage::Partial
        },
        freshness: if logical && source.freshness == d::Freshness::Unread {
            d::Freshness::Stale
        } else {
            source.freshness.clone()
        },
        captured_at: chrono::Utc::now().to_rfc3339(),
        metrics: fields
            .iter()
            .map(|field| {
                let value = match field {
                    F::InputTokens => record
                        .usage
                        .input_tokens
                        .clone()
                        .map(|value| V::Tokens { value }),
                    F::OutputTokens => record
                        .usage
                        .output_tokens
                        .clone()
                        .map(|value| V::Tokens { value }),
                    F::TotalTokens => record
                        .usage
                        .total_tokens
                        .clone()
                        .map(|value| V::Tokens { value }),
                    F::AttemptCount => record.attempts.clone().map(|value| V::Count { value }),
                    F::DurationMs => record.duration_ms.map(|value| V::Milliseconds { value }),
                    F::RequestOutcome => Some(V::Outcome {
                        value: match record.outcome.as_str() {
                            "success" | "completed" | "succeeded" => O::Succeeded,
                            "failed" | "error" => O::Failed,
                            "cancelled" => O::Cancelled,
                            "in_progress" | "running" | "pending" | "streaming" => O::InProgress,
                            _ => O::Unknown,
                        },
                    }),
                    _ => None,
                };
                o::SummaryMetric {
                    field: *field,
                    missing_reason: value.is_none().then_some(o::MissingReason::NotRecorded),
                    value,
                }
            })
            .collect(),
    };
    summary
        .validate()
        .map_err(|_| crate::plugin_host::external::fail("invalid_record"))?;
    Ok(summary)
}

/// Count distinct, enabled IDs in the saved configuration, not live model inventory.
fn provider_summary(
    selector: &o::ObjectSelector,
    fields: &[o::SummaryField],
    provider: &local_agent_core::Provider,
) -> d::Result<o::ObjectSummaryRead> {
    if !matches!(selector, o::ObjectSelector::Provider { id } if id == &provider.id) {
        return Err(d::Failure::new(d::ErrorCode::StaleSnapshot));
    }
    let summary = o::ObjectSummary {
        api_version: o::API_VERSION.into(),
        target: o::ObjectTarget::Provider,
        coverage: d::Coverage::Complete,
        freshness: d::Freshness::Current,
        captured_at: chrono::Utc::now().to_rfc3339(),
        metrics: fields
            .iter()
            .map(|field| {
                Ok(o::SummaryMetric {
                    field: *field,
                    missing_reason: None,
                    value: Some(match field {
                        o::SummaryField::ModelCount => o::SummaryValue::Count {
                            value: (provider.enabled_model_ids().len() as u64).into(),
                        },
                        o::SummaryField::Configuration => o::SummaryValue::Configuration {
                            configured: provider.configured,
                        },
                        _ => return Err(d::Failure::new(d::ErrorCode::InvalidInput)),
                    }),
                })
            })
            .collect::<d::Result<_>>()?,
    };
    let read = o::ObjectSummaryRead {
        selector: selector.clone(),
        read_identity: local_agent_core::projects::hash(
            &serde_json::json!([selector, fields, provider]).to_string(),
        ),
        summary,
    };
    read.validate(selector, fields)?;
    Ok(read)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn object_provider_summary_counts_saved_enabled_ids_and_hides_endpoint_and_credentials() {
        let provider: local_agent_core::Provider = serde_json::from_value(serde_json::json!({
            "id":"p","name":"Private label","endpoint":"https://private.invalid/v1",
            "protocol":"messages","model":"a","configured":true,"models":["a","b","c"],
            "disabledModels":["b"],"credentialRef":"private-reference","credentialLabel":"private-label"
        })).unwrap();
        let selector = o::ObjectSelector::Provider { id: "p".into() };
        let fields = [o::SummaryField::ModelCount, o::SummaryField::Configuration];
        let read = provider_summary(&selector, &fields, &provider).unwrap();
        assert_eq!(
            read.summary.metrics[0].value,
            Some(o::SummaryValue::Count {
                value: 2_u64.into()
            })
        );
        let encoded = serde_json::to_string(&read.summary).unwrap();
        assert!(!encoded.contains("private"));
        assert!(!encoded.contains("Private"));
        let mut changed = provider.clone();
        changed.revision += 1;
        assert_ne!(
            read.read_identity,
            provider_summary(&selector, &fields, &changed)
                .unwrap()
                .read_identity
        );
        assert_eq!(
            read.read_identity,
            provider_summary(&selector, &fields, &provider)
                .unwrap()
                .read_identity
        );
    }
}
