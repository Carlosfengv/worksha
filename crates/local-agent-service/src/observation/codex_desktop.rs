//! Read-only desktop IPC follower. Never starts/resumes a turn or touches model routing.
mod metrics;
#[cfg(test)]
mod tests;
mod timings;
use local_agent_core::sessions::live::NativeActivity;
use metrics::{Metrics, Target};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[derive(Default)]
pub(super) struct Observer {
    worker: Option<Worker>,
    timings: timings::Timings,
}
struct Worker {
    home: PathBuf,
    desired: Arc<Mutex<HashMap<String, Target>>>,
    metrics: Arc<Mutex<Metrics>>,
    stop: Arc<AtomicBool>,
    #[cfg(unix)]
    socket: Arc<Mutex<Option<std::os::unix::net::UnixStream>>>,
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        #[cfg(unix)]
        if let Ok(socket) = self.socket.lock()
            && let Some(socket) = socket.as_ref()
        {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
    }
}
impl Observer {
    pub fn new(directory: &Path) -> Self {
        Self {
            timings: timings::Timings::open(directory.join("codex-native-timings.json")),
            ..Default::default()
        }
    }
    pub fn flush(&mut self) -> Result<(), String> {
        self.timings.commit()
    }
    pub fn error(&self) -> Option<String> {
        self.timings.error.clone()
    }
    pub fn forget(&mut self) {
        self.stop();
        self.timings.forget();
        let _ = self.flush();
    }
    pub fn stop(&mut self) {
        self.worker = None;
    }
    pub fn sync(&mut self, home: &Path, rows: &mut [NativeActivity]) {
        let now = chrono::Utc::now().timestamp_millis();
        let mut desired: HashMap<String, Target> = HashMap::new();
        let mut ranks = HashMap::new();
        for row in rows.iter() {
            if row.agent_id != "codex" || row.agent_product_id.as_deref() == Some("codex-cli") {
                continue;
            }
            let Some(session) = row
                .session_id
                .strip_prefix("codex:")
                .filter(|s| uuid::Uuid::parse_str(s).is_ok())
            else {
                continue;
            };
            let Some(turn) = row
                .message_id()
                .filter(|s| uuid::Uuid::parse_str(s).is_ok())
            else {
                continue;
            };
            let started = row
                .message_started_at
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|t| t.timestamp_millis())
                .unwrap_or(0);
            let rank = if started > 0 {
                started
            } else {
                chrono::DateTime::parse_from_rfc3339(&row.event_at)
                    .map(|t| t.timestamp_millis())
                    .unwrap_or(0)
            };
            if rank == 0 {
                continue;
            }
            if !(0..=10_800_000).contains(&(now - rank)) {
                continue;
            }
            if ranks.get(session).is_none_or(|previous| rank > *previous) {
                ranks.insert(session.to_owned(), rank);
                desired.insert(
                    session.into(),
                    Target {
                        turn: turn.into(),
                        started,
                    },
                );
            }
        }
        // Bound subscriptions by recency, never by whichever tool was last active.
        let mut ordered: Vec<_> = desired.into_iter().collect();
        ordered.sort_by(|a, b| ranks[&b.0].cmp(&ranks[&a.0]).then_with(|| a.0.cmp(&b.0)));
        let desired: HashMap<_, _> = ordered.into_iter().take(32).collect();
        if desired.is_empty() {
            self.stop();
            return;
        }
        if self.worker.as_ref().is_some_and(|w| w.home != home) {
            self.stop();
        }
        if self.worker.is_none() {
            self.worker = Worker::start(home, self.timings.text_totals());
        }
        let Some(worker) = &self.worker else { return };
        if let Ok(mut targets) = worker.desired.lock() {
            *targets = desired.clone();
        }
        let Ok(mut metrics) = worker.metrics.lock() else {
            return;
        };
        metrics.retain(&desired);
        for row in rows {
            let Some(session) = row
                .session_id
                .strip_prefix("codex:")
                .filter(|s| desired.contains_key(*s))
            else {
                continue;
            };
            let Some(turn) = row.message_id() else {
                continue;
            };
            let (mut measurement, phase) = metrics.measurement(session, turn, now);
            if let Some(sampling) = &mut measurement.sampling {
                sampling.reported = row.message_usage.clone();
            }
            let started = metrics.started(session, turn).or_else(|| {
                row.message_started_at
                    .as_deref()
                    .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
                    .map(|at| at.timestamp_millis())
            });
            self.timings.apply(
                session,
                turn,
                started,
                &mut measurement,
                metrics.text_totals(session, turn),
            );
            let (model, input) = metrics.metadata(session, turn);
            if row.model.is_none() {
                row.model = model.map(str::to_owned);
            }
            if row.user_input.is_none()
                && let (Some(text), Some(at)) = (
                    input,
                    started.and_then(chrono::DateTime::from_timestamp_millis),
                )
            {
                row.user_input = Some(local_agent_core::sessions::activity_input::UserInput {
                    text: text.into(),
                    at: at.to_rfc3339(),
                });
            }
            if row.message_started_at.is_none()
                && let Some(started) = started.and_then(chrono::DateTime::from_timestamp_millis)
            {
                row.message_started_at = Some(started.to_rfc3339());
            }
            if let Some((phase, at)) = phase {
                let original = chrono::DateTime::parse_from_rfc3339(&row.event_at)
                    .map(|t| t.timestamp_millis())
                    .unwrap_or(0);
                if at > original
                    && !["waiting_input", "error", "cancelled"].contains(&row.phase.as_str())
                {
                    row.phase = phase.into();
                    if let Some(time) = chrono::DateTime::from_timestamp_millis(at) {
                        row.event_at = time.to_rfc3339();
                    }
                    row.stale = now - at > 30_000;
                    row.step = match phase {
                        "streaming" => "正在输出正文",
                        "thinking" => "正在思考",
                        "tool_requested" => "正在调用工具",
                        _ => &row.step,
                    }
                    .into();
                }
            }
            if let Some(sampling) = &measurement.sampling {
                row.message_usage = sampling.reported.clone();
            }
            row.realtime = Some(measurement);
        }
        // Commit before publishing these rows to the UI. A value that has
        // already been displayed must not depend on a later shutdown flush.
        let _ = self.timings.commit();
    }
}
impl Worker {
    fn start(home: &Path, text: HashMap<(String, String), metrics::TextTotals>) -> Option<Self> {
        #[cfg(target_os = "macos")]
        {
            let worker = Self {
                home: home.into(),
                desired: Arc::default(),
                metrics: Arc::new(Mutex::new(Metrics::with_text_totals(text))),
                stop: Arc::default(),
                socket: Arc::default(),
            };
            let (home, desired, metrics, stop, socket) = (
                worker.home.clone(),
                worker.desired.clone(),
                worker.metrics.clone(),
                worker.stop.clone(),
                worker.socket.clone(),
            );
            std::thread::Builder::new()
                .name("codex-native-events".into())
                .spawn(move || run(home, desired, metrics, stop, socket))
                .ok()?;
            Some(worker)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (home, text);
            None
        }
    }
}

#[cfg(any(target_os = "macos", test))]
fn send(socket: &mut impl std::io::Write, value: &serde_json::Value) -> std::io::Result<()> {
    let data = serde_json::to_vec(value)?;
    if data.len() > 8192 {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
    socket.write_all(&(data.len() as u32).to_le_bytes())?;
    socket.write_all(&data)
}
#[cfg(any(target_os = "macos", test))]
fn following(client: &str, session: &str, owner: &str, value: bool) -> serde_json::Value {
    serde_json::json!({"type":"broadcast","method":"thread-stream-following-changed","sourceClientId":client,"targetClientIds":[owner],"version":1,"params":{"conversationId":session,"hostId":"local","following":value}})
}
#[cfg(target_os = "macos")]
fn connect(home: &Path) -> std::io::Result<std::os::unix::net::UnixStream> {
    use std::{
        ffi::CStr,
        os::{
            fd::AsRawFd,
            unix::fs::{FileTypeExt, MetadataExt},
        },
    };
    let path = home.join(".codex/ipc/ipc.sock");
    // No symlinks, alternate endpoints, or world/group writable IPC directories.
    for part in [home.join(".codex"), home.join(".codex/ipc"), path.clone()] {
        let meta = part.symlink_metadata()?;
        if meta.file_type().is_symlink()
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.mode() & 0o022 != 0
        {
            return Err(std::io::ErrorKind::PermissionDenied.into());
        }
    }
    if !path.symlink_metadata()?.file_type().is_socket() {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    let socket = std::os::unix::net::UnixStream::connect(path)?;
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of_val(&pid) as libc::socklen_t;
    // Read the kernel-authenticated peer PID, never a PID claimed by a JSON event.
    if unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&mut pid as *mut libc::pid_t).cast(),
            &mut len,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error());
    }
    let mut peer_path = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    if unsafe { libc::proc_pidpath(pid, peer_path.as_mut_ptr().cast(), peer_path.len() as u32) }
        <= 0
        || CStr::from_bytes_until_nul(&peer_path)
            .ok()
            .and_then(|s| s.to_str().ok())
            != Some("/Applications/ChatGPT.app/Contents/MacOS/ChatGPT")
    {
        return Err(std::io::ErrorKind::PermissionDenied.into());
    }
    socket.set_read_timeout(Some(Duration::from_millis(250)))?;
    socket.set_write_timeout(Some(Duration::from_secs(2)))?;
    Ok(socket)
}

#[cfg(target_os = "macos")]
fn run(
    home: PathBuf,
    desired: Arc<Mutex<HashMap<String, Target>>>,
    metrics: Arc<Mutex<Metrics>>,
    stop: Arc<AtomicBool>,
    control: Arc<Mutex<Option<std::os::unix::net::UnixStream>>>,
) {
    while !stop.load(Ordering::Acquire) {
        let result = connect(&home).and_then(|socket| {
            if let Ok(mut metrics) = metrics.lock() {
                metrics.state = "connecting";
            }
            if let Ok(mut control) = control.lock() {
                *control = Some(socket.try_clone()?);
            }
            session(socket, &desired, &metrics, &stop)
        });
        if let Ok(mut control) = control.lock() {
            *control = None;
        }
        if let Ok(mut metrics) = metrics.lock() {
            let incompatible = metrics.state == "incompatible";
            metrics.disconnected(if incompatible {
                "incompatible"
            } else if result.is_err_and(|e| e.kind() == std::io::ErrorKind::PermissionDenied) {
                "unavailable"
            } else {
                "reconnecting"
            });
        }
        for _ in 0..20 {
            if stop.load(Ordering::Acquire) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

#[cfg(any(target_os = "macos", test))]
enum Frame {
    Event(serde_json::Value),
    Snapshot(metrics::Snapshot),
}
#[cfg(any(target_os = "macos", test))]
fn frame(reader: &mut impl std::io::Read) -> std::io::Result<Frame> {
    use std::io::Read;
    let mut header = [0u8; 4];
    reader.read_exact(&mut header)?;
    let size = u32::from_le_bytes(header);
    if size == 0 || size > 268_435_456 {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
    let mut body = reader.take(u64::from(size));
    let result = if size > 1024 * 1024 {
        // Streaming projection prevents a 20 MB history snapshot becoming a
        // full second transcript in the observer's heap.
        Frame::Snapshot(serde_json::from_reader(&mut body)?)
    } else {
        Frame::Event(serde_json::from_reader(&mut body)?)
    };
    if body.limit() != 0 {
        return Err(std::io::ErrorKind::UnexpectedEof.into());
    }
    Ok(result)
}

#[cfg(target_os = "macos")]
fn session(
    mut socket: std::os::unix::net::UnixStream,
    desired: &Mutex<HashMap<String, Target>>,
    metrics: &Mutex<Metrics>,
    stop: &AtomicBool,
) -> std::io::Result<()> {
    use serde_json::json;
    use std::io::{BufRead, BufReader};
    let init = uuid::Uuid::new_v4().to_string();
    send(
        &mut socket,
        &json!({"type":"request","requestId":init,"method":"initialize","version":0,"params":{"clientType":"local-agent-observer"}}),
    )?;
    let mut reader = BufReader::new(socket.try_clone()?);
    let mut client = None;
    let mut subscriptions = HashMap::<String, String>::new();
    let mut pending = HashMap::<String, String>::new();
    let mut attempt = std::time::Instant::now() - Duration::from_secs(5);
    let connected = std::time::Instant::now();
    while !stop.load(Ordering::Acquire) {
        if metrics.lock().is_ok_and(|m| m.state == "incompatible") {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
        let targets = desired.lock().map(|d| d.clone()).unwrap_or_default();
        if let Some(client) = client.as_deref() {
            let removed: Vec<_> = subscriptions
                .keys()
                .filter(|s| !targets.contains_key(*s))
                .cloned()
                .collect();
            for session in removed {
                let owner = subscriptions
                    .remove(&session)
                    .expect("existing subscription");
                send(&mut socket, &following(client, &session, &owner, false))?;
            }
            if attempt.elapsed() >= Duration::from_secs(5) {
                pending.clear();
                for session in targets.keys().filter(|s| !subscriptions.contains_key(*s)) {
                    let id = uuid::Uuid::new_v4().to_string();
                    send(
                        &mut socket,
                        &json!({"type":"request","sourceClientId":client,"requestId":id,"method":"thread-owner-discovery","version":1,"params":{"hostId":"local","conversationId":session},"timeoutMs":3000}),
                    )?;
                    pending.insert(id, session.clone());
                }
                attempt = std::time::Instant::now();
            }
        } else if connected.elapsed() > Duration::from_secs(3) {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        match reader.fill_buf() {
            Err(e)
                if [std::io::ErrorKind::WouldBlock, std::io::ErrorKind::TimedOut]
                    .contains(&e.kind()) =>
            {
                continue;
            }
            Err(e) => return Err(e),
            Ok([]) => return Err(std::io::ErrorKind::UnexpectedEof.into()),
            _ => {}
        }
        match frame(&mut reader)? {
            Frame::Snapshot(snapshot) => {
                if let Ok(mut m) = metrics.lock() {
                    m.snapshot(snapshot, &targets, chrono::Utc::now().timestamp_millis());
                }
            }
            Frame::Event(v) => {
                if v["type"] == "response" {
                    if v["requestId"] == init
                        && v["method"] == "initialize"
                        && v["resultType"] == "success"
                    {
                        client = v["result"]["clientId"]
                            .as_str()
                            .filter(|s| uuid::Uuid::parse_str(s).is_ok())
                            .map(str::to_owned);
                    } else if let Some(session) =
                        v["requestId"].as_str().and_then(|id| pending.remove(id))
                        && v["resultType"] == "success"
                        && targets.contains_key(&session)
                        && let (Some(client), Some(owner)) = (
                            client.as_deref(),
                            v["handledByClientId"]
                                .as_str()
                                .filter(|s| uuid::Uuid::parse_str(s).is_ok()),
                        )
                    {
                        if let Ok(mut m) = metrics.lock() {
                            m.owners.insert(session.clone(), owner.into());
                        }
                        send(&mut socket, &following(client, &session, owner, true))?;
                        subscriptions.insert(session, owner.into());
                    }
                } else if v["type"] == "client-discovery-request" {
                    send(
                        &mut socket,
                        &json!({"type":"client-discovery-response","requestId":v["requestId"],"response":{"canHandle":false}}),
                    )?;
                } else if v["type"] == "broadcast" && v["method"] == "thread-stream-state-changed" {
                    if v["params"]["change"]["type"] == "snapshot" {
                        let snapshot = serde_json::from_value(v)?;
                        if let Ok(mut m) = metrics.lock() {
                            m.snapshot(snapshot, &targets, chrono::Utc::now().timestamp_millis());
                        }
                    } else {
                        let resync = metrics.lock().ok().and_then(|mut m| {
                            m.patches(&v, &targets, chrono::Utc::now().timestamp_millis())
                        });
                        if let Some(session) = resync
                            && let (Some(client), Some(owner)) =
                                (client.as_deref(), subscriptions.get(&session))
                        {
                            send(&mut socket, &following(client, &session, owner, true))?;
                        }
                    }
                } else if v["method"] == "client-status-changed"
                    && v["params"]["status"] == "disconnected"
                {
                    let owner = v["params"]["clientId"].as_str();
                    let lost: Vec<_> = subscriptions
                        .iter()
                        .filter(|(_, id)| Some(id.as_str()) == owner)
                        .map(|(s, _)| s.clone())
                        .collect();
                    if let Ok(mut m) = metrics.lock() {
                        for session in lost {
                            subscriptions.remove(&session);
                            m.session_disconnected(&session);
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
