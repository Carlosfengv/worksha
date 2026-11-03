//! Client evidence is scoped to one native tool execution, never its whole turn.
//! Only fixed local desktop execution log events are read; no bodies are retained.
use super::{CallSummary, Catalog, IndexedFile, parser::Parsed, safe_path, stamp};
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    fs::{self, OpenOptions},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttributionStatus {
    pub status: &'static str,
    pub scanned_log_files: usize,
    pub confirmed_tool_executions: usize,
    pub matched_calls: usize,
    pub conflicting_calls: usize,
    pub ambiguous_executions: usize,
}
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct Key {
    turn: String,
    call: String,
    tool: String,
}
struct LogFile {
    stamp: (u64, u128, u64, u64, u128),
    identity: (u64, u64),
    partial: bool,
    starts: HashSet<(String, Key)>,
    ends: HashSet<(String, Key)>,
}
#[derive(Default)]
pub(super) struct EvidenceCache {
    files: HashMap<PathBuf, LogFile>,
    applied: Option<(u64, HashSet<Key>, AttributionStatus)>,
}
pub(super) struct Evidence {
    keys: HashSet<Key>,
    pub(super) status: AttributionStatus,
}

impl EvidenceCache {
    pub fn apply(&mut self, catalog: &mut Catalog, mut evidence: Evidence) {
        if let Some((revision, keys, status)) = &self.applied
            && (*revision == catalog.revision || (keys.is_empty() && evidence.keys.is_empty()))
            && keys == &evidence.keys
        {
            evidence.status.matched_calls = status.matched_calls;
            evidence.status.conflicting_calls = status.conflicting_calls;
            evidence.status.ambiguous_executions = status.ambiguous_executions;
            if let Some(source) = catalog.sources.iter_mut().find(|s| s.agent_id == "codex") {
                source.client_attribution = Some(evidence.status.clone());
            }
        } else {
            apply(
                catalog,
                Evidence {
                    keys: evidence.keys.clone(),
                    status: evidence.status,
                },
            );
            evidence.status = catalog
                .sources
                .iter()
                .find(|s| s.agent_id == "codex")
                .unwrap()
                .client_attribution
                .clone()
                .unwrap_or_default();
        }
        self.applied = Some((catalog.revision, evidence.keys, evidence.status));
    }
}

fn identifier(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 512
        && v != "unknown"
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
}
fn run(path: &Path) -> Option<String> {
    let name = path
        .file_name()?
        .to_str()?
        .strip_prefix("codex-desktop-")?
        .strip_suffix(".log")?;
    let (run, _) = name.split_once("-t")?;
    let (uuid, pid) = run.rsplit_once('-')?;
    (uuid.len() == 36
        && uuid.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
        && !pid.is_empty()
        && pid.bytes().all(|b| b.is_ascii_digit()))
    .then(|| run.to_owned())
}
fn execution(line: &str) -> Option<(bool, Key)> {
    let (time, body) = line.trim_end().split_once(' ')?;
    chrono::DateTime::parse_from_rfc3339(time).ok()?;
    let body = body.strip_prefix("info [electron-message-handler] ")?;
    let (start, fields) =
        if let Some(rest) = body.strip_prefix("dynamic_app_tool_renderer_execution_started ") {
            (true, rest)
        } else {
            (false, body.strip_prefix("Sending server response ")?)
        };
    let mut values = HashMap::new();
    for token in fields.split_whitespace() {
        let (key, value) = token.split_once('=')?;
        if ![
            "callId",
            "turnId",
            "tool",
            "isRemoteHost",
            "success",
            "rendererWebContentsId",
            "rendererWindowAppearance",
            "rendererWindowFocused",
            "rendererWindowId",
            "rendererWindowVisible",
        ]
        .contains(&key)
            || values.insert(key, value).is_some()
        {
            return None;
        }
    }
    if values.get("isRemoteHost") != Some(&"false")
        || (!start && !matches!(values.get("success"), Some(&"true" | &"false")))
    {
        return None;
    }
    let turn = *values.get("turnId")?;
    let call = *values.get("callId")?;
    let tool = *values.get("tool")?;
    [turn, call, tool].into_iter().all(identifier).then(|| {
        (
            start,
            Key {
                turn: turn.into(),
                call: call.into(),
                tool: tool.into(),
            },
        )
    })
}
#[cfg(unix)]
fn file_identity(m: &fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (m.dev(), m.ino())
}
#[cfg(not(unix))]
fn file_identity(_: &fs::Metadata) -> (u64, u64) {
    (0, 0)
}
fn paths(home: &Path, root: &Path, depth: usize, result: &mut Vec<PathBuf>, partial: &mut bool) {
    if result.len() >= 2048 || !safe_path(home, root) {
        *partial = true;
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        *partial = true;
        return;
    };
    for (index, entry) in entries.take(4097).enumerate() {
        if index == 4096 {
            *partial = true;
            break;
        }
        let Ok(entry) = entry else {
            *partial = true;
            continue;
        };
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            *partial = true;
            continue;
        };
        if kind.is_symlink() {
            *partial = true;
            continue;
        }
        if depth > 0
            && kind.is_dir()
            && entry.file_name().to_str().is_some_and(|s| {
                s.len() == if depth == 3 { 4 } else { 2 } && s.bytes().all(|b| b.is_ascii_digit())
            })
        {
            paths(home, &path, depth - 1, result, partial);
        } else if depth == 0 && kind.is_file() && run(&path).is_some() {
            result.push(path);
        }
        if result.len() >= 2048 {
            *partial = true;
            break;
        }
    }
}
impl EvidenceCache {
    pub(super) fn snapshot(&mut self, home: &Path, enabled: bool) -> Evidence {
        let mut status = AttributionStatus {
            status: "not_found",
            ..Default::default()
        };
        if !enabled {
            self.files.clear();
            status.status = "not_enabled";
            return Evidence {
                keys: HashSet::new(),
                status,
            };
        }
        let root = home.join("Library/Logs/com.openai.codex");
        let mut candidates = vec![];
        let mut partial = false;
        if root.exists() {
            paths(home, &root, 3, &mut candidates, &mut partial);
        }
        candidates.sort_by_cached_key(|p| std::cmp::Reverse(stamp(p).map(|s| s.1)));
        let mut seen = HashSet::new();
        let mut bytes = 0u64;
        for path in candidates {
            let Some(mark) = stamp(&path) else {
                partial = true;
                continue;
            };
            if mark.0 > 16 * 1024 * 1024
                || bytes.saturating_add(mark.0) > 256 * 1024 * 1024
                || !safe_path(home, &path)
            {
                partial = true;
                continue;
            }
            bytes += mark.0;
            let Ok(meta) = fs::symlink_metadata(&path) else {
                partial = true;
                continue;
            };
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if meta.nlink() != 1 {
                    partial = true;
                    continue;
                }
            }
            let identity = file_identity(&meta);
            seen.insert(path.clone());
            if let Some(cached) = self
                .files
                .get(&path)
                .filter(|f| f.stamp == mark && f.identity == identity)
            {
                partial |= cached.partial;
                continue;
            }
            self.files.remove(&path);
            let mut options = OpenOptions::new();
            options.read(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(libc::O_NOFOLLOW);
            }
            let Ok(file) = options.open(&path) else {
                partial = true;
                continue;
            };
            if !file.metadata().is_ok_and(|m| file_identity(&m) == identity) {
                partial = true;
                continue;
            }
            let mut parsed = LogFile {
                stamp: mark,
                identity,
                partial: false,
                starts: HashSet::new(),
                ends: HashSet::new(),
            };
            let run = run(&path).unwrap();
            let mut reader = BufReader::new(std::io::Read::take(file, mark.0));
            let mut line = vec![];
            let mut failed = false;
            loop {
                line.clear();
                match reader.read_until(b'\n', &mut line) {
                    Ok(0) => break,
                    Ok(_) if line.last() == Some(&b'\n') && line.len() <= 64 * 1024 => {
                        if let Ok(text) = std::str::from_utf8(&line)
                            && let Some((start, key)) = execution(text)
                        {
                            if start {
                                parsed.starts.insert((run.clone(), key));
                            } else {
                                parsed.ends.insert((run.clone(), key));
                            }
                        }
                    }
                    Ok(_) => parsed.partial = true,
                    Err(_) => {
                        failed = true;
                        break;
                    }
                }
            }
            partial |= parsed.partial;
            if !failed
                && stamp(&path) == Some(mark)
                && fs::symlink_metadata(&path).is_ok_and(|m| file_identity(&m) == identity)
            {
                self.files.insert(path, parsed);
            } else {
                partial = true;
            }
        }
        self.files.retain(|p, _| seen.contains(p));
        let starts: HashSet<_> = self.files.values().flat_map(|f| f.starts.iter()).collect();
        let ends: HashSet<_> = self.files.values().flat_map(|f| f.ends.iter()).collect();
        let keys = starts
            .intersection(&ends)
            .map(|(_, key)| key.clone())
            .collect::<HashSet<_>>();
        status.status = if partial {
            "partial"
        } else if self.files.is_empty() {
            "not_found"
        } else {
            "ready"
        };
        status.scanned_log_files = self.files.len();
        status.confirmed_tool_executions = keys.len();
        Evidence { keys, status }
    }
}
fn call_key(call: &CallSummary) -> Option<Key> {
    if call.agent_id != "codex" || !["mcp", "tool"].contains(&call.kind.as_str()) {
        return None;
    }
    let turn = call
        .turn_id
        .strip_prefix(&format!("{}:turn:", call.session_id))?;
    let id = call
        .id
        .strip_prefix(&format!("{}:call:", call.session_id))?;
    let tool = call
        .tool_name
        .as_deref()
        .or_else(|| call.name.rsplit_once("__").map(|(_, tool)| tool))
        .unwrap_or(&call.name);
    [turn, id, tool].into_iter().all(identifier).then(|| Key {
        turn: turn.into(),
        call: id.into(),
        tool: tool.into(),
    })
}
fn summarize(file: &mut IndexedFile) {
    let mut products = file.native_products.clone();
    products.extend(file.calls.iter().filter_map(|c| c.agent_product_id.clone()));
    products.sort();
    products.dedup();
    file.summary.has_unknown_client = file.native_unknown_non_calls
        || file.calls.iter().any(|c| c.agent_product_id.is_none())
        || products.is_empty();
    file.summary.agent_product_id =
        (!file.summary.has_unknown_client && products.len() == 1).then(|| products[0].clone());
    file.summary.agent_products = products;
    for write in &mut file.writes {
        if let Some(call) = file.calls.iter().find(|c| c.id == write.id) {
            write.agent_product_id = call.agent_product_id.clone();
        }
    }
}
pub(super) fn apply(catalog: &mut Catalog, mut evidence: Evidence) {
    let mut candidates: HashMap<Key, (HashSet<String>, bool)> = HashMap::new();
    for file in catalog.files.values().filter(|f| f.agent == "codex") {
        for call in file.calls.iter() {
            if let Some(key) = call_key(call) {
                let candidate = candidates.entry(key).or_default();
                candidate.0.insert(call.session_id.clone());
                candidate.1 |= call
                    .native_agent_product_id
                    .as_deref()
                    .is_some_and(|p| p != "codex-desktop");
            }
        }
    }
    let mut changed = false;
    let mut ambiguous = HashSet::new();
    for file in catalog.files.values_mut().filter(|f| f.agent == "codex") {
        for index in 0..file.calls.len() {
            let call = &file.calls[index];
            let mut product = call.native_agent_product_id.clone();
            let mut attribution = product.as_ref().map(|_| "native_record");
            if let Some(key) = call_key(call)
                && evidence.keys.contains(&key)
            {
                if candidates[&key].0.len() != 1 {
                    ambiguous.insert(key);
                    if product.is_none() {
                        attribution = Some("ambiguous_evidence");
                    }
                } else if candidates[&key].1 {
                    product = None;
                    attribution = Some("conflicting_evidence");
                } else {
                    product = Some("codex-desktop".into());
                    attribution = Some("desktop_tool_execution");
                }
            }
            if (call.agent_product_id.as_ref(), call.agent_attribution)
                != (product.as_ref(), attribution)
            {
                let call = &mut std::sync::Arc::make_mut(&mut file.calls)[index];
                call.agent_product_id = product;
                call.agent_attribution = attribution;
                changed = true;
            }
        }
        summarize(file);
    }
    if changed {
        catalog.revision += 1;
        catalog.as_of = super::now();
    }
    // Status must use the same canonical transcript versions as all native pages.
    // Older paginated copies may contain additional call IDs hidden by unique_files.
    for call in super::unique_files(catalog)
        .into_iter()
        .flat_map(|f| f.calls.iter())
    {
        match call.agent_attribution {
            Some("desktop_tool_execution") => evidence.status.matched_calls += 1,
            Some("conflicting_evidence") => evidence.status.conflicting_calls += 1,
            _ => {}
        }
    }
    evidence.status.ambiguous_executions = ambiguous.len();
    if let Some(source) = catalog.sources.iter_mut().find(|s| s.agent_id == "codex") {
        source.client_attribution = Some(evidence.status);
    }
}
pub(super) fn conversation(parsed: &mut Parsed, indexed: &IndexedFile) {
    for call in &mut parsed.calls {
        if let Some(current) = indexed.calls.iter().find(|c| {
            c.id == call.id
                && c.native_agent_product_id == call.native_agent_product_id
                && call_key(c) == call_key(call)
        }) {
            call.agent_product_id = current.agent_product_id.clone();
            call.agent_attribution = current.agent_attribution;
        }
    }
    for event in &mut parsed.events {
        if let Some(call) = parsed.calls.iter().find(|c| c.id == event.id) {
            event.agent_product_id = call.agent_product_id.clone();
            event.agent_attribution = call.agent_attribution;
        }
    }
    parsed.summary.agent_products.extend(
        parsed
            .calls
            .iter()
            .filter_map(|c| c.agent_product_id.clone()),
    );
    parsed.summary.agent_products.sort();
    parsed.summary.agent_products.dedup();
    parsed.summary.has_unknown_client = parsed.unknown_non_calls
        || parsed.calls.iter().any(|c| c.agent_product_id.is_none())
        || parsed.summary.agent_products.is_empty();
    parsed.summary.agent_product_id = (!parsed.summary.has_unknown_client
        && parsed.summary.agent_products.len() == 1)
        .then(|| parsed.summary.agent_products[0].clone());
}
