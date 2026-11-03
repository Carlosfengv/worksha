//! OpenCode 1.x SQLite history. Read transactions include uncheckpointed WAL data;
//! Credentials, snapshots and external tool-output files are never opened.
//! Explicit session-linked errors are supplemented from the bounded current diagnostic log.
use super::{IndexedFile, NativeError, NativeResult, opencode_mcp, parser, safe_path};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::Duration,
};

const MAX_RECORD: usize = 8 * 1024 * 1024;
const MAX_SESSION: u64 = 64 * 1024 * 1024;
const MAX_ROWS: usize = 100_000;
const MAX_SESSIONS: usize = 5000;

pub(super) fn path(home: &Path) -> PathBuf {
    home.join(".local/share/opencode/opencode.db")
}
fn unavailable() -> NativeError {
    NativeError::new(
        "unavailable",
        "OpenCode 会话数据库无法读取、正忙或结构不受支持；请检查来源后重试",
    )
}
pub(super) fn open(home: &Path) -> NativeResult<Connection> {
    let path = path(home);
    if !safe_path(home, &path) {
        return Err(NativeError::new(
            "unauthorized",
            "OpenCode 数据库不存在、超出授权范围或包含符号链接",
        ));
    }
    for suffix in ["-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{suffix}", path.display()));
        if sidecar.symlink_metadata().is_ok() && !safe_path(home, &sidecar) {
            return Err(NativeError::new(
                "unauthorized",
                "OpenCode 数据库附属文件包含符号链接",
            ));
        }
    }
    let meta = path.symlink_metadata().map_err(|_| unavailable())?;
    if !meta.is_file() || meta.len() > 2 * 1024 * 1024 * 1024 {
        return Err(NativeError::new(
            "unsupported",
            "OpenCode 数据库不是普通文件或超过 2 GiB 读取限制",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() > 1 {
            return Err(NativeError::new(
                "unsupported",
                "不读取硬链接 OpenCode 数据库",
            ));
        }
    }
    // macOS temporary homes may start at /var -> /private/var. Resolve the
    // already-checked authorized root before SQLite's NOFOLLOW open.
    let canonical = path.canonicalize().map_err(|_| unavailable())?;
    if !canonical.starts_with(home.canonicalize().map_err(|_| unavailable())?) {
        return Err(NativeError::new(
            "unauthorized",
            "OpenCode 数据库超出授权来源",
        ));
    }
    let connection = Connection::open_with_flags(
        canonical,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|_| unavailable())?;
    connection
        .busy_timeout(Duration::from_millis(500))
        .map_err(|_| unavailable())?;
    connection
        .execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF; BEGIN DEFERRED;")
        .map_err(|_| unavailable())?;
    for table in ["session", "message", "part"] {
        let table_type: String = connection
            .query_row(
                "SELECT type FROM sqlite_master WHERE name=?1",
                [table],
                |row| row.get(0),
            )
            .map_err(|_| unavailable())?;
        if table_type != "table" {
            return Err(unavailable());
        }
    }
    Ok(connection)
}

pub(super) struct Record {
    pub id: String,
    pub parent: String,
    pub created: i64,
    pub data: Value,
}
pub(super) struct History {
    pub info: Value,
    pub messages: Vec<Record>,
    pub parts: Vec<Record>,
    pub bytes: u64,
    pub fingerprint: String,
    pub warnings: Vec<String>,
    // Missing MCP configuration and bounded diagnostics do not make otherwise
    // complete transcript rows unreadable. Keep their coverage notes separate.
    pub coverage_warnings: Vec<String>,
    pub mcp: opencode_mcp::Context,
    pub diagnostics: Vec<super::opencode_log::Diagnostic>,
}
pub(super) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
fn sessions(connection: &Connection) -> NativeResult<Vec<Value>> {
    let mut statement = connection.prepare("SELECT id,title,directory,version,time_created,time_updated FROM session ORDER BY time_updated DESC,id LIMIT 5001").map_err(|_| unavailable())?;
    statement
        .query_map([], |row| {
            Ok(json!({
                "id":row.get::<_,String>(0)?,"title":row.get::<_,String>(1)?,
                "directory":row.get::<_,String>(2)?,"version":row.get::<_,String>(3)?,
                "created":row.get::<_,i64>(4)?,"updated":row.get::<_,i64>(5)?
            }))
        })
        .map_err(|_| unavailable())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| unavailable())
}
// Check each row's native update marker and size, including older rows edited in
// place. Unchanged sessions keep only the existing in-memory metadata index.
fn revision(connection: &Connection, info: &Value) -> NativeResult<(String, u64)> {
    let mut hash = Sha256::new();
    hash.update(info.to_string());
    let mut bytes = info.to_string().len() as u64;
    for table in ["message", "part"] {
        hash.update(table);
        let sql = format!(
            "SELECT id,time_created,time_updated,length(CAST(data AS BLOB)) FROM {table} WHERE session_id=?1 ORDER BY id LIMIT 100001"
        );
        let mut statement = connection.prepare(&sql).map_err(|_| unavailable())?;
        let mut rows = statement
            .query([info["id"].as_str().unwrap_or("")])
            .map_err(|_| unavailable())?;
        let mut count = 0;
        while let Some(row) = rows.next().map_err(|_| unavailable())? {
            count += 1;
            if count > MAX_ROWS {
                return Err(NativeError::new(
                    "unsupported",
                    "OpenCode 会话记录数超过读取限制",
                ));
            }
            let size = row
                .get::<_, i64>(3)
                .ok()
                .and_then(|n| u64::try_from(n).ok())
                .ok_or_else(unavailable)?;
            bytes = bytes.saturating_add(size);
            hash.update(
                serde_json::to_vec(&(
                    row.get::<_, String>(0).map_err(|_| unavailable())?,
                    row.get::<_, i64>(1).map_err(|_| unavailable())?,
                    row.get::<_, i64>(2).map_err(|_| unavailable())?,
                    size,
                ))
                .map_err(|_| unavailable())?,
            );
        }
    }
    if bytes > MAX_SESSION {
        return Err(NativeError::new(
            "unsupported",
            "OpenCode 单会话内容超过 64 MiB 读取限制",
        ));
    }
    Ok((format!("{:x}", hash.finalize()), bytes))
}
fn history(
    connection: &Connection,
    info: Value,
    bodies: bool,
    bytes: u64,
    mcp: opencode_mcp::Context,
    diagnostics: Vec<super::opencode_log::Diagnostic>,
    log_warnings: &[String],
) -> NativeResult<History> {
    let mut result = History {
        info,
        messages: vec![],
        parts: vec![],
        bytes,
        fingerprint: String::new(),
        warnings: vec![],
        coverage_warnings: mcp.warnings.clone(),
        mcp,
        diagnostics,
    };
    result.coverage_warnings.extend_from_slice(log_warnings);
    let mut hash = Sha256::new();
    hash.update(result.info.to_string());
    hash.update(&result.mcp.fingerprint);
    hash.update(super::opencode_log::fingerprint(&result.diagnostics));
    hash.update(serde_json::to_vec(&result.coverage_warnings).map_err(|_| unavailable())?);
    for table in ["message", "part"] {
        hash.update(table);
        let data = if bodies {
            "data"
        } else if table == "message" {
            "CASE WHEN json_valid(data) THEN json_remove(data,'$.error.data.responseHeaders','$.error.data.responseBody','$.error.data.metadata','$.summary') ELSE NULL END"
        } else {
            "CASE WHEN json_valid(data) THEN json_remove(data,'$.text','$.state.output','$.state.error','$.state.metadata') ELSE NULL END"
        };
        let parent = if table == "part" {
            "message_id"
        } else {
            "session_id"
        };
        let sql = format!(
            "SELECT id,{parent},time_created,{data},length(CAST(data AS BLOB)) FROM {table} WHERE session_id=?1 ORDER BY time_created,id LIMIT 100001"
        );
        let mut statement = connection.prepare(&sql).map_err(|_| unavailable())?;
        let mut rows = statement
            .query([result.info["id"].as_str().unwrap_or("")])
            .map_err(|_| unavailable())?;
        let mut count = 0;
        while let Some(row) = rows.next().map_err(|_| unavailable())? {
            count += 1;
            if count > MAX_ROWS {
                return Err(NativeError::new(
                    "unsupported",
                    "OpenCode 会话记录数超过读取限制",
                ));
            }
            let size = row
                .get::<_, i64>(4)
                .ok()
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(unavailable)?;
            // SQL is bounded before collecting text; retain a partial result on
            // a damaged/oversized record instead of losing every other message.
            if size > MAX_RECORD {
                result
                    .warnings
                    .push("跳过超出 8 MiB 限制的原生记录。".into());
                continue;
            }
            let id: String = row.get(0).map_err(|_| unavailable())?;
            let parent: String = row.get(1).map_err(|_| unavailable())?;
            let created: i64 = row.get(2).map_err(|_| unavailable())?;
            let raw: Option<String> = row.get(3).map_err(|_| unavailable())?;
            hash.update(
                serde_json::to_vec(&(&id, &parent, created, &raw)).map_err(|_| unavailable())?,
            );
            let Some(data) = raw
                .as_deref()
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
                .filter(Value::is_object)
            else {
                result
                    .warnings
                    .push("跳过无法解析的 OpenCode 消息或片段。".into());
                continue;
            };
            if !valid_id(&id) {
                result.warnings.push("跳过身份格式不受支持的记录。".into());
                continue;
            }
            let record = Record {
                id,
                parent,
                created,
                data,
            };
            if table == "message" {
                result.messages.push(record);
            } else {
                result.parts.push(record);
            }
        }
    }
    result.fingerprint = format!("{:x}", hash.finalize());
    result.warnings.sort();
    result.warnings.dedup();
    Ok(result)
}

pub(super) struct Scan {
    pub files: HashMap<PathBuf, IndexedFile>,
    pub total: usize,
    pub skipped: usize,
}
pub(super) struct OpenCodeSource;
impl crate::plugin::BuiltinPlugin for OpenCodeSource {
    fn descriptor(&self) -> crate::plugin::PluginDescriptor {
        super::source_plugins::descriptor("opencode", "io.localagent.builtin.source.opencode")
    }
    fn prepare(&self) -> Result<crate::plugin::PluginResources, crate::plugin::PluginError> {
        Ok(crate::plugin::PluginResources::default())
    }
}
impl super::source_plugins::ReadOnlySessionSource for OpenCodeSource {
    fn agent(&self) -> &'static str {
        "opencode"
    }
    fn has_database_activity(&self) -> bool {
        true
    }
    fn mcp_config(
        &self,
        context: &super::source_plugins::SourceContext<'_>,
        phase: super::mcp::ConfigPhase,
        scope: &mut super::mcp::ConfigScope<'_>,
        inventory: &mut super::mcp::Inventory,
    ) -> NativeResult<()> {
        if phase == super::mcp::ConfigPhase::Projects {
            context.check()?;
            super::mcp::config_sources::opencode_projects(context, scope, inventory);
        }
        Ok(())
    }
    fn memory(
        &self,
        context: &super::source_plugins::SourceContext<'_>,
        agent: &str,
        selected: Option<&str>,
        projects: &std::collections::BTreeSet<String>,
    ) -> NativeResult<super::MemorySnapshot> {
        super::memory::read(context, agent, "opencode", selected, projects)
    }
    fn activities(
        &self,
        context: &super::source_plugins::SourceContext<'_>,
        _files: &[IndexedFile],
        now: chrono::DateTime<chrono::Utc>,
    ) -> NativeResult<Option<(Vec<super::live::NativeActivity>, bool)>> {
        context.check()?;
        super::opencode_live::messages(context.home, now).map(Some)
    }
    fn scan(
        &self,
        context: &super::source_plugins::SourceContext<'_>,
        previous: &HashMap<PathBuf, IndexedFile>,
    ) -> NativeResult<super::source_plugins::SourceScan> {
        let by_id = previous
            .values()
            .map(|f| (f.summary.native_id.clone(), f.clone()))
            .collect();
        let result = scan_checked(context.home, &by_id, &|| context.check())?;
        let mut scan = super::source_plugins::SourceScan::database(
            result.files,
            result.total,
            result.skipped,
            (result.skipped > 0)
                .then(|| "部分 OpenCode 会话记录未完整读取，已保留可读内容。".into()),
            previous,
        );
        scan.scanned = scan.count;
        scan.versions = scan
            .files
            .values()
            .filter_map(|f| f.summary.agent_version.clone())
            .collect();
        scan.versions.sort();
        scan.versions.dedup();
        Ok(scan)
    }
    fn conversation(
        &self,
        context: &super::source_plugins::SourceContext<'_>,
        indexed: &IndexedFile,
        cursor: Option<&str>,
        anchor: Option<&str>,
    ) -> NativeResult<super::Conversation> {
        context.check()?;
        conversation(context.home, &indexed.summary.native_id, cursor, anchor)
    }
}
fn scan_checked(
    home: &Path,
    previous: &HashMap<String, IndexedFile>,
    check: &dyn Fn() -> NativeResult<()>,
) -> NativeResult<Scan> {
    check()?;
    let connection = open(home)?;
    let infos = sessions(&connection)?;
    let mut result = Scan {
        files: HashMap::new(),
        total: infos.len(),
        skipped: 0,
    };
    if infos.len() > MAX_SESSIONS {
        result.skipped += 1;
    }
    let mut contexts = HashMap::new();
    let mut projects = parser::ProjectCache::default();
    let logs = super::opencode_log::load(home);
    for info in infos.into_iter().take(MAX_SESSIONS) {
        check()?;
        let id = info["id"].as_str().unwrap_or("").to_string();
        if !valid_id(&id) {
            result.skipped += 1;
            continue;
        }
        let directory = info["directory"].as_str().unwrap_or("");
        let mcp = contexts
            .entry(directory.to_owned())
            .or_insert_with(|| opencode_mcp::load(home, directory))
            .clone();
        let Ok((database_revision, bytes)) = revision(&connection, &info) else {
            result.skipped += 1;
            continue;
        };
        let diagnostics = logs.sessions.get(&id).cloned().unwrap_or_default();
        let revision = format!(
            "{database_revision}:{}:{}:{:?}",
            mcp.fingerprint,
            super::opencode_log::fingerprint(&diagnostics),
            logs.warnings
        );
        let indexed = if let Some(old) = previous
            .get(&id)
            .filter(|old| old.database_revision.as_deref() == Some(&revision))
        {
            old.clone()
        } else {
            let Ok(mut parsed) = history(
                &connection,
                info,
                false,
                bytes,
                mcp,
                diagnostics,
                &logs.warnings,
            )
            .and_then(|h| parser::opencode_index(&path(home), h, &mut projects)) else {
                result.skipped += 1;
                continue;
            };
            if let Some(old) = previous
                .get(&id)
                .filter(|old| old.summary.project_path == parsed.summary.project_path)
            {
                parsed.summary.project_id = old.summary.project_id.clone();
                parsed.summary.project_root = old.summary.project_root.clone();
                for write in &mut parsed.writes {
                    if let Some(project) = &parsed.summary.project_id {
                        write.project_id = project.clone();
                    }
                }
            }
            IndexedFile {
                fact_calls: super::domain_facts::CallIndex::new(&parsed.calls),
                activity_messages: vec![],
                context: parsed.context.usage_summary(),
                native_products: parsed.summary.agent_products.clone(),
                native_unknown_non_calls: parsed.unknown_non_calls,
                agent: "opencode".into(),
                path: path(home),
                stamp: (bytes, 0, 0, 0, 0),
                database_revision: Some(revision),
                summary: parsed.summary,
                calls: std::sync::Arc::new(parsed.calls),
                writes: parsed.writes,
            }
        };
        if indexed.summary.partial {
            result.skipped += 1;
        }
        result.files.insert(
            path(home).with_file_name(format!("opencode-session-{id}")),
            indexed,
        );
    }
    Ok(result)
}

#[cfg(test)]
pub(super) fn scan(home: &Path, previous: &HashMap<String, IndexedFile>) -> NativeResult<Scan> {
    scan_checked(home, previous, &|| Ok(()))
}

pub(super) fn conversation(
    home: &Path,
    id: &str,
    page_cursor: Option<&str>,
    anchor: Option<&str>,
) -> NativeResult<super::Conversation> {
    let connection = open(home)?;
    let info = sessions(&connection)?
        .into_iter()
        .find(|v| v["id"].as_str() == Some(id))
        .ok_or_else(|| NativeError::new("not_found", "OpenCode 会话已删除或不在读取范围内"))?;
    let (_, bytes) = revision(&connection, &info)?;
    let mcp = opencode_mcp::load(home, info["directory"].as_str().unwrap_or(""));
    let mut logs = super::opencode_log::load(home);
    let history = history(
        &connection,
        info,
        true,
        bytes,
        mcp,
        logs.sessions.remove(id).unwrap_or_default(),
        &logs.warnings,
    )?;
    let fingerprint = format!("opencode-{}", history.fingerprint);
    let offset = super::cursor(page_cursor, &fingerprint)?;
    let mut parsed = parser::opencode(
        &path(home),
        history,
        Some(parser::Window { offset, anchor }),
    )?;
    let partial = parsed.summary.partial;
    // SQLite rows can change in place. Reject a changed cursor instead of
    // joining pages from two different native conversation snapshots.
    parsed.warnings.push("只读 OpenCode 历史记录；对话或 MCP 配置更新后需刷新再继续分页。MCP 当前配置关联不代表当时配置或当前已连接；运行时、远程和系统托管配置未读取，无法确认的来源保留未知。".into());
    parsed.warnings.push("模型异常补充当前 opencode.log 末尾的明确 Session 记录；同会话同类错误合并展示，轮次 / 最终结果未知，日志条数不代表请求 / 重试次数。日志未留存的错误可能缺失。".into());
    Ok(super::Conversation {
        context: parsed.context,
        session: parsed.summary,
        items: parsed.events,
        total: parsed.total,
        next_cursor: (parsed.end < parsed.total).then(|| format!("{fingerprint}:{}", parsed.end)),
        as_of: super::now(),
        coverage: if partial { "partial" } else { "native_history" },
        warnings: parsed.warnings,
    })
}
