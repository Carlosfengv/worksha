use super::{ProjectIdentity, canonical, command, hash};
use crate::sessions::{now, sanitize_label};
use serde::Serialize;
use std::path::Path;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitFile {
    pub id: String,
    pub path: String,
    pub previous_path: Option<String>,
    pub index_status: String,
    pub worktree_status: String,
    pub attribution: String,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    pub id: String,
    pub occurred_at: String,
    pub subject: String,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitSnapshot {
    pub as_of: String,
    pub state: String,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub remote: Option<String>,
    pub git_dir: Option<String>,
    pub common_dir: Option<String>,
    pub repository_id: Option<String>,
    pub worktree_id: Option<String>,
    pub files: Vec<GitFile>,
    pub commits: Vec<Commit>,
    pub error: Option<String>,
}
pub(super) fn unavailable(message: &str) -> GitSnapshot {
    GitSnapshot {
        as_of: now(),
        state: "unavailable".into(),
        branch: None,
        head: None,
        remote: None,
        git_dir: None,
        common_dir: None,
        repository_id: None,
        worktree_id: None,
        files: vec![],
        commits: vec![],
        error: Some(message.into()),
    }
}
fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut full = vec![
        "--no-optional-locks",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.untrackedCache=false",
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "core.quotePath=false",
        "-c",
        "diff.external=",
        "-c",
        "log.showSignature=false",
        "-C",
        root.to_str().ok_or("项目路径不是有效 UTF-8")?,
    ];
    full.extend_from_slice(args);
    // Keep the child launch directory stable; Git owns repository switching via its public -C API.
    command::run("/usr/bin/git", &full, Some(Path::new("/")))
}
fn remote_safe(value: &str) -> String {
    if let Ok(mut url) = url::Url::parse(value) {
        let _ = url.set_username("");
        let _ = url.set_password(None);
        url.set_query(None);
        url.set_fragment(None);
        sanitize_label(url.as_str(), 2048)
    } else {
        sanitize_label(value, 2048)
    }
}
pub fn snapshot(project: &ProjectIdentity) -> GitSnapshot {
    let mut result = GitSnapshot {
        as_of: now(),
        state: if project.location_state != "available" {
            "unavailable"
        } else if project.git_root.is_none() {
            "not_git"
        } else {
            "ready"
        }
        .into(),
        branch: None,
        head: None,
        remote: None,
        git_dir: None,
        common_dir: None,
        repository_id: None,
        worktree_id: None,
        files: vec![],
        commits: vec![],
        error: None,
    };
    if result.state != "ready" {
        return result;
    }
    let root = Path::new(&project.path);
    let status = match git(
        root,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--branch",
            "--untracked-files=normal",
            "--ignore-submodules=all",
        ],
    ) {
        Ok(v) => v,
        Err(e) => {
            result.state = "unavailable".into();
            result.error = Some(e);
            return result;
        }
    };
    let mut rows = status.split(|b| *b == 0);
    while let Some(row) = rows.next() {
        if row.is_empty() {
            continue;
        }
        let value = String::from_utf8_lossy(row);
        if let Some(branch) = value.strip_prefix("## ") {
            result.branch = Some(sanitize_label(
                branch.split("...").next().unwrap_or(branch),
                300,
            ));
            continue;
        }
        if row.len() < 4 || !row.is_ascii() && std::str::from_utf8(row).is_err() {
            result.state = "partial".into();
            continue;
        }
        let previous = if [b'R', b'C'].contains(&row[0]) || [b'R', b'C'].contains(&row[1]) {
            rows.next()
                .map(|r| sanitize_label(&String::from_utf8_lossy(r), 2048))
        } else {
            None
        };
        let path = String::from_utf8_lossy(&row[3..]).into_owned();
        result.files.push(GitFile {
            id: hash(&path),
            path: sanitize_label(&path, 2048),
            previous_path: previous,
            index_status: (row[0] as char).to_string(),
            worktree_status: (row[1] as char).to_string(),
            attribution: "unconfirmed".into(),
        });
    }
    if let Ok(bytes) = git(
        root,
        &["rev-parse", "--absolute-git-dir", "--git-common-dir"],
    ) {
        let text = String::from_utf8_lossy(&bytes);
        let mut lines = text.lines();
        result.git_dir = lines
            .next()
            .and_then(|p| canonical(&root.join(p)))
            .map(|p| p.to_string_lossy().into());
        result.common_dir = lines
            .next()
            .and_then(|p| canonical(&root.join(p)))
            .map(|p| p.to_string_lossy().into());
        result.repository_id = result.common_dir.as_ref().map(|p| hash(p));
        result.worktree_id = result
            .git_dir
            .as_ref()
            .map(|p| hash(&format!("{}:{p}", project.path)));
    } else {
        result.state = "partial".into();
        result.error = Some("Git 身份元数据部分不可读".into());
    }
    if let Ok(bytes) = git(
        root,
        &[
            "config",
            "--local",
            "--get",
            "--default",
            "",
            "remote.origin.url",
        ],
    ) {
        let text = String::from_utf8_lossy(&bytes);
        if !text.trim().is_empty() {
            result.remote = Some(remote_safe(text.trim()));
        }
    } else {
        result.state = "partial".into();
        result.error = Some("远程仓库元数据不可读取，origin 状态未知".into());
    }
    match git(
        root,
        &[
            "log",
            "--no-decorate",
            "-30",
            "--format=%H%x00%aI%x00%s%x00",
        ],
    ) {
        Ok(bytes) => {
            let fields: Vec<_> = bytes.split(|b| *b == 0).collect();
            for fields in fields.chunks(3) {
                if fields.len() < 3 {
                    continue;
                }
                let id = String::from_utf8_lossy(fields[0]).trim().to_string();
                if id.len() != 40 && id.len() != 64 {
                    continue;
                }
                result.commits.push(Commit {
                    id,
                    occurred_at: String::from_utf8_lossy(fields[1]).into(),
                    subject: sanitize_label(&String::from_utf8_lossy(fields[2]), 500),
                });
            }
            result.head = result.commits.first().map(|c| c.id.clone());
        }
        Err(_) => {
            if !result.branch.as_ref().is_some_and(|s| {
                s.starts_with("No commits yet on ") || s.starts_with("Initial commit on ")
            }) {
                result.state = "partial".into();
                result.error = Some("提交历史不可读取，当前工作区状态仍可查看".into());
            }
        }
    }
    result
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitDiff {
    pub as_of: String,
    pub path: String,
    pub state: String,
    pub text: Option<String>,
    pub reason: Option<String>,
}
pub fn diff(project: &ProjectIdentity, file_id: &str) -> GitDiff {
    let mut result = GitDiff {
        as_of: now(),
        path: String::new(),
        state: "unavailable".into(),
        text: None,
        reason: None,
    };
    let snapshot = snapshot(project);
    let Some(file) = snapshot.files.iter().find(|f| f.id == file_id) else {
        result.reason = Some("文件已不在当前 Git 变更快照中，请刷新".into());
        return result;
    };
    result.path = file.path.clone();
    let relative = Path::new(&file.path);
    let sensitive = relative.components().any(|part| {
        let s = part.as_os_str().to_string_lossy().to_lowercase();
        s == ".git"
            || s.starts_with(".env")
            || s.ends_with(".pem")
            || s.ends_with(".key")
            || [
                "auth.json",
                "credentials.json",
                "credentials",
                "id_rsa",
                "id_ed25519",
            ]
            .contains(&s.as_str())
    });
    if sensitive
        || relative.is_absolute()
        || canonical(&Path::new(&project.path).join(relative))
            .is_none_or(|p| !p.starts_with(&project.path))
    {
        result.reason = Some("此文件可能包含凭据或位于项目范围外，不展示内容差异".into());
        return result;
    }
    if file.index_status == "?" || snapshot.head.is_none() {
        result.reason = Some("没有可用的 Git 基线；仅保留文件状态，不读取文件正文".into());
        return result;
    }
    match git(
        Path::new(&project.path),
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "HEAD",
            "--",
            &file.path,
        ],
    ) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            if text.contains("Binary files ") {
                result.state = "binary".into();
                result.reason = Some("二进制文件不显示文本差异".into());
            } else if text.is_empty() {
                result.state = "no_text_diff".into();
                result.reason =
                    Some("当前相对 HEAD 无文本净差异；暂存与工作区状态可能相互抵消".into());
            } else {
                result.state = if text.chars().count() > 64000 {
                    "partial"
                } else {
                    "ready"
                }
                .into();
                result.text = Some(sanitize_label(&text, 64000));
            }
        }
        Err(e) => result.reason = Some(e),
    }
    result
}
