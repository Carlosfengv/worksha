//! Antigravity App's per-conversation SQLite stores (step_format=0).
//! Only the explicitly authorized App directory is visited, never IDE / CLI,
//! credentials, summaries, brain artifacts or external tool-result references.
use super::{Capability, IndexedFile, NativeError, NativeResult, parser, safe_path};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::Duration,
};

pub(super) const AGENT: &str = "antigravity";
pub(super) fn root(home: &Path) -> PathBuf {
    home.join(".gemini/antigravity/conversations")
}
pub(super) fn capabilities() -> Vec<Capability> {
    [
        ("Session / 消息", "supported", "Antigravity App 2.23.1 SQLite / Protobuf step_format=0；正文按需读取。IDE、CLI 与旧 .pb 未接入"),
        ("工具 / MCP", "partial", "按原生步骤与调用 ID 关联；明确 MCP server_name 与当前配置分别展示，不打开外置结果"),
        ("Skill", "partial", "仅展示明确 Skill 工具调用；不把读取 SKILL.md 或规则文件当作执行"),
        ("Token", "partial", "仅统计 planner_response 中的原生 model_usage；未知模型编号保留原值，不把积分当费用"),
        ("模型异常", "partial", "仅读取模型响应的明确原生错误；工具错误不作为模型失败，未落盘错误可能缺失"),
        ("当前活动", "partial", "有界读取最新步骤的明确执行状态与时间，30 秒后过期；不从文件修改或进程存在推断"),
        ("实时 TTFT / 速度 / 费用", "unsupported", "原生历史不提供网关流式计时或账单；模型连接切换与订阅额度接口尚未接入"),
    ].into_iter().map(|(name,status,reason)| Capability { name,status,reason }).collect()
}
fn unavailable() -> NativeError {
    NativeError::new(
        "unavailable",
        "Antigravity 会话数据库不可读、正忙或结构未支持；保留已读取索引",
    )
}
pub(super) fn paths(home: &Path) -> NativeResult<(Vec<PathBuf>, bool)> {
    let root = root(home);
    if !safe_path(home, &root) {
        return Err(unavailable());
    }
    let mut paths = Vec::new();
    let mut limited = false;
    for (index, entry) in std::fs::read_dir(root)
        .map_err(|_| unavailable())?
        .enumerate()
    {
        if index >= 5000 {
            limited = true;
            break;
        }
        let entry = entry.map_err(|_| unavailable())?;
        let path = entry.path();
        if path.extension().is_some_and(|s| s == "db")
            && path
                .file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|s| uuid::Uuid::parse_str(s).is_ok())
        {
            paths.push(path);
        } else if path.extension().is_some_and(|s| s == "pb") {
            limited = true;
        }
    }
    paths.sort();
    Ok((paths, limited))
}
pub(super) fn open(home: &Path, path: &Path) -> NativeResult<Connection> {
    if path.parent() != Some(root(home).as_path()) || !safe_path(home, path) {
        return Err(unavailable());
    }
    for suffix in ["", "-wal", "-shm"] {
        let p = PathBuf::from(format!("{}{suffix}", path.display()));
        match p.symlink_metadata() {
            Ok(meta) => {
                if !safe_path(home, &p) || !meta.is_file() || meta.len() > 512 * 1024 * 1024 {
                    return Err(unavailable());
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    if meta.nlink() > 1 {
                        return Err(unavailable());
                    }
                }
            }
            Err(e) if !suffix.is_empty() && e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(unavailable()),
        }
    }
    let db = Connection::open_with_flags(
        path.canonicalize().map_err(|_| unavailable())?,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|_| unavailable())?;
    db.busy_timeout(Duration::from_millis(200))
        .map_err(|_| unavailable())?;
    db.execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF; BEGIN DEFERRED;")
        .map_err(|_| unavailable())?;
    for table in ["trajectory_meta", "steps", "trajectory_metadata_blob"] {
        let kind: String = db
            .query_row(
                "SELECT type FROM sqlite_master WHERE name=?1",
                [table],
                |r| r.get(0),
            )
            .map_err(|_| unavailable())?;
        if kind != "table" {
            return Err(unavailable());
        }
    }
    Ok(db)
}
pub(super) struct Step {
    pub index: usize,
    pub kind: u64,
    pub status: u64,
    pub payload: Vec<u8>,
}
pub(super) struct History {
    pub id: String,
    pub metadata: Vec<u8>,
    pub steps: Vec<Step>,
    pub bytes: u64,
    pub fingerprint: String,
    pub warnings: Vec<String>,
}
pub(super) fn history(home: &Path, path: &Path) -> NativeResult<History> {
    let db = open(home, path)?;
    let (id, cascade): (String, String) = db
        .query_row(
            "SELECT trajectory_id,cascade_id FROM trajectory_meta LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| unavailable())?;
    if path.file_stem().and_then(|s| s.to_str()) != Some(cascade.as_str())
        || uuid::Uuid::parse_str(&id).is_err()
    {
        return Err(unavailable());
    }
    let metadata: Vec<u8> = db
        .query_row(
            "SELECT data FROM trajectory_metadata_blob WHERE id='main' AND length(data)<=8388608",
            [],
            |r| r.get(0),
        )
        .map_err(|_| unavailable())?;
    let mut hash = Sha256::new();
    hash.update(&id);
    hash.update(&metadata);
    let mut h = History {
        id: cascade,
        bytes: metadata.len() as u64,
        metadata,
        steps: vec![],
        fingerprint: String::new(),
        warnings: vec![],
    };
    let mut statement = db.prepare("SELECT idx,step_type,status,step_format,length(step_payload),CASE WHEN length(step_payload)<=8388608 THEN step_payload END FROM steps ORDER BY idx LIMIT 100001").map_err(|_| unavailable())?;
    let mut rows = statement.query([]).map_err(|_| unavailable())?;
    while let Some(row) = rows.next().map_err(|_| unavailable())? {
        if h.steps.len() >= 100000 {
            h.warnings.push("步骤超过 100000 条，后续内容未读取".into());
            break;
        }
        let index = row.get::<_, u32>(0).map_err(|_| unavailable())? as usize;
        let kind = u64::from(row.get::<_, u32>(1).map_err(|_| unavailable())?);
        let status = u64::from(row.get::<_, u32>(2).map_err(|_| unavailable())?);
        let format = row.get::<_, u32>(3).map_err(|_| unavailable())?;
        let size = u64::try_from(row.get::<_, i64>(4).map_err(|_| unavailable())?)
            .map_err(|_| unavailable())?;
        h.bytes = h.bytes.saturating_add(size);
        hash.update(
            serde_json::to_vec(&(index, kind, status, format, size)).map_err(|_| unavailable())?,
        );
        if h.bytes > 64 * 1024 * 1024 {
            h.warnings.push("单会话超过 64 MiB，后续内容未读取".into());
            break;
        }
        let payload: Option<Vec<u8>> = row.get(5).map_err(|_| unavailable())?;
        if let Some(payload) = payload.filter(|_| format == 0) {
            hash.update(&payload);
            h.steps.push(Step {
                index,
                kind,
                status,
                payload,
            });
        } else {
            h.warnings
                .push("存在超限步骤或未支持的 step_format，已跳过".into());
        }
    }
    h.fingerprint = format!("{:x}", hash.finalize());
    Ok(h)
}
fn revision(path: &Path) -> Option<String> {
    Some(format!(
        "{:?}:{:?}",
        super::stamp(path)?,
        super::stamp(&PathBuf::from(format!("{}-wal", path.display())))
    ))
}
pub(super) struct AntigravitySource;
impl crate::plugin::BuiltinPlugin for AntigravitySource {
    fn descriptor(&self) -> crate::plugin::PluginDescriptor {
        super::source_plugins::descriptor(AGENT, "io.localagent.builtin.source.antigravity")
    }
    fn prepare(&self) -> Result<crate::plugin::PluginResources, crate::plugin::PluginError> {
        Ok(crate::plugin::PluginResources::default())
    }
}
impl super::source_plugins::ReadOnlySessionSource for AntigravitySource {
    fn agent(&self) -> &'static str {
        AGENT
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
        if phase == super::mcp::ConfigPhase::Global && scope.consented {
            context.check()?;
            super::mcp::config_sources::antigravity_global(context, scope, inventory);
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
        super::memory::read(context, agent, AGENT, selected, projects)
    }
    fn activities(
        &self,
        context: &super::source_plugins::SourceContext<'_>,
        files: &[IndexedFile],
        now: chrono::DateTime<chrono::Utc>,
    ) -> NativeResult<Option<(Vec<super::live::NativeActivity>, bool)>> {
        activities_scoped(context, files, now).map(Some)
    }
    fn scan(
        &self,
        context: &super::source_plugins::SourceContext<'_>,
        previous: &HashMap<PathBuf, IndexedFile>,
    ) -> NativeResult<super::source_plugins::SourceScan> {
        context.check()?;
        let result = paths(context.home);
        let mut files = HashMap::new();
        let mut skipped = 0;
        let mut error = None;
        let mut total = 0;
        match result {
            Err(e) => {
                error = Some(e.message);
                files = previous.clone();
            }
            Ok((paths, limited)) => {
                total = paths.len();
                skipped += usize::from(limited);
                for path in paths {
                    context.check()?;
                    let rev = revision(&path);
                    if let Some(old) = previous.get(&path).filter(|f| {
                        rev.is_some()
                            && f.database_revision == rev
                            && !f.summary.partial
                            && open(context.home, &path).is_ok()
                    }) {
                        files.insert(path, old.clone());
                        continue;
                    }
                    match history(context.home, &path)
                        .and_then(|h| parser::antigravity(&path, h, None))
                    {
                        Ok(parsed) => {
                            skipped += usize::from(parsed.summary.partial);
                            files.insert(
                                path.clone(),
                                IndexedFile {
                                    fact_calls: super::domain_facts::CallIndex::new(&parsed.calls),
                                    activity_messages: vec![],
                                    context: parsed.context.usage_summary(),
                                    native_products: parsed.summary.agent_products.clone(),
                                    native_unknown_non_calls: parsed.unknown_non_calls,
                                    agent: AGENT.into(),
                                    stamp: super::stamp(&path).unwrap_or((0, 0, 0, 0, 0)),
                                    database_revision: rev,
                                    path,
                                    summary: parsed.summary,
                                    calls: std::sync::Arc::new(parsed.calls),
                                    writes: parsed.writes,
                                },
                            );
                        }
                        Err(_) => {
                            skipped += 1;
                            if let Some(old) = previous.get(&path) {
                                files.insert(path, old.clone());
                            }
                        }
                    }
                }
                if limited {
                    for (path, old) in previous {
                        files.entry(path.clone()).or_insert_with(|| old.clone());
                    }
                }
            }
        }
        Ok(super::source_plugins::SourceScan::database(files, total, skipped,
            error.or_else(|| (skipped > 0).then(|| "部分 Antigravity 会话未完整读取；旧 .pb、未知格式、超限及不可读来源未作完整计数".into())), previous))
    }
    fn conversation(
        &self,
        context: &super::source_plugins::SourceContext<'_>,
        indexed: &IndexedFile,
        cursor: Option<&str>,
        anchor: Option<&str>,
    ) -> NativeResult<super::Conversation> {
        context.check()?;
        conversation(context.home, &indexed.path, cursor, anchor)
    }
}
pub(super) fn conversation(
    home: &Path,
    path: &Path,
    page_cursor: Option<&str>,
    anchor: Option<&str>,
) -> NativeResult<super::Conversation> {
    let h = history(home, path)?;
    let fingerprint = format!("antigravity-{}", h.fingerprint);
    let offset = super::cursor(page_cursor, &fingerprint)?;
    let mut parsed = parser::antigravity(path, h, Some(parser::Window { offset, anchor }))?;
    let partial = parsed.summary.partial;
    parsed.warnings.push("Antigravity App 历史只读；不读取 IDE / CLI、凭据、附件、外置工具结果或隐藏推理。原生步骤与模型响应不等于 HTTP 请求，额度与流式计时未知。".into());
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

/// Candidate mtimes only bound the work; explicit native status + timestamp
/// establish activity. Only a bounded user excerpt is added; tool bodies stay private.
#[cfg(test)]
pub(super) fn activities(
    sessions: &super::NativeSessions,
    now: chrono::DateTime<chrono::Utc>,
) -> NativeResult<(Vec<super::live::NativeActivity>, bool)> {
    if !sessions
        .active_sources()?
        .iter()
        .any(|s| s.agent_id == AGENT && s.enabled)
    {
        return Ok((vec![], false));
    }
    let files = sessions
        .lock()?
        .files
        .values()
        .filter(|f| f.agent == AGENT)
        .cloned()
        .collect::<Vec<_>>();
    sessions.with_source_context(AGENT, None, true, |_, context| {
        activities_scoped(context, &files, now)
    })
}
fn activities_scoped(
    context: &super::source_plugins::SourceContext<'_>,
    files: &[IndexedFile],
    now: chrono::DateTime<chrono::Utc>,
) -> NativeResult<(Vec<super::live::NativeActivity>, bool)> {
    context.check()?;
    let (mut candidates, mut limited) = paths(context.home)?;
    candidates.sort_by_key(|p| std::cmp::Reverse(super::stamp(p).map(|s| s.1)));
    limited |= candidates.len() > 128;
    let summaries: HashMap<_, _> = files
        .iter()
        .map(|f| (f.path.clone(), f.summary.clone()))
        .collect();
    let mut rows = Vec::new();
    for path in candidates.into_iter().take(128) {
        context.check()?;
        let result: NativeResult<Option<super::live::NativeActivity>> = (|| {
            let db = open(context.home, &path)?;
            let latest = db.query_row("SELECT idx,step_type,status,step_format,CASE WHEN length(metadata)<=262144 THEN metadata END FROM steps ORDER BY idx DESC LIMIT 1",[],|r| Ok((r.get::<_,u32>(0)?,r.get::<_,u32>(1)?,r.get::<_,u32>(2)?,r.get::<_,u32>(3)?,r.get::<_,Option<Vec<u8>>>(4)?))).optional().map_err(|_|unavailable())?;
            let Some((idx, kind, status, format, raw)) = latest else {
                return Ok(None);
            };
            if format != 0 {
                return Err(unavailable());
            }
            if !matches!(status, 1 | 2 | 8 | 9 | 11) || kind == 14 {
                return Ok(None);
            }
            let meta =
                super::antigravity_proto::Message::parse(raw.as_deref().ok_or_else(unavailable)?)?;
            if meta.time(8).is_some() {
                return Ok(None);
            }
            let Some(time) = meta
                .time(22)
                .or_else(|| meta.time(32))
                .or_else(|| meta.time(1))
            else {
                return Ok(None);
            };
            let timestamp =
                chrono::DateTime::parse_from_rfc3339(&time).map_err(|_| unavailable())?;
            let age = now.signed_duration_since(timestamp).num_milliseconds();
            if !(-1000..=30000).contains(&age) {
                return Ok(None);
            }
            let user: Option<u32> = db
                .query_row("SELECT max(idx) FROM steps WHERE step_type=14", [], |r| {
                    r.get(0)
                })
                .map_err(|_| unavailable())?;
            let Some(user) = user.filter(|u| *u < idx) else {
                return Ok(None);
            };
            let Some(summary) = summaries.get(&path) else {
                return Ok(None);
            };
            let name = meta
                .child(4)?
                .text(2)
                .map(|s| super::sanitize_label(s, 200))
                .unwrap_or_else(|| format!("原生步骤 #{kind}"));
            let input = db.query_row("SELECT CASE WHEN length(step_payload)<=65536 THEN step_payload END,CASE WHEN length(metadata)<=262144 THEN metadata END FROM steps WHERE idx=?1 AND step_type=14 AND step_format=0",[user],|r|Ok((r.get::<_,Option<Vec<u8>>>(0)?,r.get::<_,Option<Vec<u8>>>(1)?))).optional().map_err(|_|unavailable())?;
            let message_started_at = input.as_ref().and_then(|(_, metadata)| {
                let meta = super::antigravity_proto::Message::parse(metadata.as_deref()?).ok()?;
                meta.time(1).or_else(|| meta.time(22)).filter(|at| {
                    chrono::DateTime::parse_from_rfc3339(at).is_ok_and(|at| at <= timestamp)
                })
            });
            let user_input = input.and_then(|(payload, metadata)| {
                let payload = super::antigravity_proto::Message::parse(payload.as_deref()?).ok()?;
                let user = payload.child(19).ok()?;
                let mut texts = user
                    .all(3)
                    .filter_map(|raw| {
                        super::antigravity_proto::Message::parse(raw)
                            .ok()?
                            .text(1)
                            .map(str::to_owned)
                    })
                    .take(64)
                    .collect::<Vec<_>>();
                if texts.is_empty() {
                    texts.extend(user.text(1).or(user.text(2)).map(str::to_owned));
                }
                let meta = super::antigravity_proto::Message::parse(metadata.as_deref()?).ok()?;
                let at = meta.time(1).or_else(|| meta.time(22))?;
                super::activity_input::preview(&serde_json::Value::String(texts.join("\n")))
                    .map(|text| super::activity_input::UserInput { text, at })
            });
            Ok(Some(super::live::NativeActivity {
                message_usage: None,
                turn_id: Some(format!("{}:turn:step:{user}", summary.id)),
                #[cfg(any(target_os = "macos", test))]
                call_id: None,
                agent_product_id: Some(AGENT.into()),
                agent_attribution: Some("native_app_store".into()),
                id: summary.id.clone(),
                agent_id: AGENT.into(),
                session_id: summary.id.clone(),
                title: summary.title.clone(),
                project: summary.project_path.clone(),
                model: parser::antigravity_model(&meta),
                phase: if kind == 15 {
                    "processing"
                } else {
                    "tool_requested"
                }
                .into(),
                step: if kind == 15 {
                    "模型生成中".into()
                } else {
                    format!("执行工具 · {name}")
                },
                user_input,
                tool_names: if kind == 15 { vec![] } else { vec![name] },
                message_started_at,
                event_at: time,
                turn_ttft_ms: None,
                realtime: None,
                stale: false,
            }))
        })();
        match result {
            Ok(Some(row)) => rows.push(row),
            Ok(None) => {}
            Err(_) => limited = true,
        }
    }
    context.check()?;
    Ok((rows, limited))
}
