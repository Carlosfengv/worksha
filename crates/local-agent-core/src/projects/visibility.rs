use super::{ProjectCatalog, ProjectIdentity, hash, session_identity};
use crate::sessions::{NativeError, NativeResult, SessionSummary};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HiddenProject {
    pub id: String,
    pub name: String,
    pub path: String,
    pub hidden_at: String,
}

pub fn valid_id(id: &str) -> bool {
    id.strip_prefix("project:")
        .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
}

pub fn ensure_visible(id: &str, hidden: &[HiddenProject]) -> NativeResult<()> {
    if hidden.iter().any(|p| p.id == id) {
        return Err(NativeError::new(
            "project_hidden",
            "此项目已隐藏，可在“已隐藏项目”中恢复；原始会话和文件仍保留。",
        ));
    }
    Ok(())
}

pub fn mark_session(session: &mut SessionSummary, hidden: &[HiddenProject]) {
    session.project_hidden = session
        .project_id
        .as_ref()
        .is_some_and(|id| hidden.iter().any(|p| &p.id == id));
}

/// Directory aliases come from the same stable project identity used by Projects.
/// Keep nested cwd facets hidden too, including previously indexed missing roots.
pub fn hidden_paths(catalog: &ProjectCatalog, hidden: &[HiddenProject]) -> HashSet<String> {
    let mut paths: HashSet<_> = hidden.iter().map(|p| p.path.clone()).collect();
    let ids: HashSet<_> = hidden.iter().map(|p| &p.id).collect();
    for session in &catalog.sessions {
        if session
            .project_id
            .as_ref()
            .is_some_and(|id| ids.contains(id))
        {
            paths.extend(
                session
                    .project_path
                    .iter()
                    .chain(session.project_root.iter())
                    .cloned(),
            );
        }
    }
    paths
}

/// A display preference filters the project catalog, never the native session index.
pub fn visible_catalog(mut catalog: ProjectCatalog, hidden: &[HiddenProject]) -> ProjectCatalog {
    if hidden.is_empty() {
        // No visibility change requires a million-row membership pass.
        let revision = hash(&serde_json::json!([catalog.revision, hidden]).to_string());
        catalog.revision = u64::from_str_radix(&revision[..16], 16).expect("SHA256 hex");
        return catalog;
    }
    let ids: HashSet<_> = hidden.iter().map(|p| &p.id).collect();
    catalog
        .sessions
        .retain(|s| session_identity(s).is_none_or(|p| !ids.contains(&p.id)));
    let sessions: HashSet<_> = catalog.sessions.iter().map(|s| &s.id).collect();
    catalog.calls.retain(|c| sessions.contains(&c.session_id));
    catalog.writes.retain(|w| !ids.contains(&w.project_id));
    // Bind project cursors to both native history and the visibility snapshot.
    let revision = hash(&serde_json::json!([catalog.revision, hidden]).to_string());
    catalog.revision = u64::from_str_radix(&revision[..16], 16).expect("SHA256 hex");
    catalog
}

impl From<ProjectIdentity> for HiddenProject {
    fn from(p: ProjectIdentity) -> Self {
        Self {
            id: p.id,
            name: p.name,
            path: p.path,
            hidden_at: crate::sessions::now(),
        }
    }
}
