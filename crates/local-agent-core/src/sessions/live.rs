//! Recent activity from native tails and bounded database metadata.
//! A file write is never treated as a streamed token.
use super::activity_input::{self, UserInput};
use super::activity_input_cache::{CachedInput, CachedNative, InputCache};
use super::activity_performance::{ActivitySampling, MessageUsage};
use super::{ContextUsage, NativeSessions, collect, context::Collector, safe_path};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    fs::OpenOptions,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::PathBuf,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeActivity {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_usage: Option<MessageUsage>,
    #[serde(rename = "messageId", serialize_with = "serialize_message_id")]
    pub(super) turn_id: Option<String>,
    #[serde(skip)]
    #[cfg(any(target_os = "macos", test))]
    pub(super) call_id: Option<String>,
    pub agent_product_id: Option<String>,
    pub agent_attribution: Option<String>,
    pub id: String,
    pub agent_id: String,
    pub session_id: String,
    pub title: String,
    pub project: Option<String>,
    pub model: Option<String>,
    pub phase: String,
    pub step: String,
    pub user_input: Option<UserInput>,
    pub tool_names: Vec<String>,
    pub message_started_at: Option<String>,
    pub event_at: String,
    pub turn_ttft_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub realtime: Option<NativeRealtime>,
    pub stale: bool,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeRealtime {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling: Option<ActivitySampling>,
    pub state: String,
    pub first_token_ms: Option<f64>,
    pub first_token_missed: bool,
    pub first_text_ms: Option<f64>,
    pub first_output_ms: Option<f64>,
    pub text_tokens_per_second: Option<f64>,
    pub output_tokens_per_second: Option<f64>,
    pub average_text_tokens_per_second: Option<f64>,
}
impl NativeActivity {
    pub fn message_id(&self) -> Option<&str> {
        public_message_id(&self.turn_id)
    }
}
struct Tail {
    message_usages: HashMap<String, MessageUsage>,
    message_started_at: Option<String>,
    user_input: Option<UserInput>,
    tool_calls: HashMap<String, String>,
    turn_id: Option<String>,
    exec_turn_id: Option<String>,
    turn_closed: bool,
    agent: String,
    context: Collector,
    activity_call_id: Option<String>,
    product: Option<String>,
    offset: u64,
    identity: Option<String>,
    activity: Option<NativeActivity>,
    modified: std::time::SystemTime,
    changed_at: u128,
    file_identity: (u64, u64),
}

fn public_message_id(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|id| {
        !id.is_empty()
            && id.len() <= 512
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b":._-+".contains(&b))
    })
}
fn changed_at(metadata: &std::fs::Metadata) -> u128 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (i128::from(metadata.ctime()) * 1_000_000_000 + i128::from(metadata.ctime_nsec())) as u128
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        0
    }
}
fn serialize_message_id<S: serde::Serializer>(
    value: &Option<String>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    public_message_id(value).serialize(serializer)
}
pub(super) fn message_key(activity: &NativeActivity) -> String {
    format!(
        "{}:message:{}",
        activity.session_id,
        public_message_id(&activity.turn_id)
            .or(activity.message_started_at.as_deref())
            .unwrap_or("unknown")
    )
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionContextSample {
    pub session_id: String,
    pub context: Option<ContextUsage>,
    pub state: &'static str,
    pub checked_at: Option<String>,
}
#[derive(Default)]
pub struct NativeWatch {
    input_cache: InputCache,
    source_epochs: HashMap<String, u64>,
    source_revisions: HashMap<String, u64>,
    database_recent: HashMap<String, NativeActivity>,
    #[cfg(target_os = "macos")]
    runtime_evidence: super::codex_runtime::RuntimeEvidence,
    tails: HashMap<PathBuf, Tail>,
    pub checked_at: Option<String>,
    pub error: Option<String>,
}
impl NativeWatch {
    pub fn with_cache_directory(directory: PathBuf) -> Self {
        Self {
            input_cache: InputCache::open(directory.join("native-activity-inputs.json")),
            ..Default::default()
        }
    }
    pub fn forget_inputs(&mut self, agent: &str) {
        self.input_cache.retain(|row| row.agent != agent);
        if let Err(message) = self.input_cache.commit() {
            self.error = Some(message);
        }
    }
    fn trim_database_recent(&mut self, now: DateTime<Utc>) {
        self.database_recent.retain(|_, activity| {
            DateTime::parse_from_rfc3339(&activity.event_at).is_ok_and(|at| {
                (0..=86_400_000).contains(&now.signed_duration_since(at).num_milliseconds())
            })
        });
        let mut ordered: Vec<_> = self.database_recent.values().collect();
        ordered.sort_by(|a, b| b.event_at.cmp(&a.event_at).then_with(|| a.id.cmp(&b.id)));
        let mut counts = HashMap::new();
        let keep: HashSet<_> = ordered
            .into_iter()
            .filter(|activity| {
                let count = counts.entry(&activity.agent_id).or_insert(0);
                *count += 1;
                *count <= 128
            })
            .map(|activity| activity.id.clone())
            .collect();
        self.database_recent.retain(|id, _| keep.contains(id));
    }
    fn canonical_tails(&self) -> HashMap<String, (&PathBuf, &Tail)> {
        let mut rows: HashMap<String, (&PathBuf, &Tail)> = HashMap::new();
        for (path, tail) in &self.tails {
            let Some(identity) = &tail.identity else {
                continue;
            };
            let id = format!("{}:{identity}", tail.agent);
            if rows.get(&id).is_none_or(|(old_path, old)| {
                (tail.modified, tail.offset, path) > (old.modified, old.offset, *old_path)
            }) {
                rows.insert(id, (path, tail));
            }
        }
        rows
    }
    pub fn poll(&mut self, sessions: &NativeSessions) -> Vec<NativeActivity> {
        let scopes = match super::domain_activity::scopes(sessions) {
            Ok(scopes) => scopes,
            Err(_) => {
                self.error = Some("原生采集状态不可用".into());
                return vec![];
            }
        };
        for scope in &scopes {
            if self
                .source_epochs
                .get(&scope.source_id)
                .is_some_and(|epoch| *epoch != scope.lease.epoch)
            {
                self.tails.retain(|_, tail| tail.agent != scope.source_id);
                self.database_recent
                    .retain(|_, row| row.agent_id != scope.source_id);
                self.input_cache.retain(|row| row.agent != scope.source_id);
                #[cfg(target_os = "macos")]
                if scope.source_id == "codex" {
                    self.runtime_evidence = Default::default();
                }
            }
        }
        self.source_epochs = scopes
            .iter()
            .map(|s| (s.source_id.clone(), s.lease.epoch))
            .collect();
        let observed_at = Utc::now().to_rfc3339();
        let started = std::time::Instant::now();
        let rows = self.poll_native(sessions);
        let duration = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        let (rows, failed) = super::domain_activity::publish(
            sessions,
            &scopes,
            rows,
            &observed_at,
            duration,
            self.error.is_some(),
        );
        if failed {
            self.error = Some("部分原生活动来源不可读取".into());
        }
        rows
    }
    fn poll_native(&mut self, sessions: &NativeSessions) -> Vec<NativeActivity> {
        let Ok(sources) = sessions.active_sources() else {
            self.error = Some("原生采集状态不可用".into());
            return vec![];
        };
        self.tails.retain(|_, tail| {
            sources.iter().any(|s| {
                s.enabled
                    && s.agent_id == tail.agent
                    && self.source_revisions.get(&s.agent_id) == Some(&s.authorization_revision)
            })
        });
        self.database_recent.retain(|_, activity| {
            sources.iter().any(|source| {
                source.enabled
                    && source.agent_id == activity.agent_id
                    && self.source_revisions.get(&source.agent_id)
                        == Some(&source.authorization_revision)
            })
        });
        self.input_cache.retain(|row| {
            sources.iter().any(|source| {
                source.enabled
                    && source.agent_id == row.agent
                    && self
                        .source_revisions
                        .get(&source.agent_id)
                        .is_none_or(|revision| *revision == source.authorization_revision)
            })
        });
        self.source_revisions = sources
            .iter()
            .map(|s| (s.agent_id.clone(), s.authorization_revision))
            .collect();
        let Ok(catalog) = sessions.catalog.lock() else {
            return vec![];
        };
        let meta: HashMap<_, _> = super::unique_files(&catalog)
            .into_iter()
            .map(|f| {
                (
                    f.path.clone(),
                    (
                        f.summary.clone(),
                        f.calls.iter().rev().find_map(|c| c.model.clone()),
                        f.calls
                            .iter()
                            .filter(|c| {
                                matches!(
                                    c.agent_attribution,
                                    Some(
                                        "desktop_tool_execution"
                                            | "conflicting_evidence"
                                            | "ambiguous_evidence"
                                    )
                                )
                            })
                            .filter_map(|c| {
                                c.id.strip_prefix(&format!("{}:call:", c.session_id))
                                    .map(|id| {
                                        (
                                            id.to_owned(),
                                            (c.agent_product_id.clone(), c.agent_attribution),
                                        )
                                    })
                            })
                            .collect::<HashMap<_, _>>(),
                    ),
                )
            })
            .collect();
        let indexed_paths: HashSet<_> = catalog.files.keys().cloned().collect();
        let indexed_source_paths = super::unique_files(&catalog)
            .into_iter()
            .map(|file| (file.summary.id.clone(), file.path.clone()))
            .collect::<HashMap<_, _>>();
        let indexed_last = super::unique_files(&catalog)
            .into_iter()
            .filter(|file| {
                !file.activity_messages.is_empty() && super::stamp(&file.path) == Some(file.stamp)
            })
            .filter_map(|file| {
                file.activity_messages
                    .last()
                    .map(|activity| (file.summary.id.clone(), (activity.clone(), file.stamp)))
            })
            .collect::<HashMap<_, _>>();
        let mut history = super::unique_files(&catalog)
            .into_iter()
            .filter(|file| {
                !file.activity_messages.is_empty()
                    && super::stamp(&file.path).is_some_and(|current| {
                        current == file.stamp
                            || (current.0 > file.stamp.0
                                && (current.2, current.3) == (file.stamp.2, file.stamp.3))
                    })
            })
            .flat_map(|file| {
                file.activity_messages
                    .iter()
                    .map(move |activity| (activity, &file.summary))
            })
            .collect::<Vec<_>>();
        history.retain(|(activity, _)| {
            DateTime::parse_from_rfc3339(&activity.event_at).is_ok_and(|at| {
                (0..=86_400_000).contains(&Utc::now().signed_duration_since(at).num_milliseconds())
            })
        });
        history.sort_by(|(a, _), (b, _)| b.event_at.cmp(&a.event_at));
        let mut counts = HashMap::new();
        let indexed_history = history
            .into_iter()
            .filter(|(activity, _)| {
                let count = counts.entry(activity.agent_id.clone()).or_insert(0);
                *count += 1;
                *count <= 128
            })
            .map(|(activity, summary)| {
                let mut activity = activity.clone();
                activity.title = summary.title.clone();
                activity.project = summary.project_name.clone();
                activity
            })
            .collect::<Vec<_>>();
        drop(catalog);
        let mut seen = HashSet::new();
        let mut failed = false;
        for source in sources.iter().filter(|s| s.enabled) {
            let mut paths = vec![];
            let mut skipped = 0;
            for root in sessions.activity_roots(&source.agent_id) {
                if root.exists() && safe_path(&sessions.home, &root) {
                    collect(
                        &root,
                        if source.agent_id == "claude" { 1 } else { 3 },
                        &mut paths,
                        &mut skipped,
                        &mut failed,
                    );
                }
            }
            for path in paths {
                if indexed_paths.contains(&path) && !meta.contains_key(&path) {
                    continue;
                }
                let Ok(m) = std::fs::symlink_metadata(&path) else {
                    failed = true;
                    continue;
                };
                if !m.is_file() || m.len() > 512 * 1024 * 1024 || !safe_path(&sessions.home, &path)
                {
                    continue;
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    if m.nlink() != 1 {
                        failed = true;
                        continue;
                    }
                }
                let Ok(modified) = m.modified() else {
                    continue;
                };
                let changed_at = changed_at(&m);
                #[cfg(unix)]
                let file_identity = {
                    use std::os::unix::fs::MetadataExt;
                    (m.dev(), m.ino())
                };
                #[cfg(not(unix))]
                let file_identity = (0, 0);
                // Old history belongs to the index, not the live activity surface.
                if modified.elapsed().is_ok_and(|age| age.as_secs() > 86400) {
                    continue;
                }
                seen.insert(path.clone());
                let summary = meta.get(&path).map(|(summary, _, _)| summary);
                let tail = self.tails.entry(path.clone()).or_insert_with(|| Tail {
                    message_usages: HashMap::new(),
                    message_started_at: None,
                    user_input: None,
                    tool_calls: HashMap::new(),
                    turn_id: None,
                    exec_turn_id: None,
                    turn_closed: false,
                    agent: source.agent_id.clone(),
                    context: Collector::new(false),
                    activity_call_id: None,
                    product: None,
                    offset: 0,
                    identity: summary.map(|s| s.native_id.clone()),
                    activity: None,
                    modified: std::time::UNIX_EPOCH,
                    changed_at,
                    file_identity,
                });
                if tail.identity.is_none()
                    && let Some(summary) = summary
                {
                    tail.identity = Some(summary.native_id.clone());
                    tail.offset = 0;
                }
                // A completed index can supply labels without another native
                // event. Keep event identity, timing and response model intact.
                if let (Some(activity), Some(summary)) = (&mut tail.activity, summary)
                    && activity.session_id == summary.id
                {
                    activity.title = summary.title.clone();
                    activity.project = summary.project_name.clone();
                    if source.agent_id != "claude" {
                        activity.model = meta.get(&path).and_then(|(_, model, _)| model.clone());
                    }
                }
                if tail.offset == m.len()
                    && tail.modified == modified
                    && tail.changed_at == changed_at
                    && tail.file_identity == file_identity
                {
                    continue;
                }
                let reset = tail.file_identity != file_identity
                    || tail.offset > m.len()
                    || (tail.offset == m.len()
                        && (tail.modified != modified || tail.changed_at != changed_at));
                if reset {
                    tail.message_usages.clear();
                    if let Some(id) = &tail.identity {
                        let session = format!("{}:{id}", tail.agent);
                        self.database_recent.retain(|_, a| a.session_id != session);
                        self.input_cache
                            .retain(|row| row.session_id.as_ref() != Some(&session));
                    }
                    tail.message_started_at = None;
                    tail.user_input = None;
                    tail.tool_calls.clear();
                    tail.file_identity = file_identity;
                    tail.identity = None;
                    tail.activity_call_id = None;
                    tail.offset = 0;
                    tail.activity = None;
                    tail.context = Collector::new(false);
                    tail.turn_id = None;
                    tail.exec_turn_id = None;
                    tail.turn_closed = false;
                }
                let mut options = OpenOptions::new();
                options.read(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.custom_flags(libc::O_NOFOLLOW);
                }
                let Ok(mut file) = options.open(&path) else {
                    failed = true;
                    continue;
                };
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    if file.metadata().is_ok_and(|m| m.nlink() > 1) {
                        continue;
                    }
                }
                // Long transcripts can be tailed before the full index is ready.
                // Read only the bounded identity header, never its client as a
                // claim about later execution in a shared Session.
                if source.agent_id == "codex" && tail.identity.is_none() {
                    let mut header = Vec::new();
                    let read =
                        BufReader::new(std::io::Read::by_ref(&mut file).take(256 * 1024 + 1))
                            .read_until(b'\n', &mut header);
                    if read.is_ok()
                        && header.len() <= 256 * 1024
                        && header.ends_with(b"\n")
                        && let Ok(v) = serde_json::from_slice::<Value>(&header)
                        && v["type"] == "session_meta"
                        && let Some(id) = v["payload"]["id"].as_str()
                        && !id.is_empty()
                        && id.len() <= 512
                        && id
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
                    {
                        tail.identity = Some(id.into());
                    }
                }
                let mut offset = tail.offset;
                // Limit catch-up per file; a skipped prefix is explicitly not a live event feed.
                let skip = offset == 0 && m.len() > 2 * 1024 * 1024
                    || m.len().saturating_sub(offset) > 8 * 1024 * 1024;
                if skip {
                    if let Some(id) = &tail.identity {
                        let session = format!("{}:{id}", tail.agent);
                        self.database_recent.retain(|_, a| a.session_id != session);
                    }
                    tail.message_started_at = None;
                    tail.user_input = None;
                    tail.tool_calls.clear();
                    tail.exec_turn_id = None;
                    offset = m.len().saturating_sub(2 * 1024 * 1024);
                }
                if file.seek(SeekFrom::Start(offset)).is_err() {
                    failed = true;
                    continue;
                }
                let mut reader = BufReader::new(file.take(m.len() - offset));
                let mut line = vec![];
                if skip {
                    let _ = reader.read_until(b'\n', &mut line);
                    offset += line.len() as u64;
                }
                loop {
                    line.clear();
                    let Ok(n) = std::io::Read::by_ref(&mut reader)
                        .take(8 * 1024 * 1024 + 1)
                        .read_until(b'\n', &mut line)
                    else {
                        failed = true;
                        break;
                    };
                    if n == 0 || !line.ends_with(b"\n") {
                        break;
                    }
                    offset += n as u64;
                    if n > 8 * 1024 * 1024 {
                        failed = true;
                        break;
                    }
                    let Ok(v) = serde_json::from_slice::<Value>(&line) else {
                        let session = tail
                            .identity
                            .as_ref()
                            .map(|id| format!("{}:{id}", tail.agent));
                        self.database_recent
                            .retain(|_, a| Some(&a.session_id) != session.as_ref());
                        tail.message_started_at = None;
                        tail.user_input = None;
                        tail.tool_calls.clear();
                        continue;
                    };
                    let native = if source.agent_id == "claude" {
                        v["sessionId"].as_str()
                    } else if v["type"] == "session_meta" {
                        v["payload"]["id"].as_str()
                    } else {
                        None
                    };
                    if let Some(id) = native {
                        if tail.identity.as_deref().is_some_and(|old| old != id) {
                            continue;
                        }
                        tail.identity = Some(id.into());
                    }
                    let Some(id) = tail.identity.clone() else {
                        continue;
                    };
                    let ts = v["timestamp"]
                        .as_str()
                        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
                        .map(|t| t.with_timezone(&Utc));
                    if ts.is_some_and(|ts| ts > Utc::now() + chrono::Duration::minutes(5)) {
                        continue;
                    }
                    let p = &v["payload"];
                    if source.agent_id == "codex" && v["type"] == "token_usage_record" {
                        if let Some(turn) = p["turn_id"].as_str()
                            && let Some(usage) = MessageUsage::codex(&v, &id, turn)
                        {
                            if tail.message_usages.len() >= 128
                                && !tail.message_usages.contains_key(turn)
                            {
                                let oldest = tail
                                    .message_usages
                                    .iter()
                                    .min_by_key(|(_, value)| &value.at)
                                    .map(|(id, _)| id.clone());
                                if let Some(oldest) = oldest {
                                    tail.message_usages.remove(&oldest);
                                }
                            }
                            let mut previous = tail.message_usages.get(turn).cloned();
                            MessageUsage::merge(&mut previous, Some(usage.clone()));
                            if let Some(previous) = previous {
                                tail.message_usages.insert(turn.into(), previous);
                            }
                            for activity in self
                                .database_recent
                                .values_mut()
                                .chain(tail.activity.iter_mut())
                            {
                                if activity.session_id == format!("codex:{id}")
                                    && activity.message_id() == Some(turn)
                                {
                                    MessageUsage::merge(
                                        &mut activity.message_usage,
                                        Some(usage.clone()),
                                    );
                                }
                            }
                        }
                        // Accounting never creates a new row or advances task activity time.
                        continue;
                    }
                    let declared_client = if v["type"] == "session_meta" {
                        None
                    } else {
                        crate::agents::native_client(&source.agent_id, &v)
                    };
                    tail.product = declared_client.clone().flatten();
                    tail.context.observe(
                        &source.agent_id,
                        &v,
                        ts.as_ref().map(|_| v["timestamp"].as_str().unwrap()),
                        0,
                        tail.product.as_deref(),
                    );
                    let Some(ts) = ts else { continue };
                    if tail
                        .activity
                        .as_ref()
                        .is_some_and(|a| a.event_at > ts.to_rfc3339())
                    {
                        continue;
                    }
                    if source.agent_id == "codex"
                        && v["type"] == "event_msg"
                        && p["type"] == "task_started"
                        && p["turn_id"]
                            .as_str()
                            .is_none_or(|id| Some(id) != tail.turn_id.as_deref())
                    {
                        tail.message_started_at = Some(ts.to_rfc3339());
                        tail.user_input = None;
                        tail.tool_calls.clear();
                    }
                    let input = if source.agent_id == "claude"
                        && v["type"] == "user"
                        && !v["message"]["content"]
                            .as_array()
                            .is_some_and(|parts| parts.iter().any(|p| p["type"] == "tool_result"))
                    {
                        activity_input::preview(&v["message"]["content"])
                    } else if source.agent_id == "codex" {
                        activity_input::codex(
                            &v,
                            p["turn_id"]
                                .as_str()
                                .or_else(|| {
                                    p["internal_chat_message_metadata_passthrough"]["turn_id"]
                                        .as_str()
                                })
                                .or(tail.turn_id.as_deref()),
                        )
                    } else {
                        None
                    };
                    let has_input = input.is_some();
                    if let Some(text) = input
                        && tail
                            .user_input
                            .as_ref()
                            .is_none_or(|old| old.at <= ts.to_rfc3339())
                    {
                        if source.agent_id == "claude" {
                            let message = v["uuid"]
                                .as_str()
                                .filter(|id| !id.is_empty() && id.len() <= 512)
                                .map(str::to_owned)
                                .unwrap_or_else(|| ts.to_rfc3339());
                            if tail.turn_id.as_ref() != Some(&message) {
                                tail.message_started_at = Some(ts.to_rfc3339());
                            }
                            tail.turn_id = Some(message);
                        } else {
                            if let Some(turn) = p["turn_id"].as_str().or_else(|| {
                                p["internal_chat_message_metadata_passthrough"]["turn_id"].as_str()
                            }) && tail.turn_id.as_deref() != Some(turn)
                            {
                                tail.turn_id = Some(turn.into());
                                tail.message_started_at = Some(ts.to_rfc3339());
                                tail.turn_closed = false;
                            }
                            if tail.message_started_at.is_none() {
                                tail.message_started_at = Some(ts.to_rfc3339());
                            }
                        }
                        tail.user_input = Some(UserInput {
                            text,
                            at: ts.to_rfc3339(),
                        });
                        tail.tool_calls.clear();
                    }
                    let mut tool_names = Vec::new();
                    let mut remember = |id: &str, name: &str| {
                        let name = super::parser::safe_text(name, 100).0;
                        if !name.is_empty() && !tool_names.contains(&name) {
                            tool_names.push(name.clone());
                        }
                        if !id.is_empty() && id.len() <= 512 {
                            if tail.tool_calls.len() >= 32 {
                                tail.tool_calls.clear();
                            }
                            tail.tool_calls.insert(id.into(), name);
                        }
                    };
                    if source.agent_id == "codex"
                        && matches!(
                            p["type"].as_str(),
                            Some("function_call" | "custom_tool_call")
                        )
                    {
                        remember(
                            p["call_id"].as_str().unwrap_or(""),
                            p["name"].as_str().unwrap_or("工具"),
                        );
                    } else if source.agent_id == "claude"
                        && v["type"] == "assistant"
                        && v["message"]["model"] != "<synthetic>"
                        && let Some(parts) = v["message"]["content"].as_array()
                    {
                        for part in parts.iter().filter(|p| p["type"] == "tool_use").take(8) {
                            remember(
                                part["id"].as_str().unwrap_or(""),
                                part["name"].as_str().unwrap_or("工具"),
                            );
                        }
                    }
                    if tool_names.is_empty() {
                        let ids = if source.agent_id == "claude" {
                            v["message"]["content"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(|p| p["tool_use_id"].as_str())
                                .take(8)
                                .collect::<Vec<_>>()
                        } else {
                            vec![
                                p["call_id"]
                                    .as_str()
                                    .or_else(|| p["item"]["id"].as_str())
                                    .unwrap_or(""),
                            ]
                        };
                        for id in ids {
                            if let Some(name) = tail.tool_calls.remove(id)
                                && !tool_names.contains(&name)
                            {
                                tool_names.push(name);
                            }
                        }
                    }
                    // Exec identifies this turn, unlike the Session creator. Keep the
                    // proof in the live tail; context and history retain their own identity.
                    let mut exec_evidence = false;
                    if source.agent_id == "codex"
                        && tail
                            .activity
                            .as_ref()
                            .is_none_or(|a| a.event_at <= ts.to_rfc3339())
                    {
                        let turn = p["turn_id"].as_str().or_else(|| {
                            p["internal_chat_message_metadata_passthrough"]["turn_id"].as_str()
                        });
                        if turn.is_some_and(|id| tail.exec_turn_id.as_deref() != Some(id)) {
                            tail.exec_turn_id = None;
                        }
                        if v["type"] == "event_msg" && p["type"] == "task_started" {
                            tail.exec_turn_id = exec_turn_id(p).map(str::to_owned);
                        }
                        exec_evidence = tail.exec_turn_id.is_some()
                            && tail.exec_turn_id.as_deref() == turn.or(tail.turn_id.as_deref());
                        if declared_client
                            .as_ref()
                            .is_some_and(|id| id.as_deref() != Some("codex-cli"))
                        {
                            tail.exec_turn_id = None;
                        }
                    }
                    let phase = if source.agent_id == "codex" {
                        match (
                            v["type"].as_str().unwrap_or(""),
                            p["type"].as_str().unwrap_or(""),
                        ) {
                            _ if has_input => Some(("processing", "收到原生消息输入".into())),
                            ("event_msg", "task_started") => {
                                Some(("processing", "本轮任务已开始".to_string()))
                            }
                            ("event_msg", "error" | "stream_error" | "turn_failed") => Some((
                                "error",
                                "原生调用报告异常；详情见会话，最终结果以原生记录为准".into(),
                            )),
                            ("event_msg", "task_complete") if !p["error"].is_null() => {
                                Some(("error", "本轮未成功完成".into()))
                            }
                            ("event_msg", "task_complete") => {
                                Some(("waiting_input", "本轮已完成，等待输入".into()))
                            }
                            ("event_msg", "turn_aborted") => {
                                Some(("cancelled", "本轮已中止".into()))
                            }
                            ("event_msg", "item_completed")
                                if [
                                    "CommandExecution",
                                    "McpToolCall",
                                    "FileChange",
                                    "Extension",
                                ]
                                .contains(&p["item"]["type"].as_str().unwrap_or("")) =>
                            {
                                Some((
                                    "tool_returned",
                                    format!(
                                        "{}已返回",
                                        p["item"]["type"].as_str().unwrap_or("工具")
                                    ),
                                ))
                            }
                            ("event_msg", "item_completed") if p["item"]["type"] == "Reasoning" => {
                                Some(("thinking", "收到原生推理进度".into()))
                            }
                            ("response_item", "reasoning") => {
                                Some(("thinking", "收到原生推理进度".into()))
                            }
                            ("event_msg", "item_completed")
                                if p["item"]["type"] == "AgentMessage"
                                    && p["item"]["phase"] == "commentary" =>
                            {
                                Some(("processing", "已记录进度回复".into()))
                            }
                            ("response_item", "message")
                                if p["role"] == "assistant" && p["phase"] == "commentary" =>
                            {
                                Some(("processing", "已记录进度回复".into()))
                            }
                            ("response_item", "function_call" | "custom_tool_call") => Some((
                                "tool_requested",
                                format!("已提出调用：{}", p["name"].as_str().unwrap_or("工具")),
                            )),
                            (
                                "response_item",
                                "function_call_output" | "custom_tool_call_output",
                            ) => Some(("tool_returned", "工具结果已记录".into())),
                            ("response_item", "message") if p["role"] == "assistant" => {
                                Some(("reply_recorded", "收到模型回复".into()))
                            }
                            _ => None,
                        }
                    } else {
                        let content = &v["message"]["content"];
                        let tool = content
                            .as_array()
                            .and_then(|a| a.iter().find(|c| c["type"] == "tool_use"));
                        if v["isApiErrorMessage"] == true
                            || (v["type"] == "system" && v["subtype"] == "api_error")
                        {
                            Some(("error", "原生 API 调用报告异常；详情见会话".into()))
                        } else if v["type"] == "user"
                            && v["interruptedMessageId"]
                                .as_str()
                                .is_some_and(|id| !id.is_empty())
                        {
                            Some(("cancelled", "本轮已中止".into()))
                        } else if v["type"] == "assistant" && v["message"]["model"] == "<synthetic>"
                        {
                            None
                        } else if v["type"] == "assistant" {
                            Some(if tool.is_some() {
                                (
                                    "tool_requested",
                                    format!("已提出调用：{}", tool_names.join("、")),
                                )
                            } else if content.as_array().is_some_and(|parts| {
                                parts.iter().any(|p| p["type"] == "thinking")
                                    && !parts.iter().any(|p| {
                                        p["type"] == "text"
                                            && p["text"]
                                                .as_str()
                                                .is_some_and(|s| !s.trim().is_empty())
                                    })
                            }) {
                                ("thinking", "收到原生思考进度".into())
                            } else {
                                ("reply_recorded", "收到模型回复".into())
                            })
                        } else if v["type"] == "user"
                            && content
                                .as_array()
                                .is_some_and(|a| a.iter().any(|c| c["type"] == "tool_result"))
                        {
                            Some((
                                "tool_returned",
                                if tool_names.is_empty() {
                                    "工具结果已记录".into()
                                } else {
                                    format!("工具已返回：{}", tool_names.join("、"))
                                },
                            ))
                        } else if v["type"] == "user" {
                            Some(("processing", "收到新的用户消息".into()))
                        } else if v["type"] == "system" && v["subtype"] == "turn_duration" {
                            Some(("waiting_input", "本轮已完成，等待输入".into()))
                        } else {
                            None
                        }
                    };
                    if let Some((phase, mut step)) = phase {
                        let mut pending = tail.tool_calls.values().cloned().collect::<Vec<_>>();
                        pending.sort();
                        pending.dedup();
                        pending.truncate(8);
                        if phase == "tool_requested" {
                            for name in pending {
                                if !tool_names.contains(&name) && tool_names.len() < 8 {
                                    tool_names.push(name);
                                }
                            }
                            if !tool_names.is_empty() {
                                step = format!("已提出调用：{}", tool_names.join("、"));
                            }
                        } else if phase == "tool_returned" && !pending.is_empty() {
                            step = format!(
                                "{}；仍待返回：{}",
                                if tool_names.is_empty() {
                                    "工具结果已记录".into()
                                } else {
                                    format!("{} 已返回", tool_names.join("、"))
                                },
                                pending.join("、")
                            );
                        }
                        let previous = tail.activity.as_ref();
                        if previous.is_some_and(|a| a.event_at > ts.to_rfc3339()) {
                            continue;
                        }
                        if source.agent_id == "codex" {
                            let turn = p["turn_id"].as_str().or_else(|| {
                                p["internal_chat_message_metadata_passthrough"]["turn_id"].as_str()
                            });
                            let terminal = ["waiting_input", "cancelled", "error"].contains(&phase);
                            if !terminal
                                && tail.turn_closed
                                && (turn.is_none() || turn == tail.turn_id.as_deref())
                            {
                                tail.tool_calls.clear();
                                continue;
                            }
                            if let Some(turn) = turn {
                                // A late snapshot may first see a later turn in
                                // the middle of a step. Its start is unknown;
                                // never promote the step timestamp to a message.
                                if tail.turn_id.as_deref().is_some_and(|old| old != turn)
                                    && p["type"] != "task_started"
                                {
                                    tail.message_started_at = None;
                                    tail.user_input = None;
                                }
                                tail.turn_id = Some(turn.into());
                            }
                            tail.turn_closed = terminal;
                        }
                        if ["waiting_input", "cancelled", "error"].contains(&phase) {
                            tail.tool_calls.clear();
                        }
                        let ttft = if p["type"] == "task_complete" && p["error"].is_null() {
                            p["time_to_first_token_ms"]
                                .as_f64()
                                .filter(|v| v.is_finite() && *v >= 0.0)
                        } else {
                            None
                        };
                        let model = if source.agent_id == "claude" {
                            if ["processing", "error", "cancelled"].contains(&phase) {
                                // The next turn's model is not established by its
                                // user message or a synthetic API error response.
                                None
                            } else {
                                v["message"]["model"]
                                    .as_str()
                                    .filter(|model| !model.is_empty() && model.len() <= 250)
                                    .map(|model| super::sanitize_label(model, 250))
                                    .or_else(|| previous.and_then(|a| a.model.clone()))
                            }
                        } else {
                            meta.get(&path)
                                .and_then(|(_, model, _)| model.clone())
                                .or_else(|| previous.and_then(|a| a.model.clone()))
                        };
                        tail.activity_call_id = p["call_id"]
                            .as_str()
                            .or_else(|| p["item"]["id"].as_str())
                            .map(str::to_owned);
                        tail.activity = Some(NativeActivity {
                            message_usage: tail
                                .turn_id
                                .as_ref()
                                .and_then(|id| tail.message_usages.get(id))
                                .cloned(),
                            turn_id: tail.turn_id.clone(),
                            #[cfg(any(target_os = "macos", test))]
                            call_id: tail.activity_call_id.clone(),
                            agent_product_id: if exec_evidence && declared_client.is_none() {
                                Some("codex-cli".into())
                            } else if exec_evidence
                                && tail.product.as_deref() == Some("codex-desktop")
                            {
                                None
                            } else {
                                tail.product.clone()
                            },
                            agent_attribution: if source.agent_id == "claude" {
                                crate::agents::claude_client_evidence(&v).map(str::to_owned)
                            } else if declared_client == Some(None) {
                                Some("unrecognized_client".into())
                            } else if exec_evidence
                                && tail.product.as_deref() == Some("codex-desktop")
                            {
                                Some("conflicting_evidence".into())
                            } else if exec_evidence && declared_client.is_none() {
                                Some("native_exec_turn".into())
                            } else {
                                tail.product.as_ref().map(|_| "native_record".into())
                            },
                            id: format!("{}:{id}", source.agent_id),
                            session_id: format!("{}:{id}", source.agent_id),
                            agent_id: source.agent_id.clone(),
                            title: summary.map(|s| s.title.clone()).unwrap_or_else(|| {
                                previous.map_or_else(
                                    || "新会话 · 标题索引中".into(),
                                    |a| a.title.clone(),
                                )
                            }),
                            project: summary
                                .and_then(|s| s.project_name.clone())
                                .or_else(|| previous.and_then(|a| a.project.clone())),
                            model,
                            phase: phase.into(),
                            step: super::parser::safe_text(&step, 160).0,
                            user_input: tail.user_input.clone(),
                            tool_names,
                            message_started_at: tail.message_started_at.clone(),
                            event_at: ts.to_rfc3339(),
                            turn_ttft_ms: ttft,
                            realtime: None,
                            stale: false,
                        });
                        // Preserve every observed message while a cold tail is
                        // replayed, not just the final message in the file.
                        if let Some(activity) = tail.activity.as_ref()
                            && (public_message_id(&activity.turn_id).is_some()
                                || activity.message_started_at.is_some())
                        {
                            let mut saved = activity.clone();
                            saved.id = message_key(&saved);
                            self.database_recent.insert(saved.id.clone(), saved);
                            if self.database_recent.len() > 512
                                && let Some(oldest) = self
                                    .database_recent
                                    .values()
                                    .min_by_key(|a| &a.event_at)
                                    .map(|a| a.id.clone())
                            {
                                self.database_recent.remove(&oldest);
                            }
                        }
                    }
                }
                tail.offset = offset;
                tail.modified = modified;
                tail.changed_at = changed_at;
            }
        }
        self.tails.retain(|path, _| seen.contains(path));
        let canonical_sources = self
            .canonical_tails()
            .into_iter()
            .map(|(session, (path, _))| (session, path.clone()))
            .collect::<HashMap<_, _>>();
        let valid_sessions = canonical_sources.keys().cloned().collect::<HashSet<_>>();
        self.database_recent.retain(|_, a| {
            !["claude", "codex"].contains(&a.agent_id.as_str())
                || valid_sessions.contains(&a.session_id)
        });
        self.checked_at = Some(Utc::now().to_rfc3339());
        let mut activities: HashMap<String, NativeActivity> = HashMap::new();
        for (path, tail) in self.canonical_tails().into_values() {
            let Some(activity) = tail.activity.as_ref() else {
                continue;
            };
            let mut a = activity.clone();
            a.user_input = tail.user_input.clone();
            a.message_started_at = tail.message_started_at.clone();
            if let Some(indexed) = indexed_last
                .get(&a.session_id)
                .filter(|(history, stamp)| {
                    history.event_at == a.event_at
                        && stamp.0 == tail.offset
                        && Some(stamp.1)
                            == tail
                                .modified
                                .duration_since(std::time::UNIX_EPOCH)
                                .ok()
                                .map(|age| age.as_nanos())
                        && (stamp.2, stamp.3) == tail.file_identity
                        && stamp.4 == tail.changed_at
                })
                .map(|(history, _)| history)
                && ((a.turn_id.is_none() && a.message_started_at.is_none())
                    || (a.turn_id.is_some() && a.turn_id == indexed.turn_id))
            {
                // The full index and tail describe the same unchanged file snapshot
                // and its last business event, so its message identity is explicit.
                a.turn_id = a.turn_id.or_else(|| indexed.turn_id.clone());
                a.message_started_at = a
                    .message_started_at
                    .or_else(|| indexed.message_started_at.clone());
                a.user_input = a.user_input.or_else(|| indexed.user_input.clone());
            }
            if let Some((product, attribution)) = tail
                .activity_call_id
                .as_ref()
                .and_then(|id| meta.get(path).and_then(|(_, _, calls)| calls.get(id)))
            {
                let conflict = a.agent_product_id.is_some()
                    && product.is_some()
                    && a.agent_product_id != *product;
                a.agent_product_id = if conflict { None } else { product.clone() };
                a.agent_attribution = if conflict {
                    Some("conflicting_evidence".into())
                } else {
                    attribution.map(str::to_owned)
                };
            }
            a.stale = DateTime::parse_from_rfc3339(&a.event_at)
                .is_ok_and(|t| Utc::now().signed_duration_since(t).num_seconds() > 30)
                && !["waiting_input", "cancelled"].contains(&a.phase.as_str());
            a.id = message_key(&a);
            if activities
                .get(&a.id)
                .is_none_or(|old| old.event_at < a.event_at)
            {
                activities.insert(a.id.clone(), a);
            }
        }
        let mut current_ids = activities
            .values()
            .map(|row| (row.session_id.clone(), row.id.clone()))
            .collect::<HashMap<_, _>>();
        self.database_recent.retain(|_, row| {
            row.turn_id.is_some()
                || row.message_started_at.is_some()
                || !indexed_last.contains_key(&row.session_id)
        });
        for mut history in indexed_history {
            if canonical_sources.get(&history.session_id)
                != indexed_source_paths.get(&history.session_id)
            {
                continue;
            }
            history.id = message_key(&history);
            let current = activities
                .entry(history.id.clone())
                .or_insert_with(|| history.clone());
            MessageUsage::merge(&mut current.message_usage, history.message_usage.clone());
            if history.user_input.as_ref().is_some_and(|input| {
                current
                    .user_input
                    .as_ref()
                    .is_none_or(|old| old.at <= input.at)
            }) {
                current.user_input = history.user_input;
            }
            current.message_started_at = current
                .message_started_at
                .clone()
                .or(history.message_started_at);
            current.model = current.model.clone().or(history.model);
            current.turn_ttft_ms = current.turn_ttft_ms.or(history.turn_ttft_ms);
        }
        for activity in activities.values() {
            for old in self
                .database_recent
                .values_mut()
                .filter(|old| old.session_id == activity.session_id)
            {
                old.title = activity.title.clone();
                old.project = activity.project.clone();
            }
            self.database_recent
                .insert(activity.id.clone(), activity.clone());
        }
        self.error = failed.then(|| "部分原生活动来源不可读取".into());
        for source in sources.iter().filter(|s| s.enabled) {
            match sessions.database_activities(source) {
                Ok(Some((rows, limited))) => {
                    if source.agent_id == "opencode" && !limited {
                        let current = rows.iter().map(message_key).collect::<HashSet<_>>();
                        self.database_recent
                            .retain(|id, a| a.agent_id != "opencode" || current.contains(id));
                    }
                    if limited {
                        self.error = Some(
                            if source.agent_id == "antigravity" {
                                "部分 Antigravity 活动来源不可读取或超限"
                            } else {
                                "部分原生活动来源不可读取"
                            }
                            .into(),
                        );
                    }
                    for mut activity in rows {
                        activity.id = message_key(&activity);
                        if activity.stale
                            && activity.phase == "processing"
                            && let Some(old) = self.database_recent.get(&activity.id)
                            && old.event_at > activity.event_at
                        {
                            let input = activity.user_input.clone();
                            activity = old.clone();
                            activity.stale = true;
                            activity.user_input = input.or(activity.user_input);
                        }
                        self.database_recent
                            .insert(activity.id.clone(), activity.clone());
                        activities.insert(activity.id.clone(), activity);
                    }
                }
                Ok(None) => {}
                Err(_) => {
                    self.error = Some(
                        if source.agent_id == "antigravity" {
                            "Antigravity 活动来源暂不可读取"
                        } else {
                            "部分原生活动来源不可读取"
                        }
                        .into(),
                    );
                }
            }
        }
        self.trim_database_recent(Utc::now());
        for activity in self.database_recent.values() {
            activities.entry(activity.id.clone()).or_insert_with(|| {
                // Some databases only expose running steps. Absence is not
                // completion evidence: retain metadata with its original time.
                let mut retained = activity.clone();
                retained.stale =
                    !["waiting_input", "cancelled", "error"].contains(&retained.phase.as_str());
                retained
            });
        }
        let mut rows: Vec<_> = activities.into_values().collect();
        rows.sort_by(|a, b| b.event_at.cmp(&a.event_at));
        #[cfg(target_os = "macos")]
        {
            if !sources.iter().any(|s| s.enabled && s.agent_id == "codex") {
                self.runtime_evidence = Default::default();
            }
            super::codex_runtime::attribute(&sessions.home, &mut rows, &mut self.runtime_evidence);
        }
        for row in &rows {
            self.database_recent.insert(row.id.clone(), row.clone());
        }
        let latest_sources = sessions.active_sources().unwrap_or_default();
        rows.retain(|row| {
            latest_sources.iter().any(|s| {
                s.enabled
                    && s.agent_id == row.agent_id
                    && self.source_revisions.get(&s.agent_id) == Some(&s.authorization_revision)
            })
        });
        self.tails.retain(|_, tail| {
            latest_sources.iter().any(|s| {
                s.enabled
                    && s.agent_id == tail.agent
                    && self.source_revisions.get(&s.agent_id) == Some(&s.authorization_revision)
            })
        });
        self.database_recent.retain(|_, activity| {
            latest_sources.iter().any(|source| {
                source.enabled
                    && source.agent_id == activity.agent_id
                    && self.source_revisions.get(&source.agent_id)
                        == Some(&source.authorization_revision)
            })
        });
        let input_stamps: HashMap<_, _> = self
            .canonical_tails()
            .into_iter()
            .map(|(session, (_, tail))| {
                let stamp = format!(
                    "{}:{}:{}:{}:{}",
                    tail.file_identity.0,
                    tail.file_identity.1,
                    tail.modified
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos(),
                    tail.offset,
                    tail.changed_at
                );
                (session, stamp)
            })
            .collect();
        self.input_cache.retain(|row| {
            latest_sources
                .iter()
                .any(|s| s.enabled && s.agent_id == row.agent)
                && row
                    .session_id
                    .as_ref()
                    .is_some_and(|id| input_stamps.get(id) == row.source_stamp.as_ref())
        });
        for row in &mut rows {
            if row.turn_id.is_none()
                && row.message_started_at.is_none()
                && let Some(saved) = self.input_cache.values().find(|saved| {
                    saved.session_id.as_ref() == Some(&row.session_id)
                        && saved
                            .native
                            .as_ref()
                            .is_some_and(|native| native.latest && native.event_at == row.event_at)
                })
                && let Some(native) = &saved.native
            {
                row.turn_id = native.message_id.clone();
                row.message_usage = None;
                row.message_started_at = saved.started_at.clone();
                row.id = message_key(row);
                row.user_input = Some(saved.input.clone());
                row.model = row.model.clone().or(native.model.clone());
                row.turn_ttft_ms = row.turn_ttft_ms.or(native.ttft_ms);
                current_ids.insert(row.session_id.clone(), row.id.clone());
            }
        }
        let cached_bases: HashMap<_, _> = self
            .canonical_tails()
            .into_iter()
            .filter_map(|(session, (_, tail))| {
                tail.activity.clone().map(|activity| (session, activity))
            })
            .collect();
        let mut restored_counts = HashMap::new();
        for row in &rows {
            *restored_counts
                .entry(row.agent_id.clone())
                .or_insert(0_usize) += 1;
        }
        let mut cached_messages = self.input_cache.values().collect::<Vec<_>>();
        cached_messages.sort_by(|a, b| {
            b.native
                .as_ref()
                .map(|row| &row.event_at)
                .cmp(&a.native.as_ref().map(|row| &row.event_at))
                .then_with(|| a.key.cmp(&b.key))
        });
        for saved in cached_messages {
            let Some(native) = &saved.native else {
                continue;
            };
            let Some(mut row) = saved
                .session_id
                .as_ref()
                .and_then(|id| cached_bases.get(id))
                .cloned()
            else {
                continue;
            };
            if row.agent_id != saved.agent
                || restored_counts.get(&row.agent_id).copied().unwrap_or(0) >= 128
                || rows.iter().any(|row| row.id == saved.key)
                || !DateTime::parse_from_rfc3339(&native.event_at).is_ok_and(|at| {
                    (0..=86_400_000)
                        .contains(&Utc::now().signed_duration_since(at).num_milliseconds())
                })
            {
                continue;
            }
            row.turn_id = native.message_id.clone();
            row.message_usage = None;
            row.message_started_at = saved.started_at.clone();
            row.id = message_key(&row);
            if row.id != saved.key {
                continue;
            }
            row.user_input = Some(saved.input.clone());
            row.model = native.model.clone();
            row.phase = native.phase.clone();
            row.stale = !["waiting_input", "cancelled", "error"].contains(&row.phase.as_str());
            row.step = "历史消息已恢复".into();
            row.event_at = native.event_at.clone();
            row.turn_ttft_ms = native.ttft_ms;
            row.tool_names.clear();
            row.agent_product_id = None;
            row.agent_attribution = None;
            #[cfg(any(target_os = "macos", test))]
            {
                row.call_id = None;
            }
            *restored_counts.entry(row.agent_id.clone()).or_default() += 1;
            rows.push(row);
        }
        for row in &mut rows {
            if row.user_input.is_none()
                && let Some(saved) = self.input_cache.get(&row.id)
                && saved.agent == row.agent_id
                && saved.session_id.as_ref() == Some(&row.session_id)
                && saved
                    .source_stamp
                    .as_ref()
                    .is_some_and(|stamp| input_stamps.get(&row.session_id) == Some(stamp))
                && DateTime::parse_from_rfc3339(&saved.input.at).ok()
                    <= DateTime::parse_from_rfc3339(&row.event_at).ok()
            {
                row.user_input = Some(saved.input.clone());
                row.message_started_at =
                    row.message_started_at.clone().or(saved.started_at.clone());
            }
            if let Some(input) = &row.user_input {
                if let Some(stamp) = input_stamps.get(&row.session_id) {
                    self.input_cache.insert(CachedInput {
                        key: row.id.clone(),
                        agent: row.agent_id.clone(),
                        session_id: Some(row.session_id.clone()),
                        started_at: row.message_started_at.clone(),
                        source_stamp: Some(stamp.clone()),
                        native: Some(CachedNative {
                            latest: current_ids.get(&row.session_id) == Some(&row.id),
                            message_id: row.turn_id.clone(),
                            event_at: row.event_at.clone(),
                            phase: row.phase.clone(),
                            model: row.model.clone(),
                            ttft_ms: row.turn_ttft_ms,
                        }),
                        input: input.clone(),
                    });
                }
                self.database_recent.insert(row.id.clone(), row.clone());
            }
        }
        if let Err(message) = self.input_cache.commit() {
            self.error = Some(message);
        } else if self.error.is_none() {
            self.error = self.input_cache.error.clone();
        }
        rows.sort_by(|a, b| b.event_at.cmp(&a.event_at));
        rows
    }

    /// Reuses index metadata and the bounded append-only tail; never reparses a
    /// conversation per overview request. Consent and file changes are rechecked.
    pub fn context_samples(
        &self,
        sessions: &NativeSessions,
        ids: impl IntoIterator<Item = String>,
    ) -> super::NativeResult<Vec<SessionContextSample>> {
        let catalog = sessions.lock()?;
        let files = super::unique_files(&catalog);
        let files: HashMap<_, _> = files
            .into_iter()
            .map(|f| (f.summary.id.as_str(), f))
            .collect();
        let tails = self.canonical_tails();
        let mut ids: Vec<_> = ids
            .into_iter()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        ids.sort();
        Ok(ids
            .into_iter()
            .map(|id| {
                let mut sample = SessionContextSample {
                    session_id: id.clone(),
                    context: None,
                    state: "unknown",
                    checked_at: None,
                };
                let family = id.split(':').next().unwrap_or("");
                let Some(source) = catalog
                    .sources
                    .iter()
                    .find(|s| s.agent_id == family && s.enabled && sessions.source_active(family))
                else {
                    sample.state = "unavailable";
                    return sample;
                };
                if let Some((path, tail)) = tails.get(&id).filter(|(path, _)| {
                    files
                        .get(id.as_str())
                        .is_none_or(|file| file.path == **path)
                }) {
                    let valid = safe_path(&sessions.home, path)
                        && std::fs::symlink_metadata(path)
                            .is_ok_and(|m| m.is_file() && m.modified().ok() == Some(tail.modified));
                    sample.checked_at = self.checked_at.clone();
                    if valid {
                        sample.context = Some(tail.context.data.usage_summary());
                        sample.state = if self.error.is_some()
                            || super::stamp(path).is_some_and(|mark| mark.0 != tail.offset)
                        {
                            "partial"
                        } else {
                            "ready"
                        };
                    } else {
                        sample.state = "pending";
                    }
                    return sample;
                }
                if let Some(file) = files.get(id.as_str()) {
                    sample.checked_at = source.last_scan.clone();
                    if !safe_path(&sessions.home, &file.path) {
                        sample.state = "unavailable";
                    } else if family != "opencode" && super::stamp(&file.path) != Some(file.stamp) {
                        sample.state = "pending";
                    } else {
                        sample.context = Some(file.context.clone());
                        sample.state = if file.summary.partial
                            || source.status == "partial"
                            || source.error.is_some()
                        {
                            "partial"
                        } else {
                            "ready"
                        };
                    }
                }
                sample
            })
            .collect())
    }
}

// Only the exact root exec-turn marker observed in native CLI output is supported.
// Composer, Session originator, subagent triggers and partial metadata are insufficient.
fn exec_turn_id(payload: &Value) -> Option<&str> {
    let id = payload["turn_id"].as_str()?;
    let evidence = &payload["turn_attribution"];
    (!id.is_empty()
        && id.len() <= 512
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        && evidence["turn_trigger"] == "exec"
        && evidence["turn_id"] == id
        && evidence["root_turn_id"] == id
        && payload["root_turn_id"] == id
        && evidence.get("parent_turn_id").is_some_and(Value::is_null)
        && evidence
            .get("initiating_agent_path")
            .is_some_and(Value::is_null))
    .then_some(id)
}

#[cfg(test)]
mod retention_tests {
    use super::*;

    fn activity(agent: &str, id: &str, event_at: String) -> NativeActivity {
        NativeActivity {
            message_usage: None,
            turn_id: None,
            call_id: None,
            agent_product_id: Some(agent.into()),
            agent_attribution: Some("native_record".into()),
            id: format!("{agent}:{id}"),
            agent_id: agent.into(),
            session_id: format!("{agent}:{id}"),
            title: "metadata only".into(),
            project: None,
            model: None,
            phase: "tool_requested".into(),
            step: "正在执行工具".into(),
            user_input: None,
            tool_names: Vec::new(),
            message_started_at: None,
            event_at,
            turn_ttft_ms: None,
            realtime: None,
            stale: false,
        }
    }

    #[test]
    fn database_recent_keeps_newest_128_per_family_without_retiming() {
        let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let mut watch = NativeWatch::default();
        for agent in ["opencode", "antigravity"] {
            for i in 0..140 {
                let a = activity(
                    agent,
                    &i.to_string(),
                    (now - chrono::Duration::seconds(i)).to_rfc3339(),
                );
                watch.database_recent.insert(a.id.clone(), a);
            }
        }
        watch.trim_database_recent(now);
        assert_eq!(watch.database_recent.len(), 256);
        for agent in ["opencode", "antigravity"] {
            for i in 0..140 {
                let row = watch.database_recent.get(&format!("{agent}:{i}"));
                assert_eq!(row.is_some(), i < 128);
                if let Some(row) = row {
                    assert_eq!(
                        row.event_at,
                        (now - chrono::Duration::seconds(i)).to_rfc3339()
                    );
                }
            }
        }
    }

    #[test]
    fn database_recent_expires_old_future_and_invalid_events() {
        let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let mut watch = NativeWatch::default();
        for (id, timestamp) in [
            ("boundary", (now - chrono::Duration::hours(24)).to_rfc3339()),
            (
                "old",
                (now - chrono::Duration::hours(24) - chrono::Duration::milliseconds(1))
                    .to_rfc3339(),
            ),
            ("future", (now + chrono::Duration::seconds(1)).to_rfc3339()),
            ("invalid", "unknown".into()),
        ] {
            let a = activity("opencode", id, timestamp);
            watch.database_recent.insert(a.id.clone(), a);
        }
        watch.trim_database_recent(now);
        assert_eq!(watch.database_recent.len(), 1);
        assert!(watch.database_recent.contains_key("opencode:boundary"));
        watch.trim_database_recent(now + chrono::Duration::milliseconds(1));
        assert!(watch.database_recent.is_empty());
    }
}
