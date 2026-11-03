//! Read only the standard local configuration layers. Never resolve variables,
//! fetch remote config, load plugins, or start an MCP process for attribution.
use super::parser::safe_text;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
static READER_WORKER: OnceLock<PathBuf> = OnceLock::new();
static UNAVAILABLE: Mutex<Vec<(PathBuf, Instant)>> = Mutex::new(Vec::new());
pub fn install_reader_worker(path: PathBuf) {
    let _ = READER_WORKER.set(path);
}
pub fn read_config_worker(path: &Path) -> super::NativeResult<Option<(String, Option<String>)>> {
    if !path.is_absolute()
        || !matches!(
            path.file_name().and_then(|n| n.to_str()),
            Some(
                ".claude.json"
                    | ".mcp.json"
                    | "opencode.json"
                    | "opencode.jsonc"
                    | "config.json"
                    | "config.toml"
                    | "mcp_config.json"
            )
        )
    {
        return Err(super::NativeError::new("invalid_input", "MCP 配置路径无效"));
    }
    read_text(path).map_err(|_| super::NativeError::new("unavailable", "MCP 配置不可读取"))
}
fn protected(path: &Path) -> bool {
    #[cfg(target_os = "macos")]
    if let Some(home) = dirs::home_dir() {
        return [
            "Downloads",
            "Documents",
            "Desktop",
            "Library/Mobile Documents",
            "Library/CloudStorage",
        ]
        .iter()
        .any(|p| path.starts_with(home.join(p)))
            || path.starts_with("/Volumes");
    }
    false
}
fn worker_read(exe: &Path, path: &Path) -> Result<Option<(String, Option<String>)>, ()> {
    {
        let mut unavailable = UNAVAILABLE.lock().map_err(|_| ())?;
        unavailable.retain(|(_, at)| at.elapsed() < Duration::from_secs(30));
        if unavailable.iter().any(|(p, _)| p == path) {
            return Err(());
        }
    }
    let mut child = Command::new(exe)
        .arg("--read-mcp-config")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| ())?;
    let stdout = child.stdout.take().ok_or(())?;
    let reader = std::thread::spawn(move || {
        let mut bytes = vec![];
        stdout
            .take(6 * 1024 * 1024 + 1024)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
        if start.elapsed() > Duration::from_millis(500) {
            break None;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let Some(status) = status else {
        let _ = child.kill();
        let _ = child.wait();
        let _ = reader.join();
        if let Ok(mut unavailable) = UNAVAILABLE.lock() {
            if unavailable.len() >= 256 {
                unavailable.remove(0);
            }
            unavailable.push((path.into(), Instant::now()));
        }
        return Err(());
    };
    let bytes = reader.join().map_err(|_| ())?.map_err(|_| ())?;
    if !status.success() || bytes.len() >= 6 * 1024 * 1024 + 1024 {
        return Err(());
    }
    serde_json::from_slice(&bytes).map_err(|_| ())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSource {
    pub path: String,
    pub scope: &'static str,
    pub modified_at: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSource {
    pub status: &'static str,
    pub tool_name: Option<String>,
    pub tool_name_quality: &'static str,
    pub transport: Option<&'static str>,
    pub configured_enabled: Option<bool>,
    pub sources: Vec<ConfigSource>,
    pub reason: String,
}
#[derive(Clone, Default)]
pub(super) struct Server {
    pub transport: Option<&'static str>,
    pub enabled: Option<bool>,
    pub sources: Vec<ConfigSource>,
}
#[derive(Clone, Default)]
pub(super) struct Context {
    pub servers: BTreeMap<String, Server>,
    pub fingerprint: String,
    pub warnings: Vec<String>,
    pub versions: BTreeMap<PathBuf, String>,
}

// OpenCode replaces each non-ASCII UTF-16 code unit, retaining '_' and '-'.
fn sanitize(value: &str) -> String {
    value
        .encode_utf16()
        .map(|c| match c {
            45 | 95 | 48..=57 | 65..=90 | 97..=122 => char::from_u32(c as u32).unwrap(),
            _ => '_',
        })
        .collect()
}
fn label(value: &str) -> Option<String> {
    (!value.is_empty() && value.len() <= 512).then(|| safe_text(value, 512).0)
}
impl Context {
    pub fn resolve(&self, name: &str, native: &Value) -> (Option<String>, ToolSource) {
        let mut result = ToolSource {
            status: "unknown",
            tool_name: None,
            tool_name_quality: "unknown",
            transport: None,
            configured_enabled: None,
            sources: vec![],
            reason: "未找到可唯一关联的本机或项目 MCP 配置；可能是自定义工具或历史配置已变更。"
                .into(),
        };
        let marked = name
            .strip_prefix("mcp__")
            .or_else(|| name.strip_prefix("functions.mcp__"))
            .and_then(|tail| tail.split_once("__"))
            .filter(|(server, tool)| !server.is_empty() && !tool.is_empty());
        if let Some(server) = native["server"].as_str().and_then(label) {
            result.status = "native";
            result.tool_name = native["tool"].as_str().and_then(label);
            result.tool_name_quality = if result.tool_name.is_some() {
                "native"
            } else {
                "unknown"
            };
            if result.tool_name.is_none()
                && let Some((s, tool)) = marked
                && s == server
            {
                result.tool_name = label(tool);
                result.tool_name_quality = "native";
            }
            result.reason = "原生调用提供 MCP 服务标记；不据此推断当前服务已连接。".into();
            return (Some(server), result);
        }
        if let Some((server, tool)) = marked
            && let (Some(server), Some(tool)) = (label(server), label(tool))
        {
            result.status = "native";
            result.tool_name = Some(tool);
            result.tool_name_quality = "native";
            result.reason = "原生调用使用明确的 mcp__服务__工具 命名；未验证当前连接。".into();
            return (Some(server), result);
        }
        if [
            "bash",
            "read",
            "write",
            "edit",
            "multiedit",
            "apply_patch",
            "glob",
            "grep",
            "question",
            "task",
            "todowrite",
            "todoread",
            "skill",
            "webfetch",
            "websearch",
            "codesearch",
            "lsp",
            "batch",
            "invalid",
            "plan_enter",
            "plan_exit",
        ]
        .contains(&name)
        {
            result.status = "builtin";
            result.tool_name = label(name);
            result.tool_name_quality = "native";
            result.reason = "OpenCode 原生工具或 Skill；不按名称前缀归类为 MCP。".into();
            return (None, result);
        }
        if !self.warnings.is_empty() {
            result.reason = "配置读取范围不完整或存在无法解析的配置，MCP 来源未知。".into();
            return (None, result);
        }
        let candidates: Vec<_> = self
            .servers
            .iter()
            .filter_map(|(server, config)| {
                name.strip_prefix(&format!("{}_", sanitize(server)))
                    .filter(|tool| !tool.is_empty())
                    .map(|tool| (server, config, tool))
            })
            .collect();
        if candidates.len() > 1 {
            result.reason =
                "多个 MCP 服务前缀可匹配此工具（含规范化名称冲突）；无法唯一确认来源。".into();
        } else if let Some((server, config, tool)) = candidates.first() {
            if config.transport.is_none() {
                result.reason = "匹配到服务名，但配置未提供可识别的 MCP 类型，来源仍未知。".into();
                return (None, result);
            }
            result.status = "configuration";
            result.tool_name = label(tool);
            result.tool_name_quality = "normalized";
            result.transport = config.transport;
            result.configured_enabled = config.enabled;
            result.sources = config.sources.clone();
            result.reason = "按当前配置与 OpenCode 的服务名前缀规则唯一关联；并非调用发生时的配置快照，工具原始名称及当时连接状态未确认。".into();
            return (label(server), result);
        }
        (None, result)
    }
}

pub(super) fn global_paths(home: &Path) -> Vec<PathBuf> {
    ["config.json", "opencode.json", "opencode.jsonc"]
        .map(|name| home.join(".config/opencode").join(name))
        .into()
}

// Resolve only known filenames and existing directories, bounded by the nearest
// Git root, otherwise home (or cwd for projects outside home). No directory glob.
pub(super) fn load(home: &Path, directory: &str) -> Context {
    load_guarded(home, directory, &|| true, false)
}
pub(super) fn load_guarded(
    home: &Path,
    directory: &str,
    proceed: &dyn Fn() -> bool,
    observe_versions: bool,
) -> Context {
    let mut result = Context::default();
    let mut layers: Vec<_> = global_paths(home)
        .into_iter()
        .map(|p| (p, "local"))
        .collect();
    let cwd = Path::new(directory);
    let mut dirs = vec![];
    if cwd.is_absolute() && !cwd.components().any(|c| matches!(c, Component::ParentDir)) {
        let mut git = false;
        for dir in cwd.ancestors().take(33) {
            if !proceed() {
                result.warnings.push("MCP 配置读取预算已用尽。".into());
                break;
            }
            if dirs.len() == 32 {
                result
                    .warnings
                    .push("项目 MCP 配置目录超过 32 层，来源关联保留未知。".into());
                break;
            }
            dirs.push(dir.to_path_buf());
            git = dir.join(".git").symlink_metadata().is_ok();
            if git || dir == home || dir.parent().is_none() {
                break;
            }
        }
        if !git && !cwd.starts_with(home) {
            dirs.truncate(1);
        }
    } else {
        result
            .warnings
            .push("原生项目位置不是可识别的绝对路径，MCP 来源关联保留未知。".into());
    }
    for dir in dirs.iter().rev() {
        for name in ["opencode.json", "opencode.jsonc"] {
            layers.push((dir.join(name), "project"));
        }
    }
    for dir in &dirs {
        for name in ["opencode.json", "opencode.jsonc"] {
            layers.push((
                dir.join(".opencode").join(name),
                if dir == home { "local" } else { "project" },
            ));
        }
    }
    if !dirs.iter().any(|d| d == home) {
        for name in ["opencode.json", "opencode.jsonc"] {
            layers.push((home.join(".opencode").join(name), "local"));
        }
    }
    let mut hash = Sha256::new();
    for (path, scope) in layers {
        if !proceed() {
            result.warnings.push("MCP 配置读取预算已用尽。".into());
            break;
        }
        hash.update(path.to_string_lossy().as_bytes());
        let observed = if observe_versions {
            read_observed(&path).map(|value| {
                result.versions.insert(
                    path.clone(),
                    value
                        .as_ref()
                        .map(|(_, _, v)| v.clone())
                        .unwrap_or_else(|| "missing".into()),
                );
                value.map(|(data, modified, _)| (data, modified))
            })
        } else {
            read(&path)
        };
        match observed {
            Ok(None) => {
                hash.update(b"missing");
            }
            Err(()) => {
                hash.update(b"unreadable");
                result.warnings.push(format!(
                    "MCP 配置未读取（损坏、超限、不可访问或链接）：{}",
                    safe_text(&path.to_string_lossy(), 2048).0
                ));
            }
            Ok(Some((value, modified))) => {
                // Hash only the MCP projection, never retain provider credentials.
                hash.update(value["mcp"].to_string());
                hash.update(modified.as_deref().unwrap_or("unknown"));
                let Some(mcp) = value.get("mcp") else {
                    continue;
                };
                let Some(entries) = mcp.as_object() else {
                    result
                        .warnings
                        .push("存在不受支持的 MCP 配置结构，来源关联保留未知。".into());
                    continue;
                };
                if entries.len() > 256 {
                    result
                        .warnings
                        .push("MCP 服务超过 256 项，来源关联保留未知。".into());
                    continue;
                }
                for (name, entry) in entries {
                    if label(name).is_none() || !entry.is_object() {
                        result
                            .warnings
                            .push("存在无效 MCP 服务配置，来源关联保留未知。".into());
                        continue;
                    }
                    let config = result.servers.entry(name.clone()).or_default();
                    if let Some(kind) = entry.get("type") {
                        config.transport = match kind.as_str() {
                            Some("local") => Some("stdio"),
                            Some("remote") => Some("remote"),
                            _ => None,
                        };
                    }
                    if let Some(enabled) = entry.get("enabled") {
                        config.enabled = enabled.as_bool();
                        if config.enabled.is_none() {
                            result
                                .warnings
                                .push("存在无法识别的 MCP 启用状态，来源关联保留未知。".into());
                        }
                    }
                    config.sources.push(ConfigSource {
                        path: safe_text(&path.to_string_lossy(), 2048).0,
                        scope,
                        modified_at: modified.clone(),
                    });
                }
            }
        }
    }
    hash.update(result.warnings.join("\n"));
    result.fingerprint = format!("{:x}", hash.finalize());
    result
}

pub(super) type Observation<T> = Result<Option<(T, Option<String>, String)>, ()>;
fn configuration_version(path: &Path) -> Result<String, ()> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok("missing".into()),
        _ => return Err(()),
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() != 1 {
            return Err(());
        }
        Ok(format!(
            "{}:{}:{}:{}:{}:{}:{}",
            meta.dev(),
            meta.ino(),
            meta.len(),
            meta.mtime(),
            meta.mtime_nsec(),
            meta.ctime(),
            meta.ctime_nsec()
        ))
    }
    #[cfg(not(unix))]
    {
        Ok(format!(
            "{}:{:?}",
            meta.len(),
            meta.modified().map_err(|_| ())?
        ))
    }
}
pub(super) fn read_text_observed(path: &Path) -> Observation<String> {
    let before = configuration_version(path)?;
    let value = read_text(path)?;
    if configuration_version(path)? != before {
        return Err(());
    }
    Ok(value.map(|(raw, modified)| (raw, modified, before)))
}
pub(super) fn read_observed(path: &Path) -> Observation<Value> {
    read_text_observed(path)?
        .map(|(raw, modified, version)| {
            let value = jsonc(&raw)?;
            if !value.is_object() {
                return Err(());
            }
            Ok((value, modified, version))
        })
        .transpose()
}
pub(super) fn read(path: &Path) -> Result<Option<(Value, Option<String>)>, ()> {
    read_text(path)?
        .map(|(raw, modified)| {
            let value = jsonc(&raw)?;
            if !value.is_object() {
                return Err(());
            }
            Ok((value, modified))
        })
        .transpose()
}
pub(super) fn read_text(path: &Path) -> Result<Option<(String, Option<String>)>, ()> {
    // /var is a system alias on macOS. Config components themselves must not
    // contain symlinks. Canonicalize the trusted system prefix only.
    let checked = if let Ok(relative) = path.strip_prefix("/var") {
        Path::new("/private/var").join(relative)
    } else {
        path.to_path_buf()
    };
    for ancestor in checked.ancestors() {
        match ancestor.symlink_metadata() {
            Ok(meta) if meta.file_type().is_symlink() => return Err(()),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(()),
        }
    }
    let meta = match checked.symlink_metadata() {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(()),
    };
    if !meta.is_file() || meta.len() > 1024 * 1024 {
        return Err(());
    }
    if protected(&checked)
        && let Some(exe) = READER_WORKER.get()
    {
        return worker_read(exe, &checked);
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        if meta.nlink() != 1 {
            return Err(());
        }
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(&checked).map_err(|_| ())?;
    let after = file.metadata().map_err(|_| ())?;
    if !after.is_file() || after.len() > 1024 * 1024 {
        return Err(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if after.dev() != meta.dev() || after.ino() != meta.ino() || after.nlink() != 1 {
            return Err(());
        }
    }
    let mut raw = String::new();
    file.take(1024 * 1024 + 1)
        .read_to_string(&mut raw)
        .map_err(|_| ())?;
    if raw.len() > 1024 * 1024 {
        return Err(());
    }
    let modified = after
        .modified()
        .ok()
        .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339());
    Ok(Some((raw, modified)))
}

// Only JSON + comments + trailing commas. Keep string bytes intact, including
// URLs and escaped quotes; serde_json still validates the complete document.
fn jsonc(raw: &str) -> Result<Value, ()> {
    let mut bytes = raw.trim_start_matches('\u{feff}').as_bytes().to_vec();
    let mut i = 0;
    let mut quoted = false;
    while i < bytes.len() {
        if quoted {
            if bytes[i] == b'\\' {
                i += 2;
                continue;
            }
            if bytes[i] == b'"' {
                quoted = false;
            }
        } else if bytes[i] == b'"' {
            quoted = true;
        } else if bytes.get(i..i + 2) == Some(b"//") {
            while i < bytes.len() && bytes[i] != b'\n' {
                bytes[i] = b' ';
                i += 1;
            }
            continue;
        } else if bytes.get(i..i + 2) == Some(b"/*") {
            bytes[i] = b' ';
            bytes[i + 1] = b' ';
            i += 2;
            while i + 1 < bytes.len() && bytes.get(i..i + 2) != Some(b"*/") {
                bytes[i] = b' ';
                i += 1;
            }
            if i + 1 >= bytes.len() {
                return Err(());
            }
            bytes[i] = b' ';
            bytes[i + 1] = b' ';
            i += 2;
            continue;
        }
        i += 1;
    }
    i = 0;
    quoted = false;
    while i < bytes.len() {
        if quoted {
            if bytes[i] == b'\\' {
                i += 2;
                continue;
            }
            if bytes[i] == b'"' {
                quoted = false;
            }
        } else if bytes[i] == b'"' {
            quoted = true;
        } else if bytes[i] == b',' {
            let next = bytes[i + 1..].iter().find(|b| !b.is_ascii_whitespace());
            if matches!(next, Some(b'}' | b']')) {
                bytes[i] = b' ';
            }
        }
        i += 1;
    }
    serde_json::from_slice(&bytes).map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn write(path: &Path, data: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, data).unwrap();
    }
    #[test]
    fn jsonc_strings_comments_trailing_commas_and_invalid_documents() {
        let data = jsonc(
            r#"{ // 注释
          "url": "https://example.test/a/*b*/", "quote":"a\"//b", /* block */
          "items": [1,2,],
        }"#,
        )
        .unwrap();
        assert_eq!(data["url"], "https://example.test/a/*b*/");
        assert_eq!(data["quote"], "a\"//b");
        for raw in [
            r#"{"a":1 /*"#,
            r#"{"a":"unterminated}"#,
            "[1,,]",
            "{} garbage",
        ] {
            assert!(jsonc(raw).is_err(), "{raw}");
        }
        assert_eq!(sanitize("a.b/😀"), "a_b___");
    }
    #[test]
    fn configuration_layers_preserve_sources_and_never_return_credentials() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let project = home.join("repo");
        let cwd = project.join("nested");
        fs::create_dir_all(project.join(".git")).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        write(
            &home.join(".config/opencode/config.json"),
            r#"{"mcp":{"team":{"type":"local","command":["never-execute-secret"]}}}"#,
        );
        write(
            &home.join(".config/opencode/opencode.json"),
            r#"{"mcp":{"team":{"enabled":true}},"provider":{"key":"provider-secret"}}"#,
        );
        write(
            &project.join("opencode.jsonc"),
            r#"{// project
          "mcp":{"team":{"type":"remote","url":"https://secret.test","headers":{"Authorization":"secret-header"}},"project-only":{"type":"local"}},}"#,
        );
        write(
            &cwd.join(".opencode/opencode.json"),
            r#"{"mcp":{"team":{"enabled":false}}}"#,
        );
        // A config above the Git root is outside this project's layers.
        write(
            &home.join("opencode.json"),
            r#"{"mcp":{"unrelated":{"type":"local"}}}"#,
        );
        let context = load(home, cwd.to_str().unwrap());
        assert!(context.warnings.is_empty(), "{:?}", context.warnings);
        let (server, source) = context.resolve("team_get_contract", &Value::Null);
        assert_eq!(server.as_deref(), Some("team"));
        assert_eq!(source.status, "configuration");
        assert_eq!(source.tool_name.as_deref(), Some("get_contract"));
        assert_eq!(source.tool_name_quality, "normalized");
        assert_eq!(source.transport, Some("remote"));
        assert_eq!(source.configured_enabled, Some(false));
        assert_eq!(source.sources.len(), 4);
        assert_eq!(source.sources[0].scope, "local");
        assert_eq!(source.sources[3].scope, "project");
        assert!(source.sources.iter().all(|s| s.modified_at.is_some()));
        let serialized = serde_json::to_string(&source).unwrap();
        for secret in [
            "never-execute-secret",
            "provider-secret",
            "secret-header",
            "secret.test",
        ] {
            assert!(!serialized.contains(secret));
        }
        assert_eq!(
            context
                .resolve("project-only_read", &Value::Null)
                .0
                .as_deref(),
            Some("project-only")
        );
        assert_eq!(
            context.resolve("unrelated_read", &Value::Null).1.status,
            "unknown"
        );
        assert_eq!(context.resolve("read", &Value::Null).1.status, "builtin");
        assert_eq!(context.resolve("skill", &Value::Null).1.status, "builtin");
        assert_eq!(
            context
                .resolve("read", &json!({"server":"native","tool":"read"}))
                .1
                .status,
            "native"
        );
    }
    #[test]
    fn ambiguous_prefixes_normalization_and_incomplete_configs_stay_unknown() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let file = home.join(".config/opencode/opencode.json");
        write(
            &file,
            r#"{"mcp":{"a":{"type":"local"},"a_b":{"type":"local"},"x.y":{"type":"remote"},"x_y":{"type":"local"},"empty":{"enabled":true}}}"#,
        );
        let context = load(home, home.to_str().unwrap());
        for name in ["a_b_read", "x_y_read", "empty_read", "no_match_read"] {
            let (server, source) = context.resolve(name, &Value::Null);
            assert!(server.is_none(), "{name}");
            assert_eq!(source.status, "unknown");
        }
        write(&home.join("opencode.jsonc"), "{broken");
        let changed = load(home, home.to_str().unwrap());
        assert_ne!(context.fingerprint, changed.fingerprint);
        assert!(!changed.warnings.is_empty());
        assert_eq!(changed.resolve("a_read", &Value::Null).1.status, "unknown");
        assert_eq!(
            changed.resolve("mcp__native__get", &Value::Null).1.status,
            "native"
        );
        assert_eq!(
            changed
                .resolve("mcp__native__get", &Value::Null)
                .0
                .as_deref(),
            Some("native")
        );
    }
    #[test]
    fn symlinks_hardlinks_and_large_files_are_not_loaded() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let file = home.join(".config/opencode/opencode.json");
        let target = home.join("target");
        write(&target, r#"{"mcp":{"secret":{"type":"local"}}}"#);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&target, &file).unwrap();
            assert!(!load(home, home.to_str().unwrap()).warnings.is_empty());
            fs::remove_file(&file).unwrap();
            fs::hard_link(&target, &file).unwrap();
            assert!(!load(home, home.to_str().unwrap()).warnings.is_empty());
            fs::remove_file(&file).unwrap();
        }
        fs::write(&file, vec![b' '; 1024 * 1024 + 1]).unwrap();
        assert!(!load(home, home.to_str().unwrap()).warnings.is_empty());
    }

    #[test]
    fn config_worker_validates_paths_without_loading_arbitrary_files() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("opencode.jsonc");
        write(&file, "{\"mcp\":{}}");
        assert_eq!(
            read_config_worker(&file).unwrap().unwrap().0,
            "{\"mcp\":{}}"
        );
        assert!(read_config_worker(Path::new("opencode.jsonc")).is_err());
        assert!(read_config_worker(&temp.path().join("credentials.json")).is_err());
        let antigravity = temp.path().join(".agents/mcp_config.json");
        write(&antigravity, "{\"mcpServers\":{}}");
        assert_eq!(
            read_config_worker(&antigravity).unwrap().unwrap().0,
            "{\"mcpServers\":{}}"
        );
        assert!(
            read_config_worker(&temp.path().join("config.toml"))
                .unwrap()
                .is_none()
        );
    }

    #[cfg(unix)]
    #[test]
    fn config_worker_timeout_is_bounded_and_unavailable_result_is_cached() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let exe = temp.path().join("fixture-reader");
        let path = temp.path().join("opencode.json");
        // Dedicated test helper, never a command taken from an Agent config.
        write(&exe, "#!/bin/sh\nexec /bin/sleep 5\n");
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o700)).unwrap();
        let started = Instant::now();
        assert!(worker_read(&exe, &path).is_err());
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(UNAVAILABLE.lock().unwrap().iter().any(|(p, _)| p == &path));
        // Use distinct executables: rewriting an already executed script can
        // delay macOS exec long enough to hit the production 500ms deadline.
        // The timeout cache belongs to the config path, even with a new helper.
        let success = temp.path().join("success-reader");
        write(&success, "#!/bin/sh\nprintf 'null'\n");
        fs::set_permissions(&success, fs::Permissions::from_mode(0o700)).unwrap();
        // macOS may inspect a newly created executable on its first launch.
        // Warm only the JSON fixtures; the sleep helper still tests the real
        // 500ms deadline and the unavailable-path cache remains enforced.
        let warmed = Command::new(&success).output().unwrap();
        assert!(warmed.status.success());
        assert_eq!(warmed.stdout, b"null");
        assert!(worker_read(&success, &path).is_err());
        let readable = temp.path().join("config.toml");
        assert!(worker_read(&success, &readable).unwrap().is_none());
        let invalid = temp.path().join("invalid-reader");
        write(&invalid, "#!/bin/sh\nprintf invalid\n");
        fs::set_permissions(&invalid, fs::Permissions::from_mode(0o700)).unwrap();
        let warmed = Command::new(&invalid).output().unwrap();
        assert!(warmed.status.success());
        assert_eq!(warmed.stdout, b"invalid");
        assert!(worker_read(&invalid, &readable).is_err());
    }
}
