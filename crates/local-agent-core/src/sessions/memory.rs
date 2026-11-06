//! Bounded discovery and on-demand previews; never writes native files or executes config.
use super::{NativeError, NativeResult, NativeSessions};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
};

const LIMIT: usize = 500;
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryFile {
    pub id: String,
    pub name: String,
    pub path: String,
    pub scope: String,
    pub project: Option<String>,
    pub size_bytes: u64,
    pub modified_at: Option<String>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryPreview {
    pub id: String,
    pub text: String,
    pub truncated: bool,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorySnapshot {
    pub agent_id: String,
    pub shared_agents: Vec<String>,
    pub files: Vec<MemoryFile>,
    pub selected: Option<MemoryPreview>,
    pub as_of: String,
    pub limited: bool,
    pub warnings: Vec<String>,
}
fn ordinary(path: &Path) -> bool {
    path.is_absolute()
        && !path.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
        && path
            .ancestors()
            .all(|p| fs::symlink_metadata(p).is_ok_and(|m| !m.file_type().is_symlink()))
}
fn preview(path: &Path, id: &str) -> NativeResult<MemoryPreview> {
    if !ordinary(path) {
        return Err(NativeError::new(
            "unauthorized",
            "记忆路径已变化或包含符号链接",
        ));
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|_| NativeError::new("unavailable", "记忆文件无法读取，请检查权限"))?;
    let meta = file
        .metadata()
        .map_err(|_| NativeError::new("unavailable", "无法读取记忆文件信息"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() > 1 {
            return Err(NativeError::new("unauthorized", "不读取共享硬链接记忆文件"));
        }
    }
    if !meta.is_file() {
        return Err(NativeError::new("unsupported", "记忆来源不是普通文件"));
    }
    let mut bytes = Vec::new();
    file.take(65536)
        .read_to_end(&mut bytes)
        .map_err(|_| NativeError::new("unavailable", "读取记忆内容失败"))?;
    let (text, truncated) = super::parser::safe_text(&String::from_utf8_lossy(&bytes), 16384);
    Ok(MemoryPreview {
        id: id.into(),
        text,
        truncated: truncated || meta.len() > bytes.len() as u64,
    })
}
struct Discovery {
    candidates: BTreeMap<PathBuf, (String, Option<String>)>,
    limited: bool,
    warnings: Vec<String>,
    budget: usize,
}
impl Discovery {
    fn file(&mut self, path: PathBuf, scope: &str, project: Option<&str>) {
        if self.candidates.len() >= LIMIT {
            self.limited = true;
            return;
        }
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_file() && ordinary(&path) => {
                self.candidates
                    .entry(path)
                    .or_insert_with(|| (scope.into(), project.map(str::to_owned)));
            }
            Ok(_) => self
                .warnings
                .push("部分记忆路径包含链接或不是普通文件，未读取。".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => self
                .warnings
                .push("部分记忆文件不可访问，列表可能不完整。".into()),
        }
    }
    fn directory(&mut self, path: &Path, scope: &str, project: Option<&str>, depth: usize) {
        if self.budget == 0 {
            self.limited = true;
            return;
        }
        if !path.exists() {
            return;
        }
        if !ordinary(path) {
            self.warnings.push("跳过含符号链接的记忆目录。".into());
            return;
        }
        let entries = match fs::read_dir(path) {
            Ok(v) => v,
            Err(_) => {
                self.warnings
                    .push("记忆目录不可访问，列表可能不完整。".into());
                return;
            }
        };
        let mut paths: Vec<_> = entries
            .take(1001)
            .filter_map(Result::ok)
            .map(|e| e.path())
            .collect();
        if paths.len() > 1000 {
            self.limited = true;
            paths.truncate(1000);
        }
        paths.sort();
        for child in paths {
            if self.budget == 0 {
                self.limited = true;
                break;
            }
            self.budget -= 1;
            if self.candidates.len() >= LIMIT {
                self.limited = true;
                break;
            }
            if child.extension().is_some_and(|x| x == "md") {
                self.file(child, scope, project);
            } else if depth > 0 && child.is_dir() {
                self.directory(&child, scope, project, depth - 1);
            }
        }
    }
}
impl NativeSessions {
    pub fn memory(
        &self,
        agent: &str,
        selected: Option<&str>,
        hidden: &HashSet<String>,
    ) -> NativeResult<MemorySnapshot> {
        let family = self
            .directory()
            .products()
            .iter()
            .find(|p| p.id == agent)
            .map(|p| p.family.as_str())
            .ok_or_else(|| NativeError::new("invalid_input", "请选择明确的 Agent"))?;
        let (projects, revision) = {
            let state = self.lock()?;
            if !state
                .sources
                .iter()
                .any(|s| s.agent_id == family && s.enabled)
            {
                return Err(NativeError::new(
                    "unauthorized",
                    "先在连接与观测中授权此 Agent 的只读来源",
                ));
            }
            let mut projects = std::collections::BTreeSet::new();
            for f in super::unique_files(&state) {
                if f.agent != family
                    || !self.directory().matches(
                        Some(agent),
                        &f.agent,
                        f.summary.agent_product_id.as_deref(),
                    ) && !f.summary.agent_products.iter().any(|p| p == agent)
                {
                    continue;
                }
                for path in f
                    .summary
                    .project_path
                    .iter()
                    .chain(f.summary.project_root.iter())
                    .filter(|p| !hidden.contains(*p))
                {
                    projects.insert(path.clone());
                }
            }
            (
                projects,
                state
                    .sources
                    .iter()
                    .find(|s| s.agent_id == family)
                    .unwrap()
                    .authorization_revision,
            )
        };
        self.with_source_context(family, Some(revision), true, |source, context| {
            source.memory(context, agent, selected, &projects)
        })
    }
}

pub(super) fn read(
    context: &super::source_plugins::SourceContext<'_>,
    agent: &str,
    family: &str,
    selected: Option<&str>,
    projects: &std::collections::BTreeSet<String>,
) -> NativeResult<MemorySnapshot> {
    context.check()?;
    let mut d = Discovery {
        candidates: BTreeMap::new(),
        limited: false,
        warnings: vec![],
        budget: 2000,
    };
    let home = context.home;
    match family {
        "codex" => {
            d.file(home.join(".codex/AGENTS.md"), "全局规则", None);
            d.file(home.join(".codex/AGENTS.override.md"), "全局覆盖规则", None);
            d.directory(&home.join(".codex/memories"), "本机记忆", None, 2);
        }
        "claude" => {
            d.file(home.join(".claude/CLAUDE.md"), "全局规则", None);
            d.directory(&home.join(".claude/rules"), "全局规则", None, 2);
        }
        _ => {
            d.file(home.join(".config/opencode/AGENTS.md"), "全局规则", None);
            d.file(home.join(".claude/CLAUDE.md"), "兼容规则候选", None);
        }
    }
    for path in projects.iter().take(200) {
        let root = Path::new(path);
        if !root.is_absolute() {
            continue;
        }
        let names: &[&str] = match family {
            "claude" => &["CLAUDE.md", "CLAUDE.local.md", ".claude/CLAUDE.md"],
            "codex" => &["AGENTS.md", "AGENTS.override.md"],
            _ => &["AGENTS.md", "CLAUDE.md"],
        };
        for name in names {
            d.file(root.join(name), "项目规则", Some(path));
        }
        if family == "claude" {
            d.directory(&root.join(".claude/rules"), "项目规则", Some(path), 2);
        }
    }
    if family == "claude" {
        // The encoded project directory is a storage key, not reliable project attribution.
        let root = home.join(".claude/projects");
        if ordinary(&root)
            && let Ok(entries) = fs::read_dir(root)
        {
            for (index, p) in entries.take(201).flatten().enumerate() {
                if index == 200 {
                    d.limited = true;
                    break;
                }
                d.directory(
                    &p.path().join("memory"),
                    "项目自动记忆 · 归属待确认",
                    None,
                    1,
                );
            }
        }
    }
    if projects.len() > 200 {
        d.limited = true;
    }
    let mut files = vec![];
    let mut content = None;
    for (path, (scope, project)) in d.candidates {
        let id = format!("{:x}", Sha256::digest(path.to_string_lossy().as_bytes()));
        if selected == Some(id.as_str()) {
            content = Some(preview(&path, &id)?);
        }
        if let Ok(meta) = fs::metadata(&path) {
            files.push(MemoryFile {
                id,
                name: path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                path: super::sanitize_label(&path.to_string_lossy(), 4096),
                scope,
                project: project.map(|s| super::sanitize_label(&s, 4096)),
                size_bytes: meta.len(),
                modified_at: meta
                    .modified()
                    .ok()
                    .map(|v| chrono::DateTime::<chrono::Utc>::from(v).to_rfc3339()),
            });
        }
    }
    if selected.is_some() && content.is_none() {
        return Err(NativeError::new(
            "not_found",
            "记忆文件已变化或不在当前可读范围",
        ));
    }
    d.warnings.sort();
    d.warnings.dedup();
    d.warnings.push("只读已识别的默认目录与已关联项目；自定义启动目录、远程或托管记忆未确认。发现文件不代表此客户端或会话已加载。".into());
    Ok(MemorySnapshot {
        agent_id: agent.into(),
        shared_agents: context
            .directory
            .products()
            .iter()
            .filter(|p| p.family == family)
            .map(|p| p.id.clone())
            .collect(),
        files,
        selected: content,
        as_of: super::now(),
        limited: d.limited,
        warnings: d.warnings,
    })
}
