mod command;
pub mod git;
pub mod listeners;
pub mod ports;
mod query_cache;
pub mod readers;
pub mod routing;
#[cfg(test)]
mod tests;
pub mod visibility;
pub mod writes;

use crate::sessions::{
    CallSummary, NativeError, NativeResult, SessionSummary, Usage, now, session_matches,
};
use chrono::{DateTime, FixedOffset, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    path::{Component, Path, PathBuf},
};

pub fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
pub fn canonical(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    if let Ok(path) = path.canonicalize() {
        return Some(path);
    }
    let mut result = PathBuf::from("/");
    for component in path.components() {
        match component {
            Component::Normal(p) => result.push(p),
            Component::ParentDir => {
                result.pop();
            }
            _ => {}
        }
    }
    // Resolve existing ancestors even for a newly created/deleted file (e.g. /var -> /private/var).
    let mut ancestor = result.as_path();
    while let Some(parent) = ancestor.parent() {
        if let Ok(prefix) = ancestor.canonicalize()
            && let Ok(tail) = result.strip_prefix(ancestor)
        {
            return Some(prefix.join(tail));
        }
        ancestor = parent;
    }
    Some(result)
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectIdentity {
    pub id: String,
    pub name: String,
    pub path: String,
    pub git_root: Option<String>,
    pub location_state: String,
    pub project_kind: &'static str,
}
fn project_kind(path: &Path) -> &'static str {
    if ["/tmp", "/private/tmp", "/var/tmp", "/private/var/tmp"]
        .iter()
        .any(|root| path.starts_with(root))
    {
        return "temporary";
    }
    for root in ["/var/folders", "/private/var/folders"] {
        if let Ok(tail) = path.strip_prefix(root)
            && tail
                .components()
                .nth(2)
                .is_some_and(|c| matches!(c.as_os_str().to_str(), Some("T" | "C")))
        {
            return "temporary";
        }
    }
    let parts = path
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect::<Vec<_>>();
    if parts.windows(4).any(|p| {
        p[..3] == ["Library", "Application Support", "app.localagent.desktop"]
            && matches!(p[3], "programming-checks" | "connection-check")
    }) {
        return "verification";
    }
    "workspace"
}
pub fn identity(value: &str) -> Option<ProjectIdentity> {
    let path = canonical(Path::new(value))?;
    let available = path.is_dir();
    let root = if available {
        path.ancestors()
            .find(|p| p.join(".git").exists())
            .map(Path::to_path_buf)
    } else {
        None
    };
    let path = root.clone().unwrap_or(path);
    let path_text = path.to_string_lossy().into_owned();
    Some(ProjectIdentity {
        project_kind: project_kind(&path),
        id: format!("project:{}", hash(&path_text)),
        name: path
            .file_name()
            .map(|p| p.to_string_lossy().into())
            .unwrap_or_else(|| "/".into()),
        path: path_text,
        git_root: root.map(|p| p.to_string_lossy().into()),
        location_state: if available {
            "available"
        } else {
            "unavailable"
        }
        .into(),
    })
}
pub fn session_identity(session: &SessionSummary) -> Option<ProjectIdentity> {
    let root = session
        .project_root
        .as_deref()
        .or(session.project_path.as_deref())?;
    let path = canonical(Path::new(root))?;
    let available = path.is_dir();
    let git_root =
        (available && path.join(".git").exists()).then(|| path.to_string_lossy().into_owned());
    Some(ProjectIdentity {
        project_kind: project_kind(&path),
        id: session
            .project_id
            .clone()
            .unwrap_or_else(|| format!("project:{}", hash(&path.to_string_lossy()))),
        name: path
            .file_name()
            .map(|s| s.to_string_lossy().into())
            .unwrap_or_else(|| "/".into()),
        path: path.to_string_lossy().into(),
        git_root,
        location_state: if available {
            "available"
        } else {
            "unavailable"
        }
        .into(),
    })
}
#[derive(Clone)]
pub struct ProjectCatalog {
    pub sessions: Vec<SessionSummary>,
    pub writes: Vec<writes::FileWrite>,
    pub calls: crate::sessions::CallCatalog,
    pub revision: u64,
    pub as_of: String,
    pub coverage: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectQuery {
    /// None retains the legacy API scope. The desktop defaults to workspace.
    pub scope: Option<String>,
    pub agent: Option<String>,
    pub days: Option<u16>,
    pub query: Option<String>,
    pub cursor: Option<String>,
}
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTotals {
    pub session_count: usize,
    pub usage: Usage,
    pub usage_scope: &'static str,
    pub model_calls: usize,
    pub usage_missing_calls: usize,
    pub usage_unknown_sessions: usize,
    pub date_unknown_sessions: usize,
    pub disk_bytes: u64,
    pub disk_allocated_bytes: Option<u64>,
    pub disk_unknown_sessions: usize,
    pub write_count: usize,
    pub distinct_files: usize,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTotals {
    pub agent_id: String,
    pub totals: ProjectTotals,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSummary {
    #[serde(skip)]
    pub selected_session_ids: Vec<String>,
    #[serde(flatten)]
    pub identity: ProjectIdentity,
    pub totals: ProjectTotals,
    pub agents: Vec<AgentTotals>,
    pub last_activity: Option<String>,
    pub listener_count: Option<usize>,
    pub known_listener_count: usize,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectPage {
    #[serde(skip)]
    pub selected_session_ids: Vec<String>,
    pub items: Vec<ProjectSummary>,
    pub total: usize,
    pub next_cursor: Option<String>,
    pub summary: ProjectTotals,
    pub unassigned_sessions: usize,
    pub date_unknown_excluded: usize,
    pub missing_counts: usize,
    pub as_of: String,
    pub coverage: String,
    pub applied_filter: ProjectQuery,
    pub listeners: listeners::ListenerSnapshot,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDetail {
    pub project: ProjectSummary,
    pub sessions: Vec<SessionSummary>,
    pub writes: Vec<writes::FileWrite>,
    pub git: git::GitSnapshot,
    pub listeners: listeners::ListenerSnapshot,
    pub as_of: String,
    pub coverage: String,
    pub applied_filter: ProjectQuery,
}
pub fn validate(query: &ProjectQuery) -> NativeResult<()> {
    validate_with_directory(query, crate::agents::builtin_directory())
}
pub fn validate_with_directory(
    query: &ProjectQuery,
    directory: &crate::agents::AgentDirectory,
) -> NativeResult<()> {
    if query
        .scope
        .as_deref()
        .is_some_and(|s| !["workspace", "temporary", "all"].contains(&s))
        || query
            .agent
            .as_deref()
            .is_some_and(|a| !directory.valid_filter(a))
        || query.days.is_some_and(|d| ![1, 7, 30].contains(&d))
        || query
            .query
            .as_ref()
            .is_some_and(|q| q.chars().count() > 300)
    {
        return Err(NativeError::new("invalid_input", "项目筛选条件无效"));
    }
    Ok(())
}
pub fn in_range(time: Option<&str>, days: Option<u16>) -> bool {
    in_range_at(time, days, Utc::now())
}
fn in_range_at(time: Option<&str>, days: Option<u16>, now: DateTime<Utc>) -> bool {
    let Some(days) = days else {
        return true;
    };
    let offset = FixedOffset::east_opt(8 * 3600).unwrap();
    let today = now.with_timezone(&offset).date_naive();
    let start = (today - chrono::Duration::days(i64::from(days) - 1))
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_local_timezone(offset)
        .unwrap();
    time.and_then(|v| DateTime::parse_from_rfc3339(v).ok())
        .is_some_and(|t| t >= start && t <= now)
}
fn aggregate(sessions: &[SessionSummary], writes: &[writes::FileWrite]) -> ProjectTotals {
    let mut totals = ProjectTotals {
        session_count: sessions.len(),
        ..Default::default()
    };
    let mut unique_files = HashSet::new();
    if sessions.is_empty() {
        totals.usage.input_tokens = Some(0);
        totals.usage.output_tokens = Some(0);
        totals.usage.total_tokens = Some(0);
        totals.disk_allocated_bytes = Some(0);
    }
    for session in sessions {
        totals.usage.add(&session.usage);
        totals.usage_unknown_sessions +=
            usize::from(["partial", "unknown"].contains(&session.usage_quality));
        totals.date_unknown_sessions += usize::from(session.created_at.is_none());
        totals.disk_bytes = totals.disk_bytes.saturating_add(session.disk_bytes);
        if let Some(bytes) = session.disk_allocated_bytes {
            totals.disk_allocated_bytes = Some(
                totals
                    .disk_allocated_bytes
                    .unwrap_or(0)
                    .saturating_add(bytes),
            );
        } else {
            totals.disk_unknown_sessions += 1;
        }
    }
    for write in writes {
        unique_files.extend(write.files.iter().map(|f| &f.path));
    }
    totals.write_count = writes.len();
    totals.distinct_files = unique_files.len();
    totals
}
// Session participation and physical disk occupancy can overlap between clients.
// Model usage is attributed independently, once per native response, never by creator.
fn totals<'a>(
    sessions: &[SessionSummary],
    writes: &[writes::FileWrite],
    calls: impl IntoIterator<Item = &'a CallSummary>,
) -> ProjectTotals {
    let mut totals = aggregate(sessions, writes);
    totals.usage = Usage::default();
    totals.usage_scope = "model_responses";
    // Every caller consumes selected_calls or a subset of it. Deduplication
    // happens once before project and Agent grouping, in original source order.
    for call in calls.into_iter().filter(|c| c.kind == "model") {
        totals.model_calls += 1;
        totals.usage_missing_calls +=
            usize::from(call.usage.as_ref().is_none_or(|u| u.total_tokens.is_none()));
        if let Some(usage) = &call.usage {
            totals.usage.add(usage);
        }
    }
    if totals.model_calls == 0 {
        totals.usage = Usage {
            input_tokens: Some(0),
            output_tokens: Some(0),
            total_tokens: Some(0),
            ..Default::default()
        };
    }
    totals
}
fn selected_sessions(catalog: &ProjectCatalog, query: &ProjectQuery) -> Vec<SessionSummary> {
    let mut seen = HashSet::new();
    catalog
        .sessions
        .iter()
        .filter(|s| {
            seen.insert(&s.id)
                && in_range(s.created_at.as_deref(), query.days)
                && session_matches(query.agent.as_deref(), s)
        })
        .cloned()
        .collect()
}
fn selected_writes(catalog: &ProjectCatalog, query: &ProjectQuery) -> Vec<writes::FileWrite> {
    let mut seen = HashSet::new();
    catalog
        .writes
        .iter()
        .filter(|w| {
            seen.insert(&w.id)
                && in_range(w.occurred_at.as_deref(), query.days)
                && catalog.calls.directory().matches(
                    query.agent.as_deref(),
                    &w.agent_id,
                    w.agent_product_id.as_deref(),
                )
        })
        .cloned()
        .collect()
}
fn selected_calls<'a>(
    catalog: &'a ProjectCatalog,
    query: &ProjectQuery,
    sessions: &[SessionSummary],
) -> Vec<&'a CallSummary> {
    let ids: HashSet<_> = sessions.iter().map(|s| &s.id).collect();
    let mut seen = HashSet::new();
    catalog
        .calls
        .iter()
        .filter(|c| {
            ids.contains(&c.session_id)
                && seen.insert(&c.id)
                && catalog.calls.directory().matches(
                    query.agent.as_deref(),
                    &c.agent_id,
                    c.agent_product_id.as_deref(),
                )
        })
        .collect()
}
pub fn groups(
    catalog: &ProjectCatalog,
    query: &ProjectQuery,
    listeners: &listeners::ListenerSnapshot,
) -> Vec<ProjectSummary> {
    let calls = selected_calls(catalog, query, &selected_sessions(catalog, query));
    groups_from_calls(catalog, query, listeners, &calls)
}
fn groups_from_calls(
    catalog: &ProjectCatalog,
    query: &ProjectQuery,
    listeners: &listeners::ListenerSnapshot,
    selected: &[&CallSummary],
) -> Vec<ProjectSummary> {
    let mut writes_by_project = BTreeMap::<String, Vec<writes::FileWrite>>::new();
    for write in selected_writes(catalog, query) {
        writes_by_project
            .entry(write.project_id.clone())
            .or_default()
            .push(write);
    }
    let mut calls_by_session = BTreeMap::<&str, Vec<&CallSummary>>::new();
    for &call in selected {
        calls_by_session
            .entry(call.session_id.as_str())
            .or_default()
            .push(call);
    }
    let mut all = BTreeMap::<String, (ProjectIdentity, Vec<SessionSummary>)>::new();
    let mut seen = HashSet::new();
    for session in &catalog.sessions {
        if !seen.insert(&session.id) {
            continue;
        }
        if let Some(project) = session_identity(session) {
            all.entry(project.id.clone())
                .or_insert_with(|| (project, vec![]))
                .1
                .push(session.clone());
        }
    }
    let identities: Vec<_> = all.values().map(|(p, _)| p.clone()).collect();
    let mut groups: Vec<_> = all
        .into_values()
        .map(|(project, all_sessions)| {
            let last_activity = all_sessions
                .iter()
                .filter_map(|s| s.updated_at.clone())
                .max();
            let sessions: Vec<_> = all_sessions
                .into_iter()
                .filter(|s| {
                    in_range(s.created_at.as_deref(), query.days)
                        && session_matches(query.agent.as_deref(), s)
                })
                .collect();
            // Historical writes have their own occurrence date; do not filter them by session creation.
            let writes = writes_by_project.remove(&project.id).unwrap_or_default();
            let calls = sessions
                .iter()
                .flat_map(|s| calls_by_session.remove(s.id.as_str()).unwrap_or_default())
                .collect::<Vec<_>>();
            let totals = totals(&sessions, &writes, calls.iter().copied());
            let selected_session_ids = sessions.iter().map(|s| s.id.clone()).collect();
            let mut by_agent = BTreeMap::<String, Vec<SessionSummary>>::new();
            for session in sessions {
                let mut keys = session.agent_products.clone();
                if session.has_unknown_client {
                    keys.push(catalog.calls.directory().group(&session.agent_id, None));
                }
                if keys.is_empty() {
                    keys.push(
                        catalog
                            .calls
                            .directory()
                            .group(&session.agent_id, session.agent_product_id.as_deref()),
                    );
                }
                keys.sort();
                keys.dedup();
                for key in keys.into_iter().filter(|k| {
                    catalog
                        .calls
                        .directory()
                        .matches_group(query.agent.as_deref(), k)
                }) {
                    by_agent.entry(key).or_default().push(session.clone());
                }
            }
            for write in &writes {
                by_agent
                    .entry(
                        catalog
                            .calls
                            .directory()
                            .group(&write.agent_id, write.agent_product_id.as_deref()),
                    )
                    .or_default();
            }
            let call_agents: HashSet<_> = calls
                .iter()
                .map(|call| (&call.agent_id, call.agent_product_id.as_deref()))
                .collect();
            for (family, product) in call_agents {
                by_agent
                    .entry(catalog.calls.directory().group(family, product))
                    .or_default();
            }
            let agents = by_agent
                .into_iter()
                .map(|(agent_id, sessions)| {
                    let agent_writes = writes
                        .iter()
                        .filter(|w| {
                            catalog.calls.directory().matches(
                                Some(&agent_id),
                                &w.agent_id,
                                w.agent_product_id.as_deref(),
                            )
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    AgentTotals {
                        totals: self::totals(
                            &sessions,
                            &agent_writes,
                            calls.iter().copied().filter(|c| {
                                catalog.calls.directory().matches(
                                    Some(&agent_id),
                                    &c.agent_id,
                                    c.agent_product_id.as_deref(),
                                )
                            }),
                        ),
                        agent_id,
                    }
                })
                .collect();
            let matched = listeners.for_project(&project, &identities);
            ProjectSummary {
                selected_session_ids,
                identity: project,
                totals,
                agents,
                last_activity,
                listener_count: matched.count(),
                known_listener_count: matched.items.len(),
            }
        })
        .collect();
    groups.sort_by(|a, b| {
        b.last_activity
            .cmp(&a.last_activity)
            .then(a.identity.id.cmp(&b.identity.id))
    });
    groups
}
pub fn query(
    catalog: &ProjectCatalog,
    query: ProjectQuery,
    snapshot: &listeners::ListenerSnapshot,
) -> NativeResult<ProjectPage> {
    validate_with_directory(&query, catalog.calls.directory())?;
    let needle = query.query.as_deref().unwrap_or("").trim().to_lowercase();
    let call_sessions = selected_sessions(catalog, &query);
    let call_session_ids: HashSet<_> = call_sessions.iter().map(|s| s.id.as_str()).collect();
    let calls = selected_calls(catalog, &query, &call_sessions);
    let mut items = groups_from_calls(catalog, &query, snapshot, &calls);
    items.retain(|p| {
        query.scope.as_deref().is_none_or(|scope| match scope {
            "workspace" => p.identity.project_kind == "workspace",
            "temporary" => p.identity.project_kind != "workspace",
            _ => true,
        }) && (query.agent.is_none() || p.totals.session_count > 0 || p.totals.write_count > 0)
            && format!("{} {}", p.identity.name, p.identity.path)
                .to_lowercase()
                .contains(&needle)
    });
    let rev = hash(
        &serde_json::json!([
            catalog.revision,
            query.days,
            query.agent,
            query.query,
            query.scope
        ])
        .to_string(),
    );
    let offset = if let Some(cursor) = &query.cursor {
        let (version, offset) = cursor
            .rsplit_once(':')
            .ok_or_else(|| NativeError::new("invalid_input", "分页位置无效"))?;
        if version != rev {
            return Err(NativeError::new(
                "stale_snapshot",
                "项目会话已更新，请刷新后继续",
            ));
        }
        offset
            .parse::<usize>()
            .ok()
            .filter(|v| *v <= 10000)
            .ok_or_else(|| NativeError::new("invalid_input", "分页位置无效"))?
    } else {
        0
    };
    let ids: HashSet<_> = items.iter().map(|p| p.identity.id.as_str()).collect();
    let sessions: Vec<_> = selected_sessions(catalog, &query)
        .into_iter()
        .filter(|s| session_identity(s).is_some_and(|p| ids.contains(p.id.as_str())))
        .collect();
    let writes: Vec<_> = selected_writes(catalog, &query)
        .into_iter()
        .filter(|w| {
            ids.contains(w.project_id.as_str()) && in_range(w.occurred_at.as_deref(), query.days)
        })
        .collect();
    // Reuse only exactly the same Session set. A project/search/date filter may
    // change which duplicate ID wins, so narrower selections still reselect.
    let summary_calls = if sessions.len() == call_session_ids.len()
        && sessions
            .iter()
            .all(|s| call_session_ids.contains(s.id.as_str()))
    {
        calls
    } else {
        selected_calls(catalog, &query, &sessions)
    };
    let summary = totals(&sessions, &writes, summary_calls.iter().copied());
    let total = items.len();
    let unassigned_sessions = selected_sessions(catalog, &query)
        .into_iter()
        .filter(|s| session_identity(s).is_none() && in_range(s.created_at.as_deref(), query.days))
        .count();
    let mut listener_summary = snapshot.clone();
    listener_summary.items.clear();
    let selected_session_ids = items
        .iter()
        .flat_map(|p| p.selected_session_ids.clone())
        .collect();
    Ok(ProjectPage {
        selected_session_ids,
        items: items.into_iter().skip(offset).take(40).collect(),
        total,
        next_cursor: (offset + 40 < total).then(|| format!("{rev}:{}", offset + 40)),
        missing_counts: summary.usage_missing_calls,
        summary,
        unassigned_sessions,
        date_unknown_excluded: if query.days.is_some() {
            catalog
                .sessions
                .iter()
                .filter(|s| {
                    s.created_at
                        .as_deref()
                        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                        .is_none()
                })
                .count()
        } else {
            0
        },
        as_of: catalog.as_of.clone(),
        coverage: catalog.coverage.clone(),
        applied_filter: query,
        listeners: listener_summary,
    })
}
pub fn detail(
    catalog: &ProjectCatalog,
    id: &str,
    query: ProjectQuery,
    snapshot: &listeners::ListenerSnapshot,
) -> NativeResult<ProjectDetail> {
    detail_with_git(catalog, id, query, snapshot, &git::snapshot)
}
fn detail_with_git(
    catalog: &ProjectCatalog,
    id: &str,
    query: ProjectQuery,
    snapshot: &listeners::ListenerSnapshot,
    read_git: &dyn Fn(&ProjectIdentity) -> git::GitSnapshot,
) -> NativeResult<ProjectDetail> {
    validate_with_directory(&query, catalog.calls.directory())?;
    let all = groups(catalog, &query, snapshot);
    let project = all
        .iter()
        .find(|p| p.identity.id == id)
        .cloned()
        .ok_or_else(|| NativeError::new("not_found", "项目不存在或会话来源已断开"))?;
    let identities: Vec<_> = all.into_iter().map(|p| p.identity).collect();
    let mut sessions: Vec<_> = selected_sessions(catalog, &query)
        .into_iter()
        .filter(|s| {
            session_identity(s).is_some_and(|p| p.id == id)
                && in_range(s.created_at.as_deref(), query.days)
        })
        .collect();
    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then(a.id.cmp(&b.id)));
    let mut writes: Vec<_> = selected_writes(catalog, &query)
        .into_iter()
        .filter(|w| w.project_id == id && in_range(w.occurred_at.as_deref(), query.days))
        .collect();
    writes.sort_by(|a, b| b.occurred_at.cmp(&a.occurred_at).then(a.id.cmp(&b.id)));
    let git = read_git(&project.identity);
    let listeners = snapshot.for_project(&project.identity, &identities);
    Ok(ProjectDetail {
        project,
        sessions,
        writes,
        git,
        listeners,
        as_of: now(),
        coverage: catalog.coverage.clone(),
        applied_filter: query,
    })
}
