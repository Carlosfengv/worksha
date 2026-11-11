use super::{ProjectIdentity, hash, project_kind, visibility};
use crate::sessions::{NativeError, NativeResult, SessionSummary};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, path::Path};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingProject {
    pub id: String,
    pub name: String,
    pub path: String,
    pub project_kind: &'static str,
}

impl RoutingProject {
    pub(crate) fn from_session(session: &SessionSummary) -> Option<Self> {
        let path = session
            .project_root
            .as_ref()
            .or(session.project_path.as_ref())?;
        let directory = Path::new(path);
        if !directory.is_absolute() {
            return None;
        }
        Some(Self {
            id: session.project_id.clone()?,
            name: directory
                .file_name()
                .map_or_else(|| "/".into(), |part| part.to_string_lossy().into_owned()),
            path: path.clone(),
            project_kind: project_kind(directory),
        })
    }
}

impl From<ProjectIdentity> for RoutingProject {
    fn from(project: ProjectIdentity) -> Self {
        Self {
            id: project.id,
            name: project.name,
            path: project.path,
            project_kind: project.project_kind,
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoutingProjectQuery {
    pub query: Option<String>,
    pub cursor: Option<String>,
    pub selected_id: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingProjectPage {
    pub items: Vec<RoutingProject>,
    pub next_cursor: Option<String>,
    pub selected: Option<RoutingProject>,
}

pub fn page(
    mut choices: Vec<RoutingProject>,
    query: RoutingProjectQuery,
    hidden: &[visibility::HiddenProject],
) -> NativeResult<RoutingProjectPage> {
    const PAGE_SIZE: usize = 40;
    if query
        .query
        .as_ref()
        .is_some_and(|text| text.chars().count() > 300)
        || query
            .selected_id
            .as_deref()
            .is_some_and(|id| !visibility::valid_id(id))
    {
        return Err(NativeError::new("invalid_input", "项目筛选条件无效"));
    }
    let hidden: HashSet<_> = hidden.iter().map(|project| project.id.as_str()).collect();
    choices.retain(|project| !hidden.contains(project.id.as_str()));
    let selected = query
        .selected_id
        .as_deref()
        .and_then(|id| choices.iter().find(|project| project.id == id))
        .cloned();
    let needle = query.query.as_deref().unwrap_or("").trim().to_lowercase();
    if !needle.is_empty() {
        choices.retain(|project| {
            format!("{} {}", project.name, project.path)
                .to_lowercase()
                .contains(&needle)
        });
    }
    choices.sort_by_cached_key(|project| (project.name.to_lowercase(), project.id.clone()));
    let revision = hash(
        &serde_json::json!([
            needle,
            choices
                .iter()
                .map(|project| &project.id)
                .collect::<Vec<_>>()
        ])
        .to_string(),
    );
    let offset = if let Some(cursor) = query.cursor {
        let invalid = || NativeError::new("invalid_input", "项目分页位置无效");
        let (version, offset) = cursor.rsplit_once(':').ok_or_else(invalid)?;
        if version != revision {
            return Err(NativeError::new("stale_snapshot", "项目已更新，请重新读取"));
        }
        offset
            .parse::<usize>()
            .ok()
            .filter(|value| *value < choices.len())
            .ok_or_else(invalid)?
    } else {
        0
    };
    let next_cursor =
        (offset + PAGE_SIZE < choices.len()).then(|| format!("{revision}:{}", offset + PAGE_SIZE));
    Ok(RoutingProjectPage {
        items: choices.into_iter().skip(offset).take(PAGE_SIZE).collect(),
        next_cursor,
        selected,
    })
}
