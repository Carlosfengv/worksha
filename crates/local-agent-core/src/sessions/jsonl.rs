//! Claude and Codex retain their original bounded JSONL parser and index cache.
use super::source_plugins::{ReadOnlySessionSource, SourceContext, SourceScan};
use super::*;
use crate::plugin::{BuiltinPlugin, PluginDescriptor, PluginError, PluginResources};

pub(super) struct JsonlSource(pub &'static str);
impl BuiltinPlugin for JsonlSource {
    fn descriptor(&self) -> PluginDescriptor {
        source_plugins::descriptor(
            self.0,
            if self.0 == "claude" {
                "io.localagent.builtin.source.claude"
            } else {
                "io.localagent.builtin.source.codex"
            },
        )
    }
    fn prepare(&self) -> Result<PluginResources, PluginError> {
        Ok(PluginResources::default())
    }
}

impl ReadOnlySessionSource for JsonlSource {
    fn agent(&self) -> &'static str {
        self.0
    }
    fn mcp_config(
        &self,
        context: &SourceContext<'_>,
        phase: super::mcp::ConfigPhase,
        scope: &mut super::mcp::ConfigScope<'_>,
        inventory: &mut super::mcp::Inventory,
    ) -> NativeResult<()> {
        context.check()?;
        use super::mcp::{ConfigPhase, config_sources};
        match (self.0, phase) {
            ("claude", ConfigPhase::Global) => {
                config_sources::claude_global(context, scope, inventory)
            }
            ("claude", ConfigPhase::Projects) => {
                config_sources::claude_projects(context, scope, inventory)
            }
            ("codex", ConfigPhase::Global) => inventory.codex_global(context.home),
            ("codex", ConfigPhase::Projects) => {
                config_sources::codex_projects(context, scope, inventory)
            }
            _ => {}
        }
        Ok(())
    }
    fn memory(
        &self,
        context: &SourceContext<'_>,
        agent: &str,
        selected: Option<&str>,
        projects: &std::collections::BTreeSet<String>,
    ) -> NativeResult<super::MemorySnapshot> {
        super::memory::read(context, agent, self.0, selected, projects)
    }
    fn scan(
        &self,
        context: &SourceContext<'_>,
        previous: &HashMap<PathBuf, IndexedFile>,
    ) -> NativeResult<SourceScan> {
        let agent = self.0;
        context.check()?;
        let mut paths = vec![];
        let mut skipped = 0;
        let mut read_error = false;
        let mut found_roots = 0;
        for root in roots(context.home, agent) {
            context.check()?;
            match fs::symlink_metadata(&root) {
                Ok(_) => found_roots += 1,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => {
                    read_error = true;
                    continue;
                }
            }
            if !safe_path(context.home, &root) {
                skipped += 1;
                read_error = true;
                continue;
            }
            collect(
                &root,
                if agent == "claude" { 1 } else { 3 },
                &mut paths,
                &mut skipped,
                &mut read_error,
            );
        }
        read_error |= found_roots == 0;
        paths.sort();
        let (titles, title_error) = native_titles(context.home, agent, &paths);
        read_error |= title_error;
        context.progress(paths.len(), 0)?;
        let retain: Option<HashSet<_>> = previous.is_empty().then(|| {
            let mut newest: Vec<_> = paths
                .iter()
                .filter_map(|p| stamp(p).map(|s| (s.1, p)))
                .collect();
            newest.sort_unstable();
            newest.into_iter().rev().take(8).map(|(_, p)| p).collect()
        });
        let mut files = previous.clone();
        let mut projects = parser::ProjectCache::default();
        let mut seen = HashSet::new();
        let mut revision_delta = 0;
        let mut scanned = 0;
        for path in &paths {
            context.check()?;
            seen.insert(path.clone());
            let Some(stamp) = stamp(path) else {
                read_error = true;
                continue;
            };
            let unchanged = files.get(path).is_some_and(|f| f.stamp == stamp);
            if !unchanged {
                let parsed = context
                    .index_cache
                    .lock()
                    .map_err(|_| NativeError::new("unavailable", "原生索引缓存不可用"))?
                    .parse(
                        context.home,
                        path,
                        agent,
                        retain.as_ref().is_none_or(|paths| paths.contains(path)),
                        &mut projects,
                    );
                context.check()?;
                match parsed {
                    Ok(mut parsed) => {
                        if let Some(old) = files.get(path)
                            && old.summary.project_path == parsed.summary.project_path
                        {
                            parsed.summary.project_id = old.summary.project_id.clone();
                            parsed.summary.project_root = old.summary.project_root.clone();
                            if let Some(id) = &parsed.summary.project_id {
                                for write in &mut parsed.writes {
                                    write.project_id = id.clone();
                                }
                            }
                        }
                        if let Some(title) = titles.get(&parsed.summary.native_id) {
                            parsed.summary.title = title.clone();
                            parsed.summary.title_source = "native_title".into();
                            for call in &mut parsed.calls {
                                call.session_title = title.clone();
                            }
                        }
                        files.insert(
                            path.clone(),
                            IndexedFile {
                                fact_calls: super::domain_facts::CallIndex::new(&parsed.calls),
                                activity_messages: parsed.activity_messages,
                                context: parsed.context.usage_summary(),
                                native_products: parsed.summary.agent_products.clone(),
                                native_unknown_non_calls: parsed.unknown_non_calls,
                                agent: agent.into(),
                                path: path.clone(),
                                stamp,
                                database_revision: None,
                                summary: parsed.summary,
                                calls: Arc::new(parsed.calls),
                                writes: parsed.writes,
                            },
                        );
                        revision_delta += 1;
                    }
                    Err(error) => {
                        let safe = safe_path(context.home, path)
                            && fs::symlink_metadata(path).is_ok_and(|meta| {
                                #[cfg(unix)]
                                {
                                    use std::os::unix::fs::MetadataExt;
                                    meta.is_file() && meta.nlink() == 1
                                }
                                #[cfg(not(unix))]
                                {
                                    meta.is_file()
                                }
                            });
                        if safe && error.code != "unauthorized" {
                            if let Some(old) = files.get_mut(path)
                                && !old.summary.partial
                            {
                                old.summary.partial = true;
                                revision_delta += 1;
                            }
                        } else if files.remove(path).is_some() {
                            revision_delta += 1;
                        }
                        skipped += 1;
                        read_error = true;
                    }
                }
            } else if let Some(file) = files.get_mut(path)
                && let Some(title) = titles.get(&file.summary.native_id)
                && file.summary.title != *title
            {
                file.summary.title = title.clone();
                file.summary.title_source = "native_title".into();
                for call in Arc::make_mut(&mut file.calls) {
                    call.session_title = title.clone();
                }
                revision_delta += 1;
            }
            scanned += 1;
            context.progress(paths.len(), scanned)?;
        }
        if !read_error {
            let before = files.len();
            files.retain(|path, _| seen.contains(path));
            revision_delta += u64::from(before != files.len());
        }
        let catalog = Catalog {
            sources: vec![],
            files,
            revision: 0,
            as_of: String::new(),
        };
        let unique = unique_files(&catalog);
        let count = unique.len();
        let partial = unique.iter().filter(|f| f.summary.partial).count();
        read_error |= partial > 0;
        skipped = skipped.max(partial);
        let mut versions: Vec<_> = unique
            .iter()
            .filter_map(|f| f.summary.agent_version.clone())
            .collect();
        versions.sort();
        versions.dedup();
        Ok(SourceScan {
            files: catalog.files,
            total: paths.len(),
            scanned,
            count,
            skipped,
            error: read_error.then(|| {
                "部分来源不可读或超出读取限制，已保留可读取的会话；未扫描其他位置。".into()
            }),
            versions,
            revision_delta,
        })
    }

    fn conversation(
        &self,
        context: &SourceContext<'_>,
        indexed: &IndexedFile,
        page_cursor: Option<&str>,
        anchor: Option<&str>,
    ) -> NativeResult<Conversation> {
        read_conversation(context, indexed, page_cursor, anchor)
    }
}

fn read_conversation(
    context: &SourceContext<'_>,
    indexed: &IndexedFile,
    page_cursor: Option<&str>,
    anchor: Option<&str>,
) -> NativeResult<Conversation> {
    context.check()?;
    let current_stamp = stamp(&indexed.path)
        .ok_or_else(|| NativeError::new("not_found", "原生会话文件已不可读取"))?;
    let (snapshot_bytes, expected_hash, offset) = if let Some(value) = page_cursor {
        let (revision, _) = value
            .rsplit_once(':')
            .ok_or_else(|| NativeError::new("invalid_input", "分页位置无效"))?;
        let (size, hash) = revision
            .split_once('-')
            .ok_or_else(|| NativeError::new("invalid_input", "分页位置无效"))?;
        let size: u64 = size
            .parse()
            .map_err(|_| NativeError::new("invalid_input", "分页位置无效"))?;
        if hash.len() != 64 || !hash.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(NativeError::new("invalid_input", "分页位置无效"));
        }
        if size > current_stamp.0 {
            return Err(NativeError::new(
                "stale_snapshot",
                "会话文件已缩短，请刷新后继续阅读",
            ));
        }
        (size, Some(hash), cursor(Some(value), revision)?)
    } else {
        (current_stamp.0, None, 0)
    };
    let mut parsed = parser::parse_snapshot(
        context.home,
        &indexed.path,
        &indexed.agent,
        Some(parser::Window { offset, anchor }),
        snapshot_bytes,
    )?;
    if expected_hash.is_some_and(|hash| hash != parsed.fingerprint) {
        return Err(NativeError::new(
            "stale_snapshot",
            "会话历史内容已改写，请刷新后继续阅读",
        ));
    }
    if indexed.agent == "codex" {
        codex_attribution::conversation(&mut parsed, indexed);
    }
    parsed.summary.project_id = indexed.summary.project_id.clone();
    parsed.summary.project_root = indexed.summary.project_root.clone();
    if indexed.summary.title_source == "native_title" {
        parsed.summary.title = indexed.summary.title.clone();
        parsed.summary.title_source = "native_title".into();
    }
    context.check()?;
    let changed =
        stamp(&indexed.path).is_none_or(|after| after != current_stamp || after.0 > snapshot_bytes);
    let mut warnings = parsed.warnings;
    if changed {
        warnings.push("原文件仍在更新，分页保持当前快照；刷新对话后查看新增记录。".into());
    }
    Ok(Conversation {
        context: parsed.context,
        session: parsed.summary,
        total: parsed.total,
        items: parsed.events,
        next_cursor: (parsed.end < parsed.total).then(|| {
            format!(
                "{}-{}:{}",
                parsed.snapshot_bytes, parsed.fingerprint, parsed.end
            )
        }),
        as_of: now(),
        coverage: if changed || !warnings.is_empty() {
            "partial"
        } else {
            "native_history"
        },
        warnings,
    })
}
