use std::{
    collections::{BTreeMap, VecDeque},
    ffi::{CStr, CString},
    fs::File,
    io::{Read, Seek, SeekFrom},
    os::fd::{AsRawFd, FromRawFd, IntoRawFd},
    os::unix::fs::MetadataExt,
    path::Path,
    sync::Arc,
    time::Instant,
};

use d::facts::{
    ActivityCandidate, ActivityPhase, EvidenceLevel, FactCandidate, FactIdentity, FactKind,
    FactRecord, FactSnapshot, ProjectCandidate, SessionCandidate, publication::object_id,
};
use local_agent_core::{domain as d, plugin::platform::*};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::AppState;

use super::{Checked, Result, Selection, directory, fail, open_store, package, run_sync};

const FILE_LIMIT: u64 = 2 * 1024 * 1024;
const SCAN_LIMIT: u64 = 16 * 1024 * 1024;
const MAX_ENTRIES: usize = 8192;
const ACTIVITY_WINDOW_SECONDS: u32 = 30;
const CONVERSATION_PAGE_SIZE: usize = 32;

pub(in crate::plugin_host::external) struct Snapshot {
    key: String,
    observed: Instant,
    records: BTreeMap<String, MappedSession>,
    facts: FactSnapshot,
    scan_problem: Option<&'static str>,
}

#[derive(Serialize)]
struct MappedSession {
    record: d::SessionRecord,
    model: Option<String>,
    project: Option<ProjectCandidate>,
    activity: Option<ActivityCandidate>,
    body: Option<TranscriptLocation>,
}

#[derive(Serialize)]
struct TranscriptLocation {
    relative_path: String,
    file_hash: String,
    document: usize,
}

pub(crate) struct DirectoryView {
    pub(crate) plugin_id: String,
    pub(crate) digest: String,
    pub(crate) source: d::Source,
    pub(crate) provider: d::Provider,
    pub(crate) product: d::Product,
    snapshot: Option<Arc<Snapshot>>,
    problem: Option<String>,
    last_attempt: Option<String>,
    mapper: Option<Box<SourceMapper>>,
}

impl DirectoryView {
    pub(crate) fn read_status(&self) -> Value {
        serde_json::json!({
            "lastAttempt":self.last_attempt,
            "lastSuccess":self.snapshot.as_ref().map(|s| &s.facts.identity.observed_at),
            "sessions":self.snapshot.as_ref().map(|s|s.records.len()),"problem":self.problem,
        })
    }

    pub(crate) fn vector(&self) -> String {
        serde_json::json!([
            self.digest,
            self.source.status,
            self.problem,
            self.snapshot
                .as_ref()
                .map(|snapshot| (&snapshot.key, &snapshot.facts.source_revision))
        ])
        .to_string()
    }

    pub(crate) fn state(&self) -> d::SourceState {
        d::SourceState {
            source_id: self.source.id.clone(),
            authorization: self.source.status.authorization.clone(),
            freshness: self.source.status.freshness.clone(),
            coverage: if self.snapshot.is_none() {
                d::Coverage::Unavailable
            } else if self.source.status.freshness != d::Freshness::Current
                || self.problem.is_some()
            {
                d::Coverage::Partial
            } else {
                d::Coverage::Complete
            },
            problem: self.problem.clone(),
        }
    }

    pub(crate) fn record(&self, id: &str) -> Option<d::SessionRecord> {
        Some(self.snapshot.as_ref()?.records.get(id)?.record.clone())
    }

    pub(crate) fn facts(&self) -> Option<&FactSnapshot> {
        Some(&self.snapshot.as_ref()?.facts)
    }

    pub(crate) fn capabilities(&self) -> Vec<d::CapabilityEntry> {
        use d::ReadOperation::{CollectActivity, CollectProjectEvidence, ReadConversation, Scan};
        let entry = |name: &str, operations| {
            let mut provider = self.provider.clone();
            provider.capability.name = name.into();
            provider.provider_key = format!("{}:{name}@1:source", self.plugin_id);
            d::CapabilityEntry {
                provider,
                operations,
                status: self.source.status.clone(),
            }
        };
        let mapper = self.mapper.as_deref();
        let mut session = vec![Scan];
        if mapper.is_some_and(|m| m.conversation.is_some()) {
            session.push(ReadConversation);
        }
        let mut entries = vec![entry("domain.session-source", session)];
        if mapper.is_some_and(|m| m.activity.is_some()) {
            entries.push(entry("domain.activity-source", vec![CollectActivity]));
        }
        if mapper.is_some_and(|m| m.project.is_some()) {
            entries.push(entry("domain.project-source", vec![CollectProjectEvidence]));
        }
        entries
    }

    pub(crate) fn live_report_window(&self) -> Option<u32> {
        self.mapper.as_ref()?.activity.as_ref()?;
        Some(ACTIVITY_WINDOW_SECONDS)
    }

    pub(crate) fn reported_model(&self, session_id: &str) -> Option<String> {
        self.snapshot
            .as_ref()?
            .records
            .get(session_id)?
            .model
            .clone()
    }

    pub(crate) fn project_visibility(
        &self,
        state: &AppState,
        input: &d::facts::query::ProjectVisibility,
    ) -> Result<()> {
        let snapshot = self.snapshot.as_ref().ok_or(fail("permission_denied"))?;
        if !snapshot.facts.records.iter().any(|record| {
            record.id == input.project_id && record.value.fact_kind() == FactKind::Project
        }) {
            return Err(fail("permission_denied"));
        }
        let _commit = state
            .plugins
            .platform_commit
            .lock()
            .checked("unavailable")?;
        let selection = state.plugins.selection(&self.plugin_id)?;
        state
            .plugins
            .ensure_platform_current(&self.plugin_id, &selection)?;
        let store = open_store(state)?;
        let grant = granted(&store.0, &self.plugin_id, &selection)?.ok_or(fail("grant_revoked"))?;
        if snapshot.key != grant.key(&selection) {
            return Err(fail("grant_revoked"));
        }
        grant.open()?;
        state.plugins.set_directory_project_visibility(input)
    }

    pub(crate) fn activity_state(
        &self,
        activity: &ActivityCandidate,
        now_ms: i64,
    ) -> d::activity::query::ActivityState {
        use d::activity::query::ActivityState;
        if !matches!(
            activity.phase,
            ActivityPhase::Running
                | ActivityPhase::Thinking
                | ActivityPhase::Generating
                | ActivityPhase::Tool
        ) {
            return ActivityState::Historical;
        }
        let recent = activity
            .occurred_at
            .as_deref()
            .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
            .is_some_and(|at| {
                (0..=i64::from(ACTIVITY_WINDOW_SECONDS) * 1000)
                    .contains(&now_ms.saturating_sub(at.timestamp_millis()))
            });
        if recent && self.source.status.freshness == d::Freshness::Current {
            ActivityState::Current
        } else {
            ActivityState::Expired
        }
    }

    pub(crate) async fn conversation(
        &self,
        state: &AppState,
        request: &d::UnifiedConversationRequest,
    ) -> Result<d::ConversationPage> {
        if request.cursor.is_some() && request.anchor.is_some() {
            return Err(fail("invalid_input"));
        }
        let mapper = self.mapper.clone().ok_or(fail("unavailable"))?;
        let snapshot = self.snapshot.clone().ok_or(fail("grant_revoked"))?;
        let (state, request) = (state.clone(), request.clone());
        let (id, digest) = (self.plugin_id.clone(), self.digest.clone());
        run_sync(move || {
            let body = mapper.conversation.as_ref().ok_or(fail("unsupported"))?;
            let mapped = snapshot
                .records
                .get(&request.session_id)
                .ok_or(fail("not_found"))?;
            let location = mapped.body.as_ref().ok_or(fail("not_found"))?;
            let selection = state.plugins.selection(&id)?;
            if selection.digest != digest {
                return Err(fail("grant_revoked"));
            }
            state.plugins.ensure_platform_current(&id, &selection)?;
            let db = open_store(&state)?;
            let grant = granted(&db.0, &id, &selection)?.ok_or(fail("grant_revoked"))?;
            if grant.key(&selection) != snapshot.key {
                return Err(fail("grant_revoked"));
            }
            let file = open_transcript(grant.open()?, &location.relative_path)
                .map_err(|_| fail("stale_snapshot"))?;
            let mut bytes = Vec::new();
            file.take(FILE_LIMIT + 1)
                .read_to_end(&mut bytes)
                .checked("unavailable")?;
            if bytes.len() as u64 > FILE_LIMIT || package::hash(&bytes) != location.file_hash {
                return Err(fail("stale_snapshot"));
            }
            let raw = documents(&bytes, mapper.format)?
                .nth(location.document)
                .ok_or(fail("stale_snapshot"))?;
            let document: Value = serde_json::from_str(raw).checked("invalid_record")?;
            let messages = pick(&document, &body.messages)
                .and_then(Value::as_array)
                .ok_or(fail("unsupported"))?;
            let page = conversation_page(&request, body, messages)?;
            let _commit = state
                .plugins
                .platform_commit
                .lock()
                .checked("unavailable")?;
            state.plugins.ensure_platform_current(&id, &selection)?;
            if granted(&db.0, &id, &selection)?.as_ref() != Some(&grant) {
                return Err(fail("grant_revoked"));
            }
            grant.open()?;
            Ok(page)
        })
        .await
    }

    pub(super) fn summary(&self, id: &str) -> Option<HydratedEntity> {
        if self.source.status.freshness != d::Freshness::Current {
            return None;
        }
        let mapped = self.snapshot.as_ref()?.records.get(id)?;
        Some(HydratedEntity {
            authorization: Authorization::Authorized,
            freshness: Freshness::Current,
            title: Some(mapped.record.title.clone()),
            source_id: Some(self.source.id.clone()),
            activity_state: mapped.activity.as_ref().and_then(|activity| {
                (self.activity_state(activity, chrono::Utc::now().timestamp_millis())
                    == d::activity::query::ActivityState::Current)
                    .then(|| serde_json::to_value(&activity.phase).ok())
                    .flatten()
                    .and_then(|phase| phase.as_str().map(str::to_owned))
            }),
            model: mapped.model.clone(),
            total_tokens: mapped
                .record
                .usage
                .total_tokens
                .as_ref()
                .map(d::Decimal::value),
            route_hash: None,
        })
    }

    pub(super) fn summary_grant(&self) -> Option<SummaryGrant> {
        Some(SummaryGrant {
            plugin: self.plugin_id.clone(),
            key: self.snapshot.as_ref()?.key.clone(),
        })
    }

    pub(crate) fn sessions(
        &self,
        filter: &d::SessionFilter,
        after: Option<d::SessionOrder>,
        cutoff: Option<i64>,
        hidden: &[String],
    ) -> Result<(Vec<d::SessionItem>, d::SessionScopeSummary)> {
        let mut window = d::SessionWindow::new(&self.source.id, after);
        if let Some(snapshot) = &self.snapshot {
            for mapped in snapshot.records.values() {
                let record = &mapped.record;
                if filter.matches(record, cutoff)
                    && !record
                        .project_ref
                        .as_ref()
                        .is_some_and(|project| hidden.contains(project))
                {
                    window
                        .observe(record.clone())
                        .map_err(|_| fail("resource_limit"))?;
                }
            }
        }
        Ok(window.finish())
    }
}

pub(super) struct SummaryGrant {
    plugin: String,
    key: String,
}

impl SummaryGrant {
    pub(super) fn ensure(&self, state: &AppState, db: &Connection) -> Result<()> {
        let selection = state.plugins.selection(&self.plugin)?;
        state
            .plugins
            .ensure_platform_current(&self.plugin, &selection)?;
        self.ensure_binding(db, &selection)
    }

    fn ensure_binding(&self, db: &Connection, selection: &Selection) -> Result<()> {
        let grant = granted(db, &self.plugin, selection)?.ok_or(fail("grant_revoked"))?;
        if grant.key(selection) != self.key {
            return Err(fail("grant_revoked"));
        }
        grant.open().map_err(|_| fail("grant_revoked"))?;
        Ok(())
    }
}

#[derive(PartialEq, Eq)]
struct BoundDirectory {
    path: String,
    device: i64,
    inode: i64,
    revision: i64,
}

impl BoundDirectory {
    fn open(&self) -> Result<File> {
        let file = directory::open(Path::new(&self.path))?;
        let meta = file.metadata().checked("permission_denied")?;
        if (meta.dev() as i64, meta.ino() as i64) != (self.device, self.inode) {
            return Err(fail("grant_revoked"));
        }
        Ok(file)
    }

    fn key(&self, selected: &Selection) -> String {
        package::hash(
            serde_json::json!([
                self.path,
                self.device,
                self.inode,
                self.revision,
                selected.digest,
                selected.revision,
                selected.epoch
            ])
            .to_string()
            .as_bytes(),
        )
    }
}

fn granted(db: &Connection, id: &str, selected: &Selection) -> Result<Option<BoundDirectory>> {
    db.query_row(
        "SELECT g.dir_path, g.device, g.inode, COALESCE(r.revision, 0)
         FROM ext_source_directory_grants g LEFT JOIN ext_platform_revisions r USING(plugin_id)
         WHERE g.plugin_id=?1 AND g.digest=?2 AND g.host_revision=?3",
        params![id, selected.digest, selected.revision.to_string()],
        |row| {
            Ok(BoundDirectory {
                path: row.get(0)?,
                device: row.get(1)?,
                inode: row.get(2)?,
                revision: row.get(3)?,
            })
        },
    )
    .optional()
    .checked("invalid_record")
}

pub(in crate::plugin_host::external) fn validate_mapper(mapper: &SourceMapper) -> Result<()> {
    let paths = [
        Some(&mapper.session_id),
        Some(&mapper.title),
        mapper.project.as_ref(),
        mapper.model.as_ref(),
        mapper.created_at.as_ref(),
        mapper.updated_at.as_ref(),
        mapper.usage.as_ref().map(|usage| &usage.input_tokens),
        mapper.usage.as_ref().map(|usage| &usage.output_tokens),
        mapper.activity.as_ref().map(|activity| &activity.phase),
        mapper
            .activity
            .as_ref()
            .map(|activity| &activity.occurred_at),
        mapper.conversation.as_ref().map(|body| &body.messages),
        mapper.conversation.as_ref().map(|body| &body.role),
        mapper.conversation.as_ref().map(|body| &body.text),
    ];
    if mapper.glob.len() > 200
        || mapper.glob.split('/').any(|part| {
            part.is_empty()
                || matches!(part, "." | "..")
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._*-".contains(&byte))
        })
        || paths
            .into_iter()
            .flatten()
            .any(|path| path_fields(path).is_none())
    {
        return Err(fail("invalid_input"));
    }
    Ok(())
}

fn path_fields(path: &str) -> Option<Vec<&str>> {
    let path = path.strip_prefix("$.")?;
    if path.len() > 200 {
        return None;
    }
    let fields = path.split('.').collect::<Vec<_>>();
    fields
        .iter()
        .all(|field| {
            !field.is_empty()
                && field
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
        })
        .then_some(fields)
}

fn pick<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path_fields(path)?
        .iter()
        .try_fold(value, |value, name| value.get(*name))
}

fn matching_segment(pattern: &str, name: &str) -> bool {
    let (mut pattern, mut name) = (pattern.as_bytes(), name.as_bytes());
    let mut retry = None;
    while let Some((&byte, rest)) = name.split_first() {
        match pattern.split_first() {
            Some((&b'*', tail)) => {
                pattern = tail;
                retry = Some((tail, name));
            }
            Some((&expected, tail)) if expected == byte => {
                pattern = tail;
                name = rest;
            }
            _ => match retry {
                Some((tail, previous)) if !previous.is_empty() => {
                    name = &previous[1..];
                    pattern = tail;
                    retry = Some((tail, name));
                }
                _ => return false,
            },
        }
    }
    pattern.iter().all(|byte| *byte == b'*')
}

fn matches_glob(pattern: &[&str], path: &[&str]) -> bool {
    glob_reach(pattern, path).0
}

fn glob_reach(pattern: &[&str], path: &[&str]) -> (bool, bool) {
    let mut reachable = vec![false; path.len() + 1];
    reachable[0] = true;
    let mut can_descend = false;
    for part in pattern {
        can_descend |= reachable[path.len()];
        if *part == "**" {
            for index in 1..reachable.len() {
                reachable[index] |= reachable[index - 1];
            }
        } else {
            for index in (1..reachable.len()).rev() {
                reachable[index] = reachable[index - 1] && matching_segment(part, path[index - 1]);
            }
            reachable[0] = false;
        }
    }
    (reachable[path.len()], can_descend)
}

struct Entries(*mut libc::DIR);
impl Drop for Entries {
    fn drop(&mut self) {
        unsafe { libc::closedir(self.0) };
    }
}

fn entries(folder: &File, remaining: &mut usize) -> Result<Vec<CString>> {
    let cloned = folder.try_clone().checked("unavailable")?;
    let raw = cloned.into_raw_fd();
    let dir = unsafe { libc::fdopendir(raw) };
    if dir.is_null() {
        unsafe { libc::close(raw) };
        return Err(fail("unavailable"));
    }
    let entries = Entries(dir);
    let mut names = Vec::new();
    loop {
        #[cfg(target_os = "macos")]
        let errno = unsafe { libc::__error() };
        #[cfg(not(target_os = "macos"))]
        let errno = unsafe { libc::__errno_location() };
        unsafe { *errno = 0 };
        let entry = unsafe { libc::readdir(entries.0) };
        if entry.is_null() {
            if unsafe { *errno } != 0 {
                return Err(fail("unavailable"));
            }
            break;
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        if matches!(name.to_bytes(), b"." | b"..") {
            continue;
        }
        *remaining = remaining.checked_sub(1).ok_or(fail("resource_limit"))?;
        if name.to_str().is_ok() {
            names.push(name.to_owned());
        }
    }
    names.sort();
    Ok(names)
}

fn open_relative(folder: &File, name: &CStr) -> std::io::Result<File> {
    let file = unsafe {
        libc::openat(
            folder.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if file < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(file) })
    }
}

fn open_transcript(mut root: File, path: &str) -> Result<File> {
    for part in path.split('/') {
        if part.is_empty() || matches!(part, "." | "..") {
            return Err(fail("permission_denied"));
        }
        root = open_relative(&root, &CString::new(part).checked("invalid_input")?)
            .checked("permission_denied")?;
    }
    let meta = root.metadata().checked("permission_denied")?;
    if !meta.is_file() || meta.nlink() != 1 || meta.len() > FILE_LIMIT {
        return Err(fail("permission_denied"));
    }
    Ok(root)
}

fn visit_files(
    root: File,
    glob: &str,
    mut visit: impl FnMut(String, File, u64) -> Result<()>,
) -> Result<bool> {
    let mut complete = true;
    let pattern = glob.split('/').collect::<Vec<_>>();
    let mut remaining = MAX_ENTRIES;
    let mut files = 0;
    let mut directories = VecDeque::from([(String::new(), root)]);
    while let Some((prefix, folder)) = directories.pop_front() {
        for name in entries(&folder, &mut remaining)? {
            let full = format!("{prefix}{}", name.to_string_lossy());
            let parts = full.split('/').collect::<Vec<_>>();
            let (matches, can_descend) = glob_reach(&pattern, &parts);
            if !matches && !can_descend {
                continue;
            }
            let data = match open_relative(&folder, &name) {
                Ok(data) => data,
                Err(error) if error.raw_os_error() == Some(libc::ELOOP) => continue,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    complete = false;
                    continue;
                }
                Err(_) => return Err(fail("unavailable")),
            };
            let meta = data.metadata().checked("unavailable")?;
            if meta.is_dir() {
                if can_descend && parts.len() >= 20 {
                    complete = false;
                } else if can_descend {
                    if directories.len() >= 256 {
                        return Err(fail("resource_limit"));
                    }
                    directories.push_back((format!("{full}/"), data));
                }
                continue;
            }
            if !matches || !meta.is_file() || meta.nlink() != 1 {
                continue;
            }
            files += 1;
            if files > 256 || meta.len() > FILE_LIMIT || !super::valid_object_id(&full) {
                return Err(fail("resource_limit"));
            }
            visit(full, data, meta.len())?;
        }
    }
    Ok(complete)
}

pub(super) fn guest_read(
    db: &Connection,
    plugin: &str,
    selection: &Selection,
    mapper: &SourceMapper,
    method: &str,
    input: Value,
) -> Result<Value> {
    let grant = granted(db, plugin, selection)?.ok_or(fail("permission_denied"))?;
    let root = grant.open()?;
    let result = match method {
        "source.list_files" => {
            if input != serde_json::json!({}) {
                return Err(fail("invalid_input"));
            }
            let mut paths = Vec::new();
            let complete = visit_files(root, &mapper.glob, |path, _, length| {
                paths.push(serde_json::json!({"path":path,"bytes":length}));
                Ok(())
            })?;
            if !complete {
                return Err(fail("unavailable"));
            }
            serde_json::json!({"files":paths})
        }
        "source.read" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct ReadChunk {
                path: String,
                offset: u64,
                max_bytes: u64,
            }
            let request: ReadChunk = serde_json::from_value(input).checked("invalid_input")?;
            let parts = request.path.split('/').collect::<Vec<_>>();
            let pattern = mapper.glob.split('/').collect::<Vec<_>>();
            if !super::valid_object_id(&request.path)
                || request.path.contains('\\')
                || parts.len() > 20
                || parts
                    .iter()
                    .any(|part| part.is_empty() || matches!(*part, "." | ".."))
                || !matches_glob(&pattern, &parts)
                || !(1..=32_768).contains(&request.max_bytes)
            {
                return Err(fail("permission_denied"));
            }
            let mut file = open_transcript(root, &request.path)?;
            let meta = file.metadata().checked("permission_denied")?;
            if request.offset > meta.len() {
                return Err(fail("permission_denied"));
            }
            file.seek(SeekFrom::Start(request.offset))
                .checked("unavailable")?;
            let mut bytes = Vec::new();
            file.take(request.max_bytes)
                .read_to_end(&mut bytes)
                .checked("unavailable")?;
            let next = request.offset + bytes.len() as u64;
            serde_json::json!({"bytes":bytes,"nextOffset":next,"complete":next >= meta.len()})
        }
        _ => return Err(fail("invalid_input")),
    };
    grant.open()?;
    Ok(result)
}

struct DirectoryScan {
    records: BTreeMap<String, MappedSession>,
    problem: Option<&'static str>,
}

fn scan(root: File, mapper: &SourceMapper, id: &str, product: &str) -> Result<DirectoryScan> {
    let mut problem = None;
    let mut records = BTreeMap::new();
    let mut left = SCAN_LIMIT;
    let source_id = format!("{id}:source");
    let complete = visit_files(root, &mapper.glob, |relative_path, data, length| {
        if length > left {
            return Err(fail("resource_limit"));
        }
        let mut bytes = Vec::new();
        data.take(FILE_LIMIT.min(left) + 1)
            .read_to_end(&mut bytes)
            .checked("unavailable")?;
        if bytes.len() as u64 > FILE_LIMIT || bytes.len() as u64 > left {
            return Err(fail("resource_limit"));
        }
        left -= bytes.len() as u64;
        let file_hash = mapper.conversation.as_ref().map(|_| package::hash(&bytes));
        for (document, chunk) in documents(&bytes, mapper.format)?.enumerate() {
            let value: Value = serde_json::from_str(chunk).checked("invalid_record")?;
            let Some(native) = pick(&value, &mapper.session_id).and_then(Value::as_str) else {
                problem = Some("unmapped_record");
                continue;
            };
            if !super::valid_object_id(native) || native.len() > 160 {
                return Err(fail("invalid_record"));
            }
            let label = pick(&value, &mapper.title)
                .and_then(Value::as_str)
                .unwrap_or("未命名会话");
            let model = mapper
                .model
                .as_ref()
                .and_then(|path| pick(&value, path))
                .and_then(Value::as_str)
                .map(|label| local_agent_core::sessions::sanitize_label(label, 120));
            let project = mapper
                .project
                .as_ref()
                .and_then(|path| pick(&value, path))
                .and_then(Value::as_str)
                .filter(|project| project.len() <= 4096 && Path::new(project).is_absolute())
                .map(|project| ProjectCandidate {
                    native_id: local_agent_core::projects::hash(project),
                    name: local_agent_core::sessions::sanitize_label(
                        Path::new(project)
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("/"),
                        120,
                    ),
                    path_hint: None,
                });
            let project_ref = project
                .as_ref()
                .map(|project| object_id(&source_id, "project", &project.native_id));
            let key = format!("{id}:{native}");
            let activity = mapper.activity.as_ref().and_then(|activity| {
                let phase = serde_json::from_value(pick(&value, &activity.phase)?.clone()).ok()?;
                Some(ActivityCandidate {
                    native_id: native.into(),
                    session_ref: key.clone(),
                    response_ref: None,
                    occurred_at: timestamp(&value, &activity.occurred_at),
                    phase,
                    duration_ms: None,
                })
            });
            let count = |path: &str| pick(&value, path).and_then(Value::as_u64);
            let input = mapper
                .usage
                .as_ref()
                .and_then(|map| count(&map.input_tokens));
            let output = mapper
                .usage
                .as_ref()
                .and_then(|map| count(&map.output_tokens));
            let record = d::SessionRecord {
                id: key.clone(),
                native_id: native.into(),
                product_ids: vec![product.into()],
                unknown_client: false,
                title: local_agent_core::sessions::sanitize_label(label, 120),
                created_at: mapper
                    .created_at
                    .as_ref()
                    .and_then(|path| timestamp(&value, path)),
                updated_at: mapper
                    .updated_at
                    .as_ref()
                    .and_then(|path| timestamp(&value, path)),
                project_ref,
                usage: d::Usage {
                    input_tokens: input.map(Into::into),
                    output_tokens: output.map(Into::into),
                    total_tokens: input
                        .zip(output)
                        .and_then(|(a, b)| a.checked_add(b))
                        .map(Into::into),
                    cached_input_tokens: None,
                    cache_write_input_tokens: None,
                    reasoning_output_tokens: None,
                },
                partial: false,
            };
            let body = file_hash.as_ref().map(|file_hash| TranscriptLocation {
                relative_path: relative_path.clone(),
                file_hash: file_hash.clone(),
                document,
            });
            records.insert(
                key,
                MappedSession {
                    record,
                    model,
                    project,
                    activity,
                    body,
                },
            );
            if records.len() > 10_000 {
                return Err(fail("resource_limit"));
            }
        }
        Ok(())
    })?;
    if !complete {
        problem = Some("incomplete_directory");
    }
    Ok(DirectoryScan { records, problem })
}

fn documents(bytes: &[u8], format: SourceFormat) -> Result<impl Iterator<Item = &str>> {
    let text = std::str::from_utf8(bytes).checked("invalid_record")?;
    let (single, lines) = match format {
        SourceFormat::Json => (Some(text), ""),
        SourceFormat::Jsonl => (None, text),
    };
    Ok(single
        .into_iter()
        .chain(lines.lines().filter(|line| !line.trim().is_empty())))
}

fn conversation_page(
    request: &d::UnifiedConversationRequest,
    mapper: &SourceConversationMapper,
    messages: &[Value],
) -> Result<d::ConversationPage> {
    if messages.len() > 10_000 {
        return Err(fail("resource_limit"));
    }
    let cursor_at = |index| {
        let scope = serde_json::json!([request.snapshot_token, request.session_id, index]);
        format!("{index}:{}", package::hash(scope.to_string().as_bytes()))
    };
    let event_id = |index: usize| {
        object_id(
            &request.session_id,
            "conversation-event",
            &index.to_string(),
        )
    };
    let start = if let Some(cursor) = &request.cursor {
        cursor
            .split_once(':')
            .and_then(|(index, _)| index.parse::<usize>().ok())
            .filter(|index| *index < messages.len() && cursor_at(*index) == *cursor)
            .ok_or(fail("stale_snapshot"))?
    } else if let Some(anchor) = &request.anchor {
        (0..messages.len())
            .find(|index| event_id(*index) == *anchor)
            .ok_or(fail("not_found"))?
    } else {
        0
    };
    let mut events = Vec::new();
    let mut next_cursor = None;
    for (index, value) in messages.iter().enumerate().skip(start) {
        let Some(text) = pick(value, &mapper.text).and_then(Value::as_str) else {
            continue;
        };
        if events.len() == CONVERSATION_PAGE_SIZE {
            next_cursor = Some(cursor_at(index));
            break;
        }
        let role = pick(value, &mapper.role)
            .and_then(Value::as_str)
            .filter(|role| matches!(*role, "user" | "assistant" | "system" | "tool"));
        let id = event_id(index);
        events.push(d::ConversationEvent {
            native_id: index.to_string(),
            turn_id: id.clone(),
            id,
            kind: "message".into(),
            role: role.map(str::to_owned),
            status: "recorded".into(),
            text: Some(text.into()),
            input: None,
            output: None,
            model: None,
            response_id: None,
            usage: None,
        });
    }
    Ok(d::ConversationPage {
        session_id: request.session_id.clone(),
        events,
        complete: next_cursor.is_none(),
        next_cursor,
    })
}

fn timestamp(value: &Value, path: &str) -> Option<String> {
    let value = pick(value, path)?.as_str()?;
    let parsed = chrono::DateTime::parse_from_rfc3339(value).ok()?;
    Some(parsed.to_utc().to_rfc3339())
}

fn facts(
    records: &BTreeMap<String, MappedSession>,
    mapper: &SourceMapper,
    identity: FactIdentity,
    source_revision: String,
) -> FactSnapshot {
    let mut available_kinds = vec![FactKind::Session];
    if mapper.project.is_some() {
        available_kinds.push(FactKind::Project);
    }
    if mapper.activity.is_some() {
        available_kinds.push(FactKind::Activity);
    }
    let mut facts = Vec::new();
    let mut projects = BTreeMap::new();
    let mut push = |id, value| {
        facts.push(FactRecord {
            id,
            identity: identity.clone(),
            source_revision: source_revision.clone(),
            value,
        })
    };
    for mapped in records.values() {
        let record = &mapped.record;
        if let Some((project_id, project)) =
            record.project_ref.as_ref().zip(mapped.project.as_ref())
        {
            projects.insert(project_id, project);
        }
        push(
            record.id.clone(),
            FactCandidate::Session(SessionCandidate {
                native_id: record.native_id.clone(),
                product_ids: record.product_ids.clone(),
                unknown_client: record.unknown_client,
                title: record.title.clone(),
                created_at: record.created_at.clone(),
                updated_at: record.updated_at.clone(),
                project_ref: record.project_ref.clone(),
                usage: record.usage.clone(),
                partial: record.partial,
            }),
        );
        if let Some(activity) = &mapped.activity {
            push(
                object_id(&identity.source_id, "activity", &activity.native_id),
                FactCandidate::Activity(activity.clone()),
            );
        }
    }
    for (id, project) in projects {
        push(id.clone(), FactCandidate::Project(project.clone()));
    }
    FactSnapshot {
        identity,
        source_revision,
        available_kinds,
        coverage: d::Coverage::Complete,
        records: facts,
    }
}

pub(crate) async fn refresh_if_directory(
    state: AppState,
    query: d::SourceConsentQuery,
) -> Result<bool> {
    run_sync(move || {
        let package = match state.plugins.installed(&query.plugin_id, &query.digest) {
            Ok(package) => package,
            Err(error) if error.code == "not_found" => return Ok(false),
            Err(error) => return Err(error),
        };
        if !package
            .manifest
            .capabilities
            .iter()
            .any(|name| name == DIRECTORY_SOURCE)
        {
            return Ok(false);
        }
        refresh(&state, &query.plugin_id, &query.digest)?;
        Ok(true)
    })
    .await
}

pub(super) fn refresh(state: &AppState, id: &str, digest: &str) -> Result<Option<&'static str>> {
    let _gate = state.plugins.gate(id)?.try_lock_owned().checked("busy")?;
    let (selection, definition) = state.plugins.active_platform(id, digest)?;
    let Some(AdapterDef::AgentAdapter { source_mapper }) = definition.adapter else {
        return Err(fail("not_found"));
    };
    let installed = state.plugins.installed(id, digest)?;
    let agent = installed.manifest.agent.ok_or(fail("invalid_record"))?;
    let store = open_store(state)?;
    let source = granted(&store.0, id, &selection)?.ok_or(fail("permission_denied"))?;
    let scanned = source
        .open()
        .and_then(|root| scan(root, &source_mapper, id, &agent.product_id));
    let _commit = state
        .plugins
        .platform_commit
        .lock()
        .checked("unavailable")?;
    state.plugins.ensure_platform_current(id, &selection)?;
    if granted(&store.0, id, &selection)?.as_ref() != Some(&source) {
        return Err(fail("grant_revoked"));
    }
    let DirectoryScan { records, problem } =
        match scanned.and_then(|scan| source.open().map(|_| scan)) {
            Ok(scan) => scan,
            Err(error) => {
                state
                    .plugins
                    .cache
                    .lock()
                    .checked("unavailable")?
                    .directory_errors
                    .insert(
                        id.into(),
                        (
                            source.key(&selection),
                            error.code,
                            chrono::Utc::now().to_rfc3339(),
                        ),
                    );
                return Err(error);
            }
        };
    let revision = package::hash(&serde_json::to_vec(&records).checked("invalid_record")?);
    let mut facts = facts(
        &records,
        &source_mapper,
        FactIdentity {
            provider_key: format!("{id}:domain.session-source@1:source"),
            source_id: format!("{id}:source"),
            implementation_identity: selection.digest.clone(),
            epoch: selection.epoch.into(),
            grant_revision: u64::try_from(source.revision)
                .checked("invalid_record")?
                .into(),
            observed_at: chrono::Utc::now().to_rfc3339(),
            evidence: EvidenceLevel::SourceReported,
        },
        revision,
    );
    if problem.is_some() {
        facts.coverage = d::Coverage::Partial;
    }
    let mut cache = state.plugins.cache.lock().checked("unavailable")?;
    cache.directory_errors.remove(id);
    cache.directory_sources.insert(
        id.into(),
        Arc::new(Snapshot {
            key: source.key(&selection),
            observed: Instant::now(),
            records,
            facts,
            scan_problem: problem,
        }),
    );
    Ok(problem)
}

pub(crate) async fn run_sources(state: AppState, runtime: crate::runtime::Runtime) {
    let mut timer = tokio::time::interval(std::time::Duration::from_secs(15));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! { _ = runtime.stopped() => return, _ = timer.tick() => {} }
        if !state.plugins.root.join("platform.sqlite3").is_file() {
            continue;
        }
        let Some(permit) = state.readonly.intake.acquire_for(None) else {
            continue;
        };
        crate::plugin_host::intake::ACTIVE_JOB
            .scope(Arc::new(permit), refresh_sources(&state))
            .await;
    }
}

async fn refresh_sources(state: &AppState) {
    let state = state.clone();
    let _ = run_sync(move || {
        for plugin in state.plugins.list()?.into_iter().filter(|plugin| {
            plugin.enabled
                && plugin
                    .manifest
                    .capabilities
                    .iter()
                    .any(|cap| cap == DIRECTORY_SOURCE)
        }) {
            let _ = refresh(&state, &plugin.manifest.id, &plugin.digest);
        }
        Ok(())
    })
    .await;
}

pub(crate) fn source_views(state: &AppState) -> Result<Vec<DirectoryView>> {
    let mut views = Vec::new();
    for plugin in state.plugins.list()?.into_iter().filter(|plugin| {
        plugin.enabled
            && plugin
                .manifest
                .capabilities
                .iter()
                .any(|cap| cap == DIRECTORY_SOURCE)
    }) {
        let id = plugin.manifest.id;
        let selected = state.plugins.selection(&id)?;
        let mapper = state
            .plugins
            .active_platform(&id, &plugin.digest)
            .ok()
            .and_then(|(_, definition)| match definition.adapter {
                Some(AdapterDef::AgentAdapter { source_mapper }) => Some(source_mapper),
                _ => None,
            });
        let runtime_active = mapper.is_some();
        let agent = plugin.manifest.agent.ok_or(fail("invalid_record"))?;
        let db = open_store(state)?;
        let grant = granted(&db.0, &id, &selected)?;
        let directory_error = grant
            .as_ref()
            .and_then(|directory| directory.open().err())
            .map(|err| err.code);
        let valid = runtime_active && grant.is_some() && directory_error.is_none();
        let key = grant.as_ref().map(|grant| grant.key(&selected));
        let cache = state.plugins.cache.lock().checked("unavailable")?;
        let failure = cache
            .directory_errors
            .get(&id)
            .filter(|(failed_key, _, _)| valid && key.as_ref() == Some(failed_key))
            .map(|(_, code, at)| (*code, at.clone()));
        let snapshot = cache
            .directory_sources
            .get(&id)
            .cloned()
            .filter(|snapshot| valid && key.as_ref() == Some(&snapshot.key));
        drop(cache);
        let scan_problem = snapshot.as_ref().and_then(|s| s.scan_problem);
        let freshness = match &snapshot {
            Some(_) if failure.is_some() => d::Freshness::Stale,
            None if failure.is_some() => d::Freshness::Failed,
            Some(snapshot) if snapshot.observed.elapsed().as_secs() < 45 => d::Freshness::Current,
            Some(_) => d::Freshness::Stale,
            None => d::Freshness::Unread,
        };
        let source_id = format!("{id}:source");
        let provider_key = format!("{id}:domain.session-source@1:source");
        views.push(DirectoryView {
            plugin_id: id.clone(),
            digest: selected.digest.clone(),
            last_attempt: failure.as_ref().map(|(_, at)| at.clone()).or_else(|| {
                snapshot
                    .as_ref()
                    .map(|s| s.facts.identity.observed_at.clone())
            }),
            snapshot,
            problem: failure
                .map(|(code, _)| code)
                .or(directory_error)
                .or(scan_problem)
                .map(str::to_owned)
                .or_else(|| (!valid).then(|| "permission_denied".into())),
            mapper,
            source: d::Source {
                id: source_id.clone(),
                provider_key: provider_key.clone(),
                product_ids: vec![agent.product_id.clone()],
                status: d::ProviderStatus {
                    enabled: selected.enabled,
                    runtime_active,
                    grant_revision: selected.revision.into(),
                    freshness,
                    authorization: if directory_error == Some("grant_revoked") {
                        d::Authorization::Revoked
                    } else if valid {
                        d::Authorization::Authorized
                    } else {
                        d::Authorization::Required
                    },
                },
            },
            provider: d::Provider {
                provider_key,
                plugin_id: id,
                implementation_version: plugin.manifest.version,
                implementation_identity: selected.digest,
                capability: d::Capability {
                    name: "domain.session-source".into(),
                    major: 1,
                    instance_id: source_id.clone(),
                },
                execution_kind: d::ExecutionKind::Declarative,
            },
            product: d::Product {
                id: agent.product_id,
                display_name: agent.display_name,
                source_id,
                config_id: None,
            },
        });
    }
    Ok(views)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hydrated_directory_grant_cannot_survive_revocation_or_reauthorization() {
        use super::super::PlatformStore;
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let store = PlatformStore::open(&root, "summary-fixture").unwrap();
        let data = root.join("transcripts");
        std::fs::create_dir(&data).unwrap();
        let metadata = data.metadata().unwrap();
        let selected = Selection {
            digest: "source-package".into(),
            revision: 1,
            epoch: 1,
            report: false,
            previous: None,
            enabled: true,
            status: "active".into(),
            error: None,
            source: None,
            connection: None,
        };
        let insert = || {
            store
                .0
                .execute(
                    "INSERT INTO ext_source_directory_grants VALUES('source',?1,'1',?2,?3,?4)",
                    params![
                        selected.digest,
                        data.to_str().unwrap(),
                        metadata.dev() as i64,
                        metadata.ino() as i64
                    ],
                )
                .unwrap();
        };
        insert();
        let summary = SummaryGrant {
            plugin: "source".into(),
            key: granted(&store.0, "source", &selected)
                .unwrap()
                .unwrap()
                .key(&selected),
        };
        summary.ensure_binding(&store.0, &selected).unwrap();
        directory::revoke(&store.0, "source", DirectoryPurpose::SourceRead).unwrap();
        PlatformStore::bump_revision(&store.0, "source").unwrap();
        assert_eq!(
            summary
                .ensure_binding(&store.0, &selected)
                .unwrap_err()
                .code,
            "grant_revoked"
        );
        insert();
        PlatformStore::bump_revision(&store.0, "source").unwrap();
        assert_eq!(
            summary
                .ensure_binding(&store.0, &selected)
                .unwrap_err()
                .code,
            "grant_revoked"
        );
    }

    #[test]
    fn cursor_mapper_entries_validate_as_independent_domain_capabilities() {
        let definition = compile_rules(
            "plugin.yaml",
            include_bytes!(
                "../../../../../../sdk/plugins/examples/cursor-dir-agent/package/plugin.yaml"
            ),
        )
        .unwrap();
        let id = definition.manifest.id.as_str();
        let Some(AdapterDef::AgentAdapter { source_mapper }) = definition.platform.adapter else {
            panic!("fixture adapter missing")
        };
        let source_id = format!("{id}:source");
        let provider_key = format!("{id}:domain.session-source@1:source");
        let mut view = DirectoryView {
            plugin_id: id.into(),
            digest: "fixture".into(),
            snapshot: None,
            problem: None,
            last_attempt: None,
            mapper: Some(source_mapper),
            source: d::Source {
                id: source_id.clone(),
                provider_key: provider_key.clone(),
                product_ids: vec![],
                status: d::ProviderStatus {
                    enabled: true,
                    runtime_active: true,
                    grant_revision: 1u64.into(),
                    authorization: d::Authorization::Authorized,
                    freshness: d::Freshness::Unread,
                },
            },
            provider: d::Provider {
                provider_key,
                plugin_id: id.into(),
                implementation_version: "1".into(),
                implementation_identity: "fixture".into(),
                capability: d::Capability {
                    name: "domain.session-source".into(),
                    major: 1,
                    instance_id: source_id.clone(),
                },
                execution_kind: d::ExecutionKind::Declarative,
            },
            product: d::Product {
                id: "cursor".into(),
                display_name: "Cursor".into(),
                source_id,
                config_id: None,
            },
        };
        let check = |view: &DirectoryView| {
            let entries = view.capabilities();
            let catalog = d::CapabilityCatalog {
                api_version: d::API_VERSION.into(),
                revision: 1u64.into(),
                external_problem: None,
                entries,
            };
            catalog.validate().unwrap();
            catalog.entries
        };
        assert_eq!(check(&view).len(), 3);
        let mapper = view.mapper.as_mut().unwrap();
        mapper.project = None;
        mapper.activity = None;
        mapper.conversation = None;
        let entries = check(&view);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].operations, [d::ReadOperation::Scan]);
    }

    #[test]
    fn relative_globs_are_bounded_and_match_nested_transcripts() {
        let pattern = ["**", "agent-transcripts", "*.jsonl"];
        assert!(matches_glob(&pattern, &["agent-transcripts", "one.jsonl"]));
        assert!(matches_glob(
            &pattern,
            &["project", "agent-transcripts", "two.jsonl"]
        ));
        assert!(!matches_glob(
            &pattern,
            &["project", "agent-transcripts", "three.json"]
        ));
    }

    #[test]
    fn scan_distinguishes_empty_unmapped_and_invalid_jsonl() {
        let package = compile_rules(
            "plugin.yaml",
            include_bytes!(
                "../../../../../../sdk/plugins/examples/cursor-dir-agent/package/plugin.yaml"
            ),
        )
        .unwrap();
        let Some(AdapterDef::AgentAdapter { source_mapper }) = package.platform.adapter else {
            panic!("fixture adapter missing")
        };
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("agent-transcripts");
        std::fs::create_dir(&dir).unwrap();
        let read = || {
            scan(
                File::open(root.path()).unwrap(),
                &source_mapper,
                "cursor",
                "cursor-desktop",
            )
        };
        let empty = read().unwrap();
        assert!(empty.records.is_empty() && empty.problem.is_none());
        let path = dir.join("sessions.jsonl");
        std::fs::write(&path, b"{\"session_id\":\"one\"}\n{}\n").unwrap();
        let partial = read().unwrap();
        assert_eq!(partial.records.len(), 1);
        assert_eq!(partial.problem, Some("unmapped_record"));
        std::fs::write(&path, b"{").unwrap();
        assert_eq!(read().err().unwrap().code, "invalid_record");
        std::fs::write(&path, b"{\"session_id\":\"one\"}\n").unwrap();
        let restored = read().unwrap();
        assert_eq!(restored.records.len(), 1);
        assert_eq!(restored.problem, None);
    }

    #[test]
    fn narrow_glob_prunes_unrelated_directory_trees() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path();
        let transcripts = path.join("agent-transcripts");
        std::fs::create_dir(&transcripts).unwrap();
        std::fs::write(transcripts.join("first.jsonl"), b"{}").unwrap();
        for index in 0..300 {
            std::fs::create_dir(path.join(format!("unrelated-{index}"))).unwrap();
        }
        std::fs::create_dir_all(transcripts.join("folder.jsonl/child")).unwrap();
        std::fs::write(transcripts.join("folder.jsonl/child/buried.jsonl"), b"{}").unwrap();
        std::os::unix::fs::symlink(
            transcripts.join("folder.jsonl/child"),
            transcripts.join("linked"),
        )
        .unwrap();
        let collect = |glob| {
            let mut paths = Vec::new();
            visit_files(File::open(path).unwrap(), glob, |path, _, _| {
                paths.push(path);
                Ok(())
            })
            .unwrap();
            paths
        };
        assert_eq!(
            collect("agent-transcripts/*.jsonl"),
            ["agent-transcripts/first.jsonl"]
        );
        assert_eq!(
            collect("agent-transcripts/**/buried.jsonl"),
            ["agent-transcripts/folder.jsonl/child/buried.jsonl"]
        );
    }
}
