//! Statistical selection only. Controller authorization and policy are separate.
use super::*;
use crate::observation::retention::{instant, key, normalized};
use chrono::{DateTime, Utc};

#[derive(Clone, Debug)]
/// Statistical Agent selection against the supplied trusted catalogue snapshot.
/// Rebuild when that catalogue changes; this type grants no source access.
pub struct AgentFilter {
    selector: String,
    // Only trusted catalogue source identifiers; no arbitrary original names.
    missing_product: Vec<(String, bool)>,
}

impl AgentFilter {
    pub fn new(directory: &AgentDirectory, selector: &str) -> Result<Self, LedgerError> {
        if !directory.valid_filter(selector) {
            return Err(LedgerError::InvalidQuery);
        }
        if directory.sources().len() > 1024 {
            return Err(LedgerError::BatchLimit);
        }
        Ok(Self {
            selector: selector.into(),
            missing_product: directory
                .sources()
                .iter()
                .map(|s| (s.id.clone(), directory.matches(Some(selector), &s.id, None)))
                .collect(),
        })
    }

    pub(super) fn sql(&self, namespace: &str, value: &str) -> String {
        let family = format!("json_extract({value},'$.agentIdentity.family')");
        let product = format!("json_extract({value},'$.agentIdentity.product')");
        let direct = key(namespace, "agent-family", &self.selector);
        let mut clauses = vec![format!("{family}='{direct}'")];
        if self.selector != "unknown" {
            let product_key = key(namespace, "agent-product", &self.selector);
            let mut term = format!("{product}='{product_key}'");
            if let Some(parent) = self.selector.strip_suffix("-unknown") {
                term.push_str(&format!(
                    " AND {family}!='{}'",
                    key(namespace, "agent-family", parent)
                ));
            }
            clauses.push(format!("({term})"));
        }
        let list = |matches_only: bool| {
            self.missing_product
                .iter()
                .filter(|(_, matched)| !matches_only || *matched)
                .map(|(source, _)| format!("'{}'", key(namespace, "agent-family", source)))
                .collect::<Vec<_>>()
                .join(",")
        };
        let known = list(false);
        let matched = list(true);
        let mut absent = Vec::new();
        if !matched.is_empty() {
            absent.push(format!("{family} IN ({matched})"));
        }
        if self.selector == "unknown" {
            absent.push(if known.is_empty() {
                "1".into()
            } else {
                format!("{family} NOT IN ({known})")
            });
        }
        if !absent.is_empty() {
            clauses.push(format!("({product} IS NULL AND ({}))", absent.join(" OR ")));
        }
        format!("({})", clauses.join(" OR "))
    }
}

#[derive(Clone, Debug, Default)]
/// Intersecting draft-statistics conditions. Dates are half-open; logical
/// summaries select root times and usage contributions select attempt times.
/// Session/project links remain caller-supplied annotations, not authorization.
pub struct GatewayFilter {
    pub start: Option<DateTime<Utc>>,
    pub end: Option<DateTime<Utc>>,
    pub agent: Option<AgentFilter>,
    pub session: Option<String>,
    pub project: Option<String>,
}

impl GatewayFilter {
    pub(super) fn validate(&self) -> Result<(), LedgerError> {
        if self.start.zip(self.end).is_some_and(|(a, b)| a >= b)
            || [self.start, self.end]
                .into_iter()
                .flatten()
                .any(|at| instant(&normalized(at)) != Some(at))
            || [self.session.as_deref(), self.project.as_deref()]
                .into_iter()
                .flatten()
                .any(|s| s.is_empty() || s.len() > 1024 || s.chars().any(char::is_control))
        {
            return Err(LedgerError::InvalidQuery);
        }
        Ok(())
    }

    pub(super) fn needs_attribution(&self) -> bool {
        self.session.is_some() || self.project.is_some()
    }

    // `value` is a host-owned alias, never caller input. Identifiers become
    // purpose-separated hex; time literals use the existing SQL quote helper.
    pub(super) fn sql(&self, namespace: &str, value: &str) -> String {
        let mut sql = String::new();
        for (bound, op) in [(self.start, ">="), (self.end, "<")] {
            if let Some(bound) = bound {
                let bound = aggregates::quote(&normalized(bound));
                sql.push_str(&format!(" AND instant_cmp(coalesce(json_extract({value},'$.startedAt'),''),{bound}){op}0"));
            }
        }
        if let Some(agent) = &self.agent {
            sql.push_str(&format!(" AND {}", agent.sql(namespace, value)));
        }
        for (field, id) in [("session", &self.session), ("project", &self.project)] {
            if let Some(id) = id {
                sql.push_str(&format!(
                    " AND json_extract(f.value,'$.attribution.{field}')='{}'",
                    key(namespace, field, id)
                ));
            }
        }
        sql
    }
}
