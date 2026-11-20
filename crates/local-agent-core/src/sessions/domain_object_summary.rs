//! Scalar projections of the existing index. No transcript body or scan is opened here.
use super::{NativeSessions, SourceStatus, unique_files};
use crate::domain::{self as d, object_summary as o};

fn failure(code: d::ErrorCode) -> d::Failure {
    d::Failure::new(code)
}
fn source_state(source: &SourceStatus) -> (d::Coverage, d::Freshness) {
    if source.status == "ready" {
        (d::Coverage::Complete, d::Freshness::Current)
    } else if source.last_successful_scan.is_some() {
        (d::Coverage::Partial, d::Freshness::Stale)
    } else if source.error.is_some() {
        (d::Coverage::Unavailable, d::Freshness::Failed)
    } else {
        (d::Coverage::Unavailable, d::Freshness::Unread)
    }
}
fn metric(
    field: o::SummaryField,
    value: Option<o::SummaryValue>,
    reason: o::MissingReason,
) -> o::SummaryMetric {
    o::SummaryMetric {
        field,
        missing_reason: value.is_none().then_some(reason),
        value,
    }
}
#[cfg(test)]
#[test]
fn native_summary_lease_expires_even_if_source_is_reenabled() {
    let home = tempfile::tempdir().unwrap();
    let sessions = NativeSessions::new(home.path().into(), &["claude".into()]);
    let lease = sessions
        .domain_activity_authorizations()
        .unwrap()
        .into_iter()
        .find(|lease| lease.source_id() == "claude")
        .unwrap();
    lease.ensure_current(&sessions).unwrap();
    sessions.set_enabled("claude", false).unwrap();
    assert!(lease.ensure_current(&sessions).is_err());
    sessions.set_enabled("claude", true).unwrap();
    assert!(lease.ensure_current(&sessions).is_err());
}

impl NativeSessions {
    pub fn platform_project_titles(
        &self,
        ids: &std::collections::BTreeSet<&str>,
        hidden: &std::collections::BTreeSet<String>,
    ) -> d::Result<std::collections::BTreeMap<String, (String, String)>> {
        let state = self
            .lock()
            .map_err(|_| failure(d::ErrorCode::Unavailable))?;
        let ready = state
            .sources
            .iter()
            .filter(|source| {
                source.enabled && source.status == "ready" && self.source_active(&source.agent_id)
            })
            .map(|source| source.agent_id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        Ok(unique_files(&state)
            .into_iter()
            .filter_map(|file| {
                let id = file.summary.project_id.as_ref()?;
                let name = file.summary.project_name.as_ref()?;
                (ids.contains(id.as_str())
                    && !hidden.contains(id)
                    && ready.contains(file.agent.as_str()))
                .then(|| {
                    (
                        id.clone(),
                        (file.agent.clone(), super::sanitize_label(name, 120)),
                    )
                })
            })
            .collect())
    }

    pub fn platform_session_summaries(
        &self,
        ids: &std::collections::BTreeSet<&str>,
    ) -> d::Result<std::collections::BTreeMap<String, crate::plugin::platform::HydratedEntity>>
    {
        use crate::plugin::platform::{Authorization, Freshness, HydratedEntity};

        let state = self
            .lock()
            .map_err(|_| failure(d::ErrorCode::Unavailable))?;
        let ready = state
            .sources
            .iter()
            .filter(|source| {
                source.enabled && source.status == "ready" && self.source_active(&source.agent_id)
            })
            .map(|source| source.agent_id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        Ok(unique_files(&state)
            .into_iter()
            .filter(|file| {
                ids.contains(file.summary.id.as_str()) && ready.contains(file.agent.as_str())
            })
            .map(|file| {
                let summary = &file.summary;
                (
                    summary.id.clone(),
                    HydratedEntity {
                        authorization: Authorization::Authorized,
                        freshness: Freshness::Current,
                        title: Some(super::sanitize_label(&summary.title, 200)),
                        source_id: Some(file.agent.clone()),
                        activity_state: None,
                        model: None,
                        total_tokens: summary.usage.total_tokens,
                        route_hash: None,
                    },
                )
            })
            .collect())
    }

    /// Hidden projects remain a display boundary; session metadata keeps its existing visibility.
    pub fn object_summary(
        &self,
        selector: &o::ObjectSelector,
        fields: &[o::SummaryField],
        hidden_projects: &[String],
    ) -> d::Result<o::ObjectSummaryRead> {
        use o::{ObjectSelector as S, SummaryField as F, SummaryValue as V};
        selector.validate()?;
        o::validate_fields(selector.target(), fields)?;
        if !matches!(
            selector,
            S::Agent { .. }
                | S::Project {
                    origin: o::ObjectOrigin::Builtin {},
                    ..
                }
                | S::Session {
                    origin: o::ObjectOrigin::Builtin {},
                    ..
                }
        ) {
            return Err(failure(d::ErrorCode::Unsupported));
        }
        let state = self
            .lock()
            .map_err(|_| failure(d::ErrorCode::Unavailable))?;
        // Include plugin epochs, not just active booleans: a restarted source cannot
        // revive an old object reference even when the index itself did not change.
        let authorities: Vec<_> = state
            .sources
            .iter()
            .map(|s| {
                (
                    &s.agent_id,
                    s.authorization_revision,
                    s.enabled,
                    self.source_plugins
                        .lease(&s.agent_id)
                        .map(|lease| lease.epoch),
                )
            })
            .collect();
        let available = |id: &str| {
            state
                .sources
                .iter()
                .find(|s| s.agent_id == id)
                .filter(|s| s.enabled && self.source_active(id))
        };
        let files = unique_files(&state);
        let (coverage, freshness, values) = match selector {
            S::Agent { id } => {
                self.validate_agent(Some(id))
                    .map_err(|_| failure(d::ErrorCode::NotFound))?;
                let family = self
                    .directory()
                    .family(id)
                    .ok_or_else(|| failure(d::ErrorCode::NotFound))?;
                let source =
                    available(family).ok_or_else(|| failure(d::ErrorCode::GrantRevoked))?;
                let (coverage, freshness) = source_state(source);
                let count = (coverage != d::Coverage::Unavailable).then(|| V::Count {
                    value: (files
                        .iter()
                        .filter(|f| {
                            f.agent == family && super::session_matches(Some(id), &f.summary)
                        })
                        .count() as u64)
                        .into(),
                });
                (coverage, freshness, vec![(F::SessionCount, count)])
            }
            S::Project { id, .. } => {
                if hidden_projects.contains(id) {
                    return Err(failure(d::ErrorCode::NotFound));
                }
                let count = files
                    .iter()
                    .filter(|f| {
                        available(&f.agent).is_some() && f.summary.project_id.as_ref() == Some(id)
                    })
                    .count();
                if count == 0 {
                    return Err(failure(d::ErrorCode::NotFound));
                }
                let complete = state.sources.iter().filter(|s| s.enabled).all(|s| {
                    self.source_active(&s.agent_id) && source_state(s).0 == d::Coverage::Complete
                });
                (
                    if complete {
                        d::Coverage::Complete
                    } else {
                        d::Coverage::Partial
                    },
                    if complete {
                        d::Freshness::Current
                    } else {
                        d::Freshness::Stale
                    },
                    vec![(
                        F::SessionCount,
                        Some(V::Count {
                            value: (count as u64).into(),
                        }),
                    )],
                )
            }
            S::Session { id, .. } => {
                let file = files
                    .iter()
                    .find(|f| f.summary.id == *id)
                    .ok_or_else(|| failure(d::ErrorCode::NotFound))?;
                let source =
                    available(&file.agent).ok_or_else(|| failure(d::ErrorCode::GrantRevoked))?;
                let (mut coverage, freshness) = source_state(source);
                if file.summary.partial && coverage == d::Coverage::Complete {
                    coverage = d::Coverage::Partial;
                }
                let usage = &file.summary.usage;
                let values = [
                    (F::InputTokens, usage.input_tokens),
                    (F::OutputTokens, usage.output_tokens),
                    (F::TotalTokens, usage.total_tokens),
                ]
                .into_iter()
                .map(|(field, value)| {
                    (
                        field,
                        if coverage == d::Coverage::Unavailable {
                            None
                        } else {
                            value.map(|value| V::Tokens {
                                value: value.into(),
                            })
                        },
                    )
                })
                .collect();
                (coverage, freshness, values)
            }
            _ => unreachable!("selector checked above"),
        };
        let missing_reason = if freshness == d::Freshness::Unread {
            o::MissingReason::Unread
        } else if coverage == d::Coverage::Unavailable {
            o::MissingReason::Unavailable
        } else {
            o::MissingReason::NotRecorded
        };
        let summary = o::ObjectSummary {
            api_version: o::API_VERSION.into(),
            target: selector.target(),
            coverage,
            freshness,
            captured_at: state.as_of.clone(),
            metrics: fields
                .iter()
                .map(|field| {
                    metric(
                        *field,
                        values
                            .iter()
                            .find(|(f, _)| f == field)
                            .and_then(|(_, value)| value.clone()),
                        missing_reason,
                    )
                })
                .collect(),
        };
        let read = o::ObjectSummaryRead {
            selector: selector.clone(),
            // Collection clocks may advance without changing retained metadata.
            // Bind quality, values and authority, rather than capture time.
            read_identity: crate::projects::hash(
                &serde_json::json!([
                    state.revision,
                    authorities,
                    hidden_projects,
                    selector,
                    summary.coverage,
                    summary.freshness,
                    summary.metrics
                ])
                .to_string(),
            ),
            summary,
        };
        read.validate(selector, fields)?;
        Ok(read)
    }
}
