//! Paginated, read-only drilldown of the same canonical attempts as Summary.
use super::{Index, Summary};
use crate::{
    projects,
    provider_cost::{Cost, Totals},
    sessions::{NativeError, NativeResult, Usage},
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Query {
    pub scope: String,
    pub id: String,
    pub agent: Option<String>,
    pub days: Option<u16>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub cursor: Option<String>,
}
impl Query {
    pub fn validate(&self) -> NativeResult<()> {
        self.validate_with_directory(crate::agents::builtin_directory())
    }
    pub fn validate_with_directory(
        &self,
        directory: &crate::agents::AgentDirectory,
    ) -> NativeResult<()> {
        if !["session", "project"].contains(&self.scope.as_str())
            || self.id.is_empty()
            || self.id.len() > 512
            || self.provider.as_ref().is_some_and(|v| v.len() > 1024)
            || self.model.as_ref().is_some_and(|v| v.len() > 1024)
            || self.cursor.as_ref().is_some_and(|v| v.len() > 100)
            || (self.scope == "session" && (self.agent.is_some() || self.days.is_some()))
        {
            return Err(NativeError::new("invalid_input", "费用筛选条件无效"));
        }
        projects::validate_with_directory(
            &projects::ProjectQuery {
                agent: self.agent.clone(),
                days: self.days,
                ..Default::default()
            },
            directory,
        )
    }
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Call {
    pub id: String,
    pub request_id: String,
    pub session_id: Option<String>,
    pub provider_key: String,
    pub provider: String,
    pub model: Option<String>,
    pub time: String,
    pub status: String,
    pub usage: Usage,
    pub cost: Cost,
    pub association: &'static str,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub provider_key: String,
    pub provider: String,
    pub model: Option<String>,
    pub attempts: usize,
    pub cost: Totals,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub summary: Summary,
    pub filtered_cost: Totals,
    pub total: usize,
    pub providers: Vec<Group>,
    pub models: Vec<Group>,
    pub filtered_providers: Vec<Group>,
    pub filtered_models: Vec<Group>,
    pub calls: Vec<Call>,
    pub next_cursor: Option<String>,
}
impl Index {
    pub fn details(&self, ids: &[String], query: &Query) -> NativeResult<Page> {
        query.validate_with_directory(&self.directory)?;
        let ids: HashSet<_> = ids.iter().collect();
        let mut seen = HashSet::new();
        let mut scoped: Vec<_> = self
            .attempts
            .iter()
            .filter(|a| {
                a.session.as_ref().is_some_and(|s| ids.contains(s))
                    && self.directory.matches(
                        query.agent.as_deref(),
                        &a.family,
                        a.product.as_deref(),
                    )
                    && seen.insert(&a.id)
            })
            .map(|a| a.record.clone())
            .collect();
        scoped.sort_by(|a, b| b.time.cmp(&a.time).then_with(|| a.id.cmp(&b.id)));
        // Candidate groups cover the full scope, independent of call pagination.
        let mut providers = BTreeMap::new();
        let mut models = BTreeMap::new();
        for a in &scoped {
            for (map, key, model) in [
                (&mut providers, (a.provider_key.clone(), None), None),
                (
                    &mut models,
                    (a.provider_key.clone(), a.model.clone()),
                    a.model.clone(),
                ),
            ] {
                let g = map.entry(key).or_insert_with(|| Group {
                    provider_key: a.provider_key.clone(),
                    provider: a.provider.clone(),
                    model,
                    attempts: 0,
                    cost: Totals::default(),
                });
                g.attempts += 1;
                g.cost.add(&a.cost);
            }
        }
        let summary = self.summary(
            &ids.into_iter().cloned().collect::<Vec<_>>(),
            query.agent.as_deref(),
        );
        let mut canonical = query.clone();
        canonical.cursor = None;
        // Bind pagination to scoped evidence, not unrelated live Session index revisions.
        // The service separately rechecks authorization and catalog consistency.
        let fingerprint = projects::hash(
            &serde_json::to_string(&(&canonical, &scoped, &summary))
                .map_err(|_| NativeError::new("unavailable", "费用快照不可用"))?,
        );
        let offset = if let Some(cursor) = &query.cursor {
            let (version, offset) = cursor
                .split_once(':')
                .ok_or_else(|| NativeError::new("invalid_input", "费用游标无效"))?;
            if version != fingerprint {
                return Err(NativeError::new(
                    "stale_snapshot",
                    "费用记录已更新，请刷新明细",
                ));
            }
            offset
                .parse::<usize>()
                .map_err(|_| NativeError::new("invalid_input", "费用游标无效"))?
        } else {
            0
        };
        let filtered: Vec<_> = scoped
            .into_iter()
            .filter(|a| {
                query.provider.as_ref().is_none_or(|p| *p == a.provider_key)
                    && query.model.as_ref().is_none_or(|m| {
                        if m == "_unknown" {
                            a.model.is_none()
                        } else {
                            a.model.as_ref() == Some(m)
                        }
                    })
            })
            .collect();
        let total = filtered.len();
        if offset > total || (offset > 0 && offset % 40 != 0) {
            return Err(NativeError::new("invalid_input", "费用游标超出范围"));
        }
        let mut filtered_cost = Totals::default();
        for a in &filtered {
            filtered_cost.add(&a.cost);
        }
        let mut fp = BTreeMap::new();
        let mut fm = BTreeMap::new();
        for a in &filtered {
            for (map, key, model) in [
                (&mut fp, (a.provider_key.clone(), None), None),
                (
                    &mut fm,
                    (a.provider_key.clone(), a.model.clone()),
                    a.model.clone(),
                ),
            ] {
                let g = map.entry(key).or_insert_with(|| Group {
                    provider_key: a.provider_key.clone(),
                    provider: a.provider.clone(),
                    model,
                    attempts: 0,
                    cost: Totals::default(),
                });
                g.attempts += 1;
                g.cost.add(&a.cost);
            }
        }
        let calls = filtered.into_iter().skip(offset).take(40).collect();
        Ok(Page {
            filtered_providers: fp.into_values().collect(),
            filtered_models: fm.into_values().collect(),
            summary,
            filtered_cost,
            total,
            providers: providers.into_values().collect(),
            models: models.into_values().collect(),
            calls,
            next_cursor: (offset + 40 < total).then(|| format!("{fingerprint}:{}", offset + 40)),
        })
    }
}
