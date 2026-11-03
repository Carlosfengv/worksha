use super::*;
use crate::sessions::source_plugins::SourceContext;

pub(in crate::sessions) fn claude_global(
    context: &SourceContext<'_>,
    scope: &mut ConfigScope<'_>,
    inventory: &mut Inventory,
) {
    let claude_path = context.home.join(".claude.json");
    if let Some((data, modified)) = inventory.json("claude", &claude_path) {
        inventory.json_entries(
            "claude",
            &data["mcpServers"],
            &claude_path,
            modified.clone(),
            "local",
            None,
        );
        if let Some(entries) = data["projects"].as_object() {
            for (project, value) in entries.iter().take(2000).filter(|(p, _)| valid_project(p)) {
                if !inventory.within_budget() {
                    inventory.issue("claude", &claude_path);
                    break;
                }
                scope.projects.insert(project.clone());
                inventory.json_entries(
                    "claude",
                    &value["mcpServers"],
                    &claude_path,
                    modified.clone(),
                    "project_local",
                    Some(project),
                );
            }
            if entries.len() > 2000 {
                inventory.blocked_agents.insert("claude".into());
                inventory
                    .warnings
                    .insert("Claude 项目超过 2000 项，配置扫描不完整。".into());
            }
        }
    }
}

pub(in crate::sessions) fn antigravity_global(
    context: &SourceContext<'_>,
    scope: &mut ConfigScope<'_>,
    inventory: &mut Inventory,
) {
    let mut configs = vec![
        (
            context.home.join(".gemini/antigravity/mcp_config.json"),
            None,
        ),
        (context.home.join(".gemini/config/mcp_config.json"), None),
    ];
    for project in scope
        .files
        .iter()
        .filter(|f| f.agent == "antigravity")
        .filter_map(|f| f.project_path.clone())
        .collect::<BTreeSet<_>>()
    {
        if valid_project(&project) {
            configs.push((
                Path::new(&project).join(".agents/mcp_config.json"),
                Some(project),
            ));
        }
        if configs.len() >= 502 {
            inventory.issue(
                "antigravity",
                &context.home.join(".gemini/config/mcp_config.json"),
            );
            break;
        }
    }
    for (path, project) in configs {
        if !inventory.within_budget() {
            inventory.issue("antigravity", &path);
            break;
        }
        if let Some((data, modified)) = inventory.json("antigravity", &path) {
            inventory.json_entries(
                "antigravity",
                &data["mcpServers"],
                &path,
                modified,
                if project.is_some() {
                    "project_local"
                } else {
                    "local"
                },
                project.as_deref(),
            );
        }
    }
}

pub(in crate::sessions) fn claude_projects(
    context: &SourceContext<'_>,
    scope: &mut ConfigScope<'_>,
    inventory: &mut Inventory,
) {
    let mut read_paths = BTreeSet::new();
    for cwd in scope.scanned_projects {
        if !inventory.within_budget() {
            inventory.blocked_agents.insert("claude".into());
            break;
        }
        for dir in project_dirs(context.home, cwd).into_iter().rev() {
            let project = dir.to_string_lossy().into_owned();
            let path = dir.join(".mcp.json");
            if read_paths.insert(path.clone())
                && let Some((data, modified)) = inventory.json("claude", &path)
            {
                inventory.json_entries(
                    "claude",
                    &data["mcpServers"],
                    &path,
                    modified,
                    "project",
                    Some(&project),
                );
            }
        }
    }
}

pub(in crate::sessions) fn codex_projects(
    context: &SourceContext<'_>,
    scope: &mut ConfigScope<'_>,
    inventory: &mut Inventory,
) {
    let mut read_paths = BTreeSet::new();
    for cwd in scope.scanned_projects {
        if !inventory.within_budget() {
            inventory.blocked_agents.insert("codex".into());
            break;
        }
        for dir in project_dirs(context.home, cwd).into_iter().rev() {
            let project = dir.to_string_lossy().into_owned();
            let path = dir.join(".codex/config.toml");
            let global_codex = context.home.join(".codex/config.toml");
            let same_global = path == global_codex
                || path
                    .canonicalize()
                    .ok()
                    .zip(global_codex.canonicalize().ok())
                    .is_some_and(|(a, b)| a == b);
            if !same_global && read_paths.insert(path.clone()) {
                inventory.codex(&path, Some(&project));
            }
        }
    }
}

pub(in crate::sessions) fn opencode_projects(
    context: &SourceContext<'_>,
    scope: &mut ConfigScope<'_>,
    inventory: &mut Inventory,
) {
    for cwd in std::iter::once(context.home.to_string_lossy().into_owned())
        .chain(scope.scanned_projects.iter().cloned())
    {
        if !inventory.within_budget() {
            inventory.blocked_agents.insert("opencode".into());
            break;
        }
        let context = opencode_mcp::load_guarded(
            context.home,
            &cwd,
            &|| inventory.within_budget(),
            inventory.deadline.is_some(),
        );
        inventory.versions.extend(context.versions.clone());
        if inventory.deadline.is_some() && !context.warnings.is_empty() {
            inventory.blocked_agents.insert("opencode".into());
        }
        inventory.warnings.extend(context.warnings.iter().cloned());
        for (name, server) in &context.servers {
            let project = server
                .sources
                .iter()
                .rev()
                .find(|s| s.scope == "project")
                .and_then(|s| Path::new(&s.path).parent())
                .map(|p| {
                    if p.file_name().is_some_and(|n| n == ".opencode") {
                        p.parent().unwrap_or(p)
                    } else {
                        p
                    }
                })
                .map(|p| p.to_string_lossy().into_owned());
            inventory.add(
                "opencode",
                name,
                project.as_deref(),
                server.sources.clone(),
                server.transport,
                server.enabled,
            );
        }
        scope.opencode_contexts.insert(cwd, context);
    }
}
