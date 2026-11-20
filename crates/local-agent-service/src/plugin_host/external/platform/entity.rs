use super::{BTreeMap, BTreeSet, Checked, Result, source_adapter::SummaryGrant};
use crate::AppState;
use crate::domain::activity::{recent, running_phase};
use crate::observation::GatewayActivity;
use local_agent_core::domain::timestamp;
use local_agent_core::{plugin::platform::*, sessions::ActivityAuthorization};
use rusqlite::Connection;

#[derive(Default)]
pub(super) struct SummaryGrants {
    native: BTreeMap<String, ActivityAuthorization>,
    directories: BTreeMap<String, SummaryGrant>,
}

impl SummaryGrants {
    pub(super) fn ensure(&self, state: &AppState, db: &Connection) -> Result<()> {
        for scope in self.native.values() {
            scope
                .ensure_current(&state.sessions)
                .checked("grant_revoked")?;
        }
        for grant in self.directories.values() {
            grant.ensure(state, db)?;
        }
        Ok(())
    }
}

fn empty(authorization: Authorization) -> HydratedEntity {
    HydratedEntity {
        authorization,
        freshness: Freshness::Unknown,
        title: None,
        source_id: None,
        activity_state: None,
        model: None,
        total_tokens: None,
        route_hash: None,
    }
}

pub(super) fn hydrate(
    state: &AppState,
    collection: &CollectionDef,
    grants: &[EntityKind],
    records: &mut [PlatformRecord],
) -> Result<SummaryGrants> {
    let references = records
        .iter()
        .flat_map(|record| record.values.iter())
        .filter_map(|(name, value)| {
            Some((
                collection.fields.get(name)?.kind.reference()?,
                value.as_str()?,
            ))
        })
        .filter(|(target, _)| grants.contains(target))
        .map(|(target, id)| (target, id.to_owned()))
        .collect::<BTreeSet<_>>();
    let mut summary_grants = SummaryGrants::default();
    let native: BTreeMap<_, _> = if references
        .iter()
        .any(|(target, _)| matches!(target, EntityKind::Session | EntityKind::Project))
    {
        state
            .sessions
            .domain_activity_authorizations()
            .checked("unavailable")?
            .into_iter()
            .map(|scope| (scope.source_id().to_owned(), scope))
            .collect()
    } else {
        BTreeMap::new()
    };

    let session_ids = references
        .iter()
        .filter(|(target, _)| *target == EntityKind::Session)
        .map(|(_, id)| id.as_str())
        .collect::<BTreeSet<_>>();
    let mut summaries = BTreeMap::new();
    if !session_ids.is_empty() {
        summaries.extend(
            state
                .sessions
                .platform_session_summaries(&session_ids)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|(id, summary)| {
                    let source = summary.source_id.as_ref()?;
                    let scope = native.get(source)?;
                    summary_grants
                        .native
                        .entry(source.clone())
                        .or_insert_with(|| scope.clone());
                    Some(((EntityKind::Session, id), summary))
                }),
        );
        if summaries.len() < session_ids.len()
            && let Ok(sources) = super::source_adapter::source_views(state)
        {
            for id in &session_ids {
                let key = (EntityKind::Session, (*id).to_owned());
                if let std::collections::btree_map::Entry::Vacant(entry) = summaries.entry(key)
                    && let Some((source, summary)) = sources
                        .iter()
                        .find_map(|source| Some((source, source.summary(id)?)))
                    && let Some(grant) = source.summary_grant()
                {
                    summary_grants
                        .directories
                        .insert(source.source.id.clone(), grant);
                    entry.insert(summary);
                }
            }
        }
        let now = chrono::Utc::now().timestamp_millis();
        let publication = state.observation.published_native_activity().ok();
        let gateway = state
            .observation
            .published_gateway_activity()
            .unwrap_or_default();
        let usable = publication.as_ref().filter(|p| {
            p.error.is_none()
                && state.observation.native_activity_collecting()
                && recent(p.checked_at.as_deref(), now, 15_000)
        });
        let authorized = usable
            .into_iter()
            .flat_map(|p| &p.authorizations)
            .filter(|scope| scope.ensure_current(&state.sessions).is_ok())
            .map(|scope| scope.source_id())
            .collect::<BTreeSet<_>>();
        for ((_, id), summary) in &mut summaries {
            let Some(source) = summary.source_id.as_deref() else {
                continue;
            };
            let native = usable
                .into_iter()
                .flat_map(|p| &p.rows)
                .filter(|r| {
                    r.session_id == *id && r.agent_id == source && authorized.contains(source)
                })
                .max_by_key(|r| {
                    activity_order(
                        r.message_started_at.as_deref(),
                        &r.event_at,
                        !running_phase(&r.phase),
                        &r.id,
                    )
                });
            let latest_gateway = gateway
                .iter()
                .filter(|r| {
                    r.request.session_id.as_ref() == Some(id) && r.request.agent_id == source
                })
                .max_by_key(|r| gateway_order(r));
            let Some(native) = native else {
                if let Some(row) = latest_gateway
                    && !row.stale
                    && (row.request.status == "in_progress"
                        || recent(
                            row.request
                                .ended_at
                                .as_deref()
                                .or(Some(&row.request.started_at)),
                            now,
                            30_000,
                        ))
                {
                    summary.activity_state = Some(row.phase.clone());
                    summary.model = row.request.model.clone();
                    summary.route_hash = route_hash(row);
                }
                continue;
            };
            if let Some(row) = latest_gateway
                && !row.stale
                && row.request.status == "in_progress"
                && let Some(message) = &row.request.activity_message
                && native.message_id() != Some(message.id.as_str())
                && timestamp(&message.started_at)
                    > native
                        .message_started_at
                        .as_deref()
                        .and_then(timestamp)
                        .or_else(|| timestamp(&native.event_at))
            {
                summary.activity_state = Some(row.phase.clone());
                summary.model = row.request.model.clone();
                summary.route_hash = route_hash(row);
                continue;
            }
            if native.stale || !recent(Some(&native.event_at), now, 30_000) {
                continue;
            }
            summary.activity_state = Some(native.phase.clone());
            summary.model = native.model.clone();
            if let Some(message) = native.message_id()
                && let Some(row) = gateway
                    .iter()
                    .filter(|r| {
                        !r.stale
                            && r.request.agent_id == source
                            && r.request.session_id.as_ref() == Some(id)
                            && r.request
                                .activity_message
                                .as_ref()
                                .is_some_and(|m| m.id == message)
                    })
                    .max_by_key(|r| gateway_order(r))
            {
                summary.model = summary.model.take().or_else(|| row.request.model.clone());
                summary.route_hash = route_hash(row);
            }
        }
    }
    let projects = references
        .iter()
        .filter(|(target, _)| *target == EntityKind::Project)
        .map(|(_, id)| id.as_str())
        .collect::<BTreeSet<_>>();
    if !projects.is_empty()
        && let Ok(hidden) = state
            .store_readers
            .read(|| (), |store| store.hidden_projects().map_err(|_| ()))
        && let Ok(titles) = state.sessions.platform_project_titles(
            &projects,
            &hidden.into_iter().map(|project| project.id).collect(),
        )
    {
        summaries.extend(titles.into_iter().filter_map(|(id, (source, title))| {
            let scope = native.get(&source)?;
            summary_grants
                .native
                .entry(source)
                .or_insert_with(|| scope.clone());
            Some(((EntityKind::Project, id), labeled(title)))
        }));
    }
    for (target, id) in references {
        if target == EntityKind::Provider {
            let provider = state
                .store_readers
                .read(|| (), |store| store.provider_by_id(&id).map_err(|_| ()));
            if let Ok(Some(provider)) = provider {
                summaries.insert((target, id), labeled(provider.name));
            }
        } else if target == EntityKind::Agent {
            let directory = state.sessions.directory();
            if let Some(product) = directory
                .products()
                .iter()
                .find(|product| product.id == id || product.config_id == id)
            {
                summaries.insert((target, id), labeled(product.name.clone()));
            }
        }
    }
    for record in records {
        for (field, raw) in &record.values {
            if let Some(target) = collection
                .fields
                .get(field)
                .and_then(|definition| definition.kind.reference())
                && let Some(object_id) = raw.as_str()
            {
                let hydrated = if grants.contains(&target) {
                    summaries
                        .get(&(target, object_id.into()))
                        .cloned()
                        .unwrap_or_else(|| empty(Authorization::Unknown))
                } else {
                    empty(Authorization::Unauthorized)
                };
                record.hydrated.insert(field.clone(), hydrated);
            }
        }
    }
    Ok(summary_grants)
}

fn labeled(title: String) -> HydratedEntity {
    HydratedEntity {
        title: Some(title),
        freshness: Freshness::Current,
        ..empty(Authorization::Authorized)
    }
}

fn activity_order<'a>(
    started: Option<&str>,
    event: &str,
    terminal: bool,
    id: &'a str,
) -> (Option<i64>, Option<i64>, bool, &'a str) {
    let event = timestamp(event);
    (started.and_then(timestamp).or(event), event, terminal, id)
}

fn gateway_order(row: &GatewayActivity) -> (Option<i64>, Option<i64>, bool, &str) {
    let req = &row.request;
    activity_order(
        req.activity_message
            .as_ref()
            .map(|m| m.started_at.as_str())
            .or(Some(&req.started_at)),
        req.ended_at.as_deref().unwrap_or(&req.started_at),
        req.status != "in_progress",
        &req.id,
    )
}

fn route_hash(row: &GatewayActivity) -> Option<String> {
    row.request
        .routing
        .as_ref()
        .map(|route| super::package::hash(route.route_version_id.as_bytes()))
}

#[cfg(test)]
mod activity_tests {
    use super::*;
    #[test]
    fn latest_turn_wins_regardless_of_input_order_and_terminal_wins_ties() {
        let old = Some("2026-11-19T10:00:00Z");
        let new = Some("2026-11-19T10:00:10Z");
        let rows = [
            (old, "2026-11-19T10:00:15Z", true, "old"),
            (new, "2026-11-19T10:00:11Z", false, "step"),
            (new, "2026-11-19T10:00:11Z", true, "done"),
        ];
        for order in [[0, 1, 2], [2, 1, 0], [1, 2, 0]] {
            assert_eq!(
                order
                    .into_iter()
                    .map(|index| rows[index])
                    .max_by_key(|(s, t, end, id)| activity_order(*s, t, *end, id))
                    .unwrap()
                    .3,
                "done"
            );
        }
    }
}
