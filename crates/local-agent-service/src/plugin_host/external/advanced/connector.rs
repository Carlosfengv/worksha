//! Explicitly started MCP carrier. No tools are executed by management commands.
use super::{
    Checked, Connection, Connector, Lease, Manager, Query, Result, checked_bytes, fail, protocol,
};
use axum::{
    Router,
    body::{Body, Bytes, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use futures_util::StreamExt;
use local_agent_core::Provider;
use rusqlite::params;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncWriteExt, BufReader},
    sync::{broadcast, mpsc, oneshot, watch},
};

#[derive(Clone)]
pub(super) struct Cancellation(watch::Sender<bool>);
impl Default for Cancellation {
    fn default() -> Self {
        Self(watch::channel(false).0)
    }
}
impl Cancellation {
    fn cancel(&self) {
        self.0.send_replace(true);
    }
    fn is_cancelled(&self) -> bool {
        *self.0.borrow()
    }
    async fn cancelled(&self) {
        let mut receiver = self.0.subscribe();
        let _ = receiver.wait_for(|cancelled| *cancelled).await;
    }
}

pub(crate) struct Live {
    cancel: Cancellation,
    pub(super) task: tokio::task::JoinHandle<()>,
    pub(super) host: String,
    token: String,
    pub(super) state: Arc<Mutex<Telemetry>>,
}
impl Drop for Live {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.task.abort();
    }
}
#[derive(Default)]
pub(super) struct Telemetry {
    connected: bool,
    tools: Option<usize>,
    notifications: u64,
    last_error: Option<&'static str>,
    calls: u64,
    pending_records: u64,
    unrecorded_calls: u64,
}
impl Live {
    pub(super) fn public(&self) -> Value {
        let t = self.state.lock().unwrap_or_else(|p| p.into_inner());
        json!({"host":self.host,"transportConnected":t.connected,"observationEnabled":!self.cancel.is_cancelled()&&!self.task.is_finished(),"tools":t.tools,"notifications":t.notifications,"calls":t.calls,"observationIncomplete":t.pending_records>0||t.unrecorded_calls>0,"unconfirmedCalls":t.pending_records.saturating_add(t.unrecorded_calls),"lastError":t.last_error,"content":"not-persisted","note":"工具调用仅由配置此入口的外部客户端发起；握手和目录读取不计为工具执行"})
    }
}
pub(super) fn validate_connection(kind: &Connector, c: &Connection) -> Result<()> {
    match (kind, c) {
        (Connector::McpStdio, Connection::Stdio { command, args }) => {
            if !Path::new(command).is_absolute()
                || args.len() > 64
                || args.iter().any(|s| s.len() > 4096 || s.contains('\0'))
            {
                return Err(fail("invalid_input"));
            }
            let p = std::fs::symlink_metadata(command).checked("permission_denied")?;
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            if !p.is_file()
                || p.file_type().is_symlink()
                || (p.nlink() != 1 && p.uid() != 0)
                || p.permissions().mode() & 0o111 == 0
            {
                return Err(fail("permission_denied"));
            }
            Ok(())
        }
        (Connector::McpHttp, Connection::Http { url, .. }) => {
            local_agent_core::providers::validate_endpoint(url)
                .map(|_| ())
                .checked("invalid_input")
        }
        (Connector::DshSessions, _) => Err(fail("unverified_connector")),
        _ => Err(fail("permission_denied")),
    }
}
pub(super) fn executable_fingerprint(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::{
        io::Read,
        os::unix::fs::{MetadataExt, OpenOptionsExt},
    };
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .checked("permission_denied")?;
    let before = file.metadata().checked("permission_denied")?;
    if !before.is_file()
        || (before.nlink() != 1 && before.uid() != 0)
        || before.len() > 128 * 1048576
        || before.mode() & 0o022 != 0
        || ![0, unsafe { libc::geteuid() }].contains(&before.uid())
    {
        return Err(fail("permission_denied"));
    }
    let mut hash = Sha256::new();
    let mut bytes = [0; 65536];
    let mut total = 0usize;
    loop {
        let count = file.read(&mut bytes).checked("permission_denied")?;
        if count == 0 {
            break;
        }
        total += count;
        if total > 128 * 1048576 {
            return Err(fail("resource_limit"));
        }
        hash.update(&bytes[..count]);
    }
    let after = std::fs::symlink_metadata(path).checked("permission_denied")?;
    if before.dev() != after.dev()
        || before.ino() != after.ino()
        || before.len() != after.len()
        || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec()
        || after.file_type().is_symlink()
    {
        return Err(fail("stale_snapshot"));
    }
    Ok(format!(
        "{}:{}:{}:{}:{}",
        before.dev(),
        before.ino(),
        before.mode(),
        before.len(),
        crate::plugin_host::external::package::hex(&hash.finalize())
    ))
}
struct Envelope {
    message: Value,
    reply: oneshot::Sender<std::result::Result<Value, &'static str>>,
}
#[derive(Clone)]
struct Bridge {
    manager: Arc<Manager>,
    lease: Lease,
    cancel: Cancellation,
    host: String,
    token: String,
    transport: Transport,
    notifications: broadcast::Sender<String>,
    telemetry: Arc<Mutex<Telemetry>>,
    connection: String,
    clock: Instant,
    core: Arc<Mutex<local_agent_core::Store>>,
    http_pending: Arc<Mutex<BTreeMap<String, Cancellation>>>,
}
#[derive(Clone)]
enum Transport {
    Stdio(mpsc::Sender<Envelope>),
    Http {
        client: reqwest::Client,
        url: url::Url,
        provider: Option<Arc<Provider>>,
    },
}
fn running(b: &Bridge) -> Result<()> {
    if b.cancel.is_cancelled() {
        return Err(fail("grant_revoked"));
    }
    b.manager.ensure_running()
}
async fn active(b: &Bridge) -> Result<()> {
    running(b)?;
    tokio::select! {
        biased;
        _ = b.cancel.cancelled() => return Err(fail("grant_revoked")),
        result = b.lease.check_async(&b.manager) => result?,
    }
    running(b)
}
fn check_interval() -> tokio::time::Interval {
    let period = Duration::from_millis(50);
    let mut interval = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    interval
}
fn unavailable_status(error: &super::super::Error, denied: StatusCode) -> StatusCode {
    if matches!(error.code, "busy" | "unavailable") {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        denied
    }
}
async fn authorize(b: &Bridge, authenticated: bool) -> std::result::Result<(), StatusCode> {
    if !authenticated {
        return Err(StatusCode::UNAUTHORIZED);
    }
    active(b)
        .await
        .map_err(|e| unavailable_status(&e, StatusCode::UNAUTHORIZED))
}
fn authenticate(b: &Bridge, r: &Request) -> bool {
    r.headers().get("host").and_then(|h| h.to_str().ok()) == Some(&b.host)
        && r.headers().get_all("authorization").iter().count() == 1
        && r.headers().get_all("host").iter().count() == 1
        && !r.headers().contains_key("origin")
        && r.uri().query().is_none()
        && r.headers()
            .get("authorization")
            .and_then(|h| h.to_str().ok())
            .is_some_and(|v| secret_eq(v, &format!("Bearer {}", b.token)))
}
fn secret_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |a, (b, c)| a | (b ^ c)) == 0
}
impl Manager {
    pub(super) async fn start_connector(
        self: &Arc<Self>,
        q: &Query,
        provider: Option<Provider>,
        core: Arc<Mutex<local_agent_core::Store>>,
    ) -> Result<Value> {
        let manager = self.clone();
        let query = q.clone();
        let (lease, grant, connection) = super::super::run_sync(move || {
            let (_, policy, lease) = manager.extension(&query)?;
            let kind = policy.connector.ok_or(fail("unsupported"))?;
            let grant = manager.extension_grant(&query)?;
            let connection: Connection =
                serde_json::from_slice(&checked_bytes(&grant)?).checked("invalid_input")?;
            validate_connection(&kind, &connection)?;
            manager.stop_connector(&query)?;
            Ok((lease, grant, connection))
        })
        .await?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .checked("unavailable")?;
        let host = listener.local_addr().checked("unavailable")?.to_string();
        let cancel = Cancellation::default();
        let telemetry = Arc::new(Mutex::new(Telemetry::default()));
        let (notifications, _) = broadcast::channel(64);
        let token =
            uuid::Uuid::new_v4().simple().to_string() + &uuid::Uuid::new_v4().simple().to_string();
        let transport = match connection {
            Connection::Stdio { command, args } => {
                let executable = PathBuf::from(&command);
                super::super::run_sync(move || {
                    if grant.executable.as_ref() != Some(&executable_fingerprint(&executable)?) {
                        return Err(fail("grant_revoked"));
                    }
                    Ok(())
                })
                .await?;
                let mut child = tokio::process::Command::new(&command)
                    .args(args)
                    .env_clear()
                    .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::null())
                    .kill_on_drop(true)
                    .spawn()
                    .checked("unavailable")?;
                let input = child.stdin.take().ok_or(fail("unavailable"))?;
                let output = BufReader::new(child.stdout.take().ok_or(fail("unavailable"))?);
                let (sender, receiver) = mpsc::channel(32);
                let b = Bridge {
                    manager: self.clone(),
                    lease: lease.clone(),
                    cancel: cancel.clone(),
                    host: host.clone(),
                    token: token.clone(),
                    transport: Transport::Stdio(sender.clone()),
                    notifications: notifications.clone(),
                    telemetry: telemetry.clone(),
                    connection: uuid::Uuid::new_v4().to_string(),
                    clock: Instant::now(),
                    core: core.clone(),
                    http_pending: Arc::new(Mutex::new(BTreeMap::new())),
                };
                tokio::spawn(async move {
                    let result = stdio(&b, input, output, receiver).await;
                    if result.is_err() {
                        b.telemetry
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .last_error = Some("transport_error");
                    }
                    b.cancel.cancel();
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                });
                Transport::Stdio(sender)
            }
            Connection::Http {
                url,
                allow_private_network,
                provider_id,
            } => {
                if provider_id.as_ref() != provider.as_ref().map(|p| &p.id) {
                    return Err(fail("permission_denied"));
                }
                let url = url::Url::parse(&url).checked("invalid_input")?;
                if let Some(provider) = &provider
                    && url.origin()
                        != url::Url::parse(&provider.endpoint)
                            .checked("invalid_input")?
                            .origin()
                {
                    return Err(fail("permission_denied"));
                }
                let client = protocol::pinned_client(&url, allow_private_network).await?;
                Transport::Http {
                    client,
                    url,
                    provider: provider.map(Arc::new),
                }
            }
            Connection::Dsh { .. } => return Err(fail("unverified_connector")),
        };
        let expected = lease.clone();
        let bridge = Bridge {
            manager: self.clone(),
            lease,
            cancel: cancel.clone(),
            host: host.clone(),
            token: token.clone(),
            transport,
            notifications,
            telemetry: telemetry.clone(),
            connection: uuid::Uuid::new_v4().to_string(),
            clock: Instant::now(),
            core,
            http_pending: Arc::new(Mutex::new(BTreeMap::new())),
        };
        let stopped = cancel.clone();
        let m = self.clone();
        let l = bridge.lease.clone();
        let task = tokio::spawn(async move {
            let router = Router::new()
                .route("/mcp", post(rpc).get(events).delete(disconnect))
                .layer(axum::extract::DefaultBodyLimit::max(1048576))
                .with_state(bridge);
            let signal = stopped.clone();
            let shutdown = async move {
                let mut interval = check_interval();
                loop {
                    tokio::select! {
                        biased;
                        _ = signal.cancelled() => break,
                        _ = interval.tick() => {
                            let result = tokio::select! {
                                biased;
                                _ = signal.cancelled() => break,
                                result = l.check_async(&m) => result,
                            };
                            if result.is_err() {
                                signal.cancel();
                                break;
                            }
                        }
                    }
                }
            };
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(shutdown)
                .await;
            stopped.cancel();
        });
        let live = Live {
            cancel,
            task,
            host,
            token,
            state: telemetry,
        };
        let manager = self.clone();
        let query = q.clone();
        super::super::run_sync(move || manager.publish_connector(&query, &expected, live)).await
    }
    fn publish_connector(&self, query: &Query, expected: &Lease, live: Live) -> Result<Value> {
        expected.check(self)?;
        {
            let mut connectors = self.connectors.lock().checked("unavailable")?;
            self.ensure_running()?;
            connectors.insert(query.plugin_id.clone(), live);
        }
        self.extension_status(query)
    }
    pub(super) fn stop_connector(&self, q: &Query) -> Result<Value> {
        self.cancel_connector(&q.plugin_id)?;
        Ok(json!({"stopped":true}))
    }
    pub(in crate::plugin_host::external) fn cancel_connector(&self, id: &str) -> Result<()> {
        if let Some(live) = self.connectors.lock().checked("unavailable")?.remove(id) {
            live.cancel.cancel();
            live.task.abort();
        }
        Ok(())
    }
    pub(crate) fn export_connector(&self, q: &Query) -> Result<Value> {
        self.extension(q)?;
        let connectors = self.connectors.lock().checked("unavailable")?;
        let l = connectors.get(&q.plugin_id).ok_or(fail("not_found"))?;
        if l.cancel.is_cancelled() || l.task.is_finished() {
            return Err(fail("unavailable"));
        }
        Ok(
            json!({"mcpServers":{q.plugin_id.clone():{"type":"http","url":format!("http://{}/mcp",l.host),"headers":{"Authorization":format!("Bearer {}",l.token)}}}}),
        )
    }
}
async fn disconnect(State(b): State<Bridge>, r: Request) -> Response {
    if let Err(status) = authorize(&b, authenticate(&b, &r)).await {
        return status.into_response();
    }
    match &b.transport {
        Transport::Stdio(_) => StatusCode::METHOD_NOT_ALLOWED.into_response(),
        Transport::Http {
            client,
            url,
            provider,
        } => {
            let call = http_request(
                &b,
                client.delete(url.clone()),
                r.headers(),
                provider.as_deref(),
            )
            .await;
            match call {
                Ok(call) => match send_http(&b, call).await {
                    Ok(r) => r.status().into_response(),
                    Err(_) => StatusCode::BAD_GATEWAY.into_response(),
                },
                Err(error) => unavailable_status(&error, StatusCode::FORBIDDEN).into_response(),
            }
        }
    }
}
async fn events(State(b): State<Bridge>, r: Request) -> Response {
    if let Err(status) = authorize(&b, authenticate(&b, &r)).await {
        return status.into_response();
    }
    match &b.transport {
        Transport::Stdio(_) => {
            let mut events = b.notifications.subscribe();
            let stream = async_stream::stream! {
                let mut interval = check_interval();
                loop {
                    tokio::select! {
                        biased;
                        _ = b.cancel.cancelled() => break,
                        _ = interval.tick() => {
                            if active(&b).await.is_err() { break; }
                        }
                        event = events.recv() => {
                            let Ok(event) = event else { break; };
                            if running(&b).is_err() { break; }
                            yield Ok::<Bytes, std::io::Error>(Bytes::from(format!("event: message\ndata: {event}\n\n")));
                        }
                    }
                }
            };
            (
                [
                    ("content-type", "text/event-stream"),
                    ("cache-control", "no-cache"),
                ],
                Body::from_stream(stream),
            )
                .into_response()
        }
        Transport::Http {
            client,
            url,
            provider,
        } => {
            let call = http_request(
                &b,
                client
                    .get(url.clone())
                    .header("accept", "text/event-stream"),
                r.headers(),
                provider.as_deref(),
            )
            .await;
            match call {
                Ok(call) => match send_http(&b, call).await {
                    Ok(r) => http_response(b.clone(), r, None),
                    Err(_) => StatusCode::BAD_GATEWAY.into_response(),
                },
                Err(error) => unavailable_status(&error, StatusCode::FORBIDDEN).into_response(),
            }
        }
    }
}
async fn rpc(State(b): State<Bridge>, request: Request) -> Response {
    if let Err(status) = authorize(&b, authenticate(&b, &request)).await {
        return status.into_response();
    }
    let headers = request.headers().clone();
    let body = match tokio::time::timeout(
        Duration::from_secs(10),
        to_bytes(request.into_body(), 1048576),
    )
    .await
    {
        Ok(Ok(v)) => v,
        _ => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let message: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let is_response = message.get("id").is_some()
        && message.get("method").is_none()
        && (message.get("result").is_some() != message.get("error").is_some());
    if message["jsonrpc"] != "2.0"
        || (message["method"] == "tools/call" && message.get("id").is_none())
        || (!message["method"].is_string() && !is_response)
        || message.is_array()
        || message
            .get("id")
            .is_some_and(|v| !(v.is_string() || v.is_number()))
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    match &b.transport {
        Transport::Stdio(sender) => {
            let (send, recv) = oneshot::channel();
            if sender
                .try_send(Envelope {
                    message,
                    reply: send,
                })
                .is_err()
            {
                return StatusCode::TOO_MANY_REQUESTS.into_response();
            }
            match tokio::time::timeout(Duration::from_secs(35), recv).await {
                Ok(Ok(Ok(v))) => {
                    if running(&b).is_err() {
                        return StatusCode::FORBIDDEN.into_response();
                    }
                    if v.is_null() {
                        StatusCode::ACCEPTED.into_response()
                    } else {
                        axum::Json(v).into_response()
                    }
                }
                _ => StatusCode::BAD_GATEWAY.into_response(),
            }
        }
        Transport::Http {
            client,
            url,
            provider,
        } => {
            let key = |id: &Value| {
                format!(
                    "{}:{}",
                    headers
                        .get("mcp-session-id")
                        .and_then(|h| h.to_str().ok())
                        .unwrap_or(""),
                    id
                )
            };
            if message["method"] == "notifications/cancelled"
                && let Some(cancel) = b
                    .http_pending
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .get(&key(&message["params"]["requestId"]))
            {
                cancel.cancel();
            }
            let mut watch = if message["method"] == "tools/call" {
                let mut pending = b.http_pending.lock().unwrap_or_else(|p| p.into_inner());
                let id = key(&message["id"]);
                if pending.len() >= 64 || pending.contains_key(&id) {
                    return StatusCode::TOO_MANY_REQUESTS.into_response();
                }
                let cancel = Cancellation::default();
                pending.insert(id.clone(), cancel.clone());
                Some(HttpCall {
                    bridge: b.clone(),
                    key: id,
                    cancel,
                    request: message.clone(),
                    start: Instant::now(),
                    finished: false,
                })
            } else {
                None
            };
            let call = http_request(
                &b,
                client
                    .post(url.clone())
                    .header("content-type", "application/json")
                    .header("accept", "application/json, text/event-stream")
                    .body(body),
                &headers,
                provider.as_deref(),
            )
            .await;
            let call = match call {
                Ok(c) => c,
                Err(error) => {
                    return unavailable_status(&error, StatusCode::FORBIDDEN).into_response();
                }
            };
            let start = Instant::now();
            let cancelled = async {
                if let Some(watch) = &watch {
                    watch.cancel.cancelled().await;
                } else {
                    std::future::pending::<()>().await;
                }
            };
            let sent = tokio::select! {
                biased;
                _ = b.cancel.cancelled() => None,
                _ = cancelled => None,
                v = tokio::time::timeout(Duration::from_secs(30),call.send()) => Some(v),
            };
            match sent {
                Some(Ok(Ok(response))) => {
                    http_response_watched(b.clone(), response, Some((message, start)), watch)
                }
                _ => {
                    if let Some(w) = watch.as_mut() {
                        w.finish(if w.cancel.is_cancelled() {
                            "cancelled"
                        } else if b.cancel.is_cancelled() {
                            "incomplete"
                        } else if sent.is_some_and(|r| r.is_err()) {
                            "timeout"
                        } else {
                            "transport_error"
                        });
                    }
                    StatusCode::BAD_GATEWAY.into_response()
                }
            }
        }
    }
}
async fn http_request(
    b: &Bridge,
    mut call: reqwest::RequestBuilder,
    headers: &HeaderMap,
    provider: Option<&Provider>,
) -> Result<reqwest::RequestBuilder> {
    active(b).await?;
    for key in ["mcp-session-id", "mcp-protocol-version", "last-event-id"] {
        if let Some(v) = headers.get(key) {
            call = call.header(key, v);
        }
    }
    if let Some(p) = provider {
        let bridge = b.clone();
        let p = p.clone();
        let secret = super::super::run_sync(move || {
            let current = bridge
                .core
                .lock()
                .checked("unavailable")?
                .providers()
                .checked("unavailable")?
                .into_iter()
                .find(|v| v.id == p.id)
                .ok_or(fail("grant_revoked"))?;
            if local_agent_core::providers::fingerprint(&current)
                != local_agent_core::providers::fingerprint(&p)
            {
                return Err(fail("grant_revoked"));
            }
            p.credential_ref
                .as_ref()
                .map(|reference| {
                    bridge
                        .manager
                        .vault
                        .get(reference)
                        .checked("permission_denied")
                })
                .transpose()
        })
        .await?;
        if let Some(secret) = secret {
            call = call.bearer_auth(std::str::from_utf8(&secret).checked("permission_denied")?);
        }
    }
    active(b).await?;
    Ok(call)
}
async fn send_http(b: &Bridge, call: reqwest::RequestBuilder) -> Result<reqwest::Response> {
    running(b)?;
    tokio::select! {
        biased;
        _ = b.cancel.cancelled() => Err(fail("grant_revoked")),
        response = tokio::time::timeout(Duration::from_secs(30), call.send()) => {
            let response = response.checked("timeout")?.checked("transport_error")?;
            running(b)?;
            Ok(response)
        }
    }
}
fn http_response(
    b: Bridge,
    response: reqwest::Response,
    request: Option<(Value, Instant)>,
) -> Response {
    http_response_watched(b, response, request, None)
}
struct HttpCall {
    bridge: Bridge,
    key: String,
    cancel: Cancellation,
    request: Value,
    start: Instant,
    finished: bool,
}
impl HttpCall {
    fn finish(&mut self, status: &str) {
        if !self.finished {
            record(&self.bridge, &self.request, None, status, self.start);
            self.finished = true;
        }
    }
}
impl Drop for HttpCall {
    fn drop(&mut self) {
        if !self.finished {
            self.finish(if self.cancel.is_cancelled() {
                "cancelled"
            } else {
                "incomplete"
            });
        }
        self.bridge
            .http_pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&self.key);
    }
}
fn http_response_watched(
    b: Bridge,
    response: reqwest::Response,
    request: Option<(Value, Instant)>,
    mut watch: Option<HttpCall>,
) -> Response {
    let status = response.status();
    let sse = response
        .headers()
        .get("content-type")
        .and_then(|s| s.to_str().ok())
        .is_some_and(|s| s.starts_with("text/event-stream"));
    let mut headers = HeaderMap::new();
    for name in [
        "content-type",
        "mcp-session-id",
        "mcp-protocol-version",
        "retry-after",
    ] {
        if let Some(v) = response.headers().get(name) {
            headers.insert(axum::http::HeaderName::from_static(name), v.clone());
        }
    }
    let stream = async_stream::try_stream! {
        let mut input=response.bytes_stream();let mut parser=Vec::new();let mut seen=false;
        let mut interval = check_interval();
        loop {
            running(&b).map_err(|e| std::io::Error::other(e.code))?;
            if let Some(w)=watch.as_mut(){
                if w.cancel.is_cancelled(){w.finish("cancelled");Err(std::io::Error::other("cancelled"))?;}
                if !w.finished && w.start.elapsed()>Duration::from_secs(30){w.finish("timeout");Err(std::io::Error::other("timeout"))?;}
            }
            let call_cancelled = async {
                if let Some(watch) = &watch {
                    watch.cancel.cancelled().await;
                } else {
                    std::future::pending::<()>().await;
                }
            };
            let event = tokio::select! {
                biased;
                _ = b.cancel.cancelled() => Err("grant_revoked"),
                _ = call_cancelled => Err("cancelled"),
                _ = interval.tick() => active(&b).await.map(|()| None).map_err(|e| e.code),
                next = input.next() => Ok(Some(next)),
            };
            let next = event.map_err(|code| {
                if code == "cancelled" && let Some(watch) = watch.as_mut() { watch.finish("cancelled"); }
                std::io::Error::other(code)
            })?;
            let Some(next) = next else { continue; };
            let Some(next)=next else{break};let chunk=next.map_err(|_|std::io::Error::other("transport_error"))?;
            running(&b).map_err(|e| std::io::Error::other(e.code))?;
            if parser.len()+chunk.len()>1048576 {Err(std::io::Error::other("resource_limit"))?;}
            parser.extend_from_slice(&chunk);
            if sse {
                while let Some((end,width))=parser.windows(2).position(|s|s==b"\n\n").map(|n|(n,2)).into_iter().chain(parser.windows(4).position(|s|s==b"\r\n\r\n").map(|n|(n,4))).min_by_key(|(n,_)|*n) {
                    let frame:Vec<_>=parser.drain(..end+width).collect();
                    let data=String::from_utf8_lossy(&frame).lines().filter_map(|l|l.strip_prefix("data:").map(str::trim_start)).collect::<Vec<_>>().join("\n");
                    if let Ok(v)=serde_json::from_str::<Value>(&data){observe(&b,&request,&v,&mut seen);if seen&& let Some(w)=watch.as_mut(){w.finished=true;}}
                }
            }
            running(&b).map_err(|e| std::io::Error::other(e.code))?;
            yield chunk;
        }
        if !sse&& let Ok(v)=serde_json::from_slice::<Value>(&parser){observe(&b,&request,&v,&mut seen);if seen&& let Some(w)=watch.as_mut(){w.finished=true;}}
        if let Some(w)=watch.as_mut()&& !seen {w.finish("incomplete");}
    };
    let stream: futures_util::stream::BoxStream<
        'static,
        std::result::Result<Bytes, std::io::Error>,
    > = Box::pin(stream);
    (status, headers, Body::from_stream(stream)).into_response()
}
fn observe(b: &Bridge, request: &Option<(Value, Instant)>, value: &Value, seen: &mut bool) {
    if value["method"].is_string() {
        b.telemetry
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .notifications += 1;
    }
    if let Some((r, start)) = request
        && value.get("id") == r.get("id")
        && value.get("id").is_some()
        && !*seen
    {
        *seen = true;
        update(b, r, value, *start);
    }
}
struct Pending {
    request: Value,
    reply: oneshot::Sender<std::result::Result<Value, &'static str>>,
    start: Instant,
}
enum StdioInput {
    Check,
    Frame(Result<Vec<u8>>),
    Request(Option<Envelope>),
}
async fn stdio(
    b: &Bridge,
    input: tokio::process::ChildStdin,
    output: BufReader<tokio::process::ChildStdout>,
    requests: mpsc::Receiver<Envelope>,
) -> Result<()> {
    let mut pending: BTreeMap<String, Pending> = BTreeMap::new();
    let result = stdio_loop(b, input, output, requests, &mut pending).await;
    for (_, call) in pending {
        record(b, &call.request, None, "incomplete", call.start);
        let _ = call.reply.send(Err("incomplete"));
    }
    result
}
async fn stdio_loop(
    b: &Bridge,
    mut input: tokio::process::ChildStdin,
    mut output: BufReader<tokio::process::ChildStdout>,
    mut requests: mpsc::Receiver<Envelope>,
    pending: &mut BTreeMap<String, Pending>,
) -> Result<()> {
    let mut interval = check_interval();
    loop {
        let reading = crate::plugin_host::external::broker::read_frame(&mut output);
        tokio::pin!(reading);
        loop {
            let event = tokio::select! {
                biased;
                _ = b.cancel.cancelled() => return Err(fail("grant_revoked")),
                _ = interval.tick() => StdioInput::Check,
                event = async {
                    // Keep cancellation/deadlines first, but remain fair between
                    // busy stdout and client cancellation/response messages.
                    tokio::select! {
                        frame = &mut reading => StdioInput::Frame(frame),
                        request = requests.recv() => StdioInput::Request(request),
                    }
                } => event,
            };
            match event {
                StdioInput::Check => {
                    active(b).await?;
                    let expired: Vec<_> = pending
                        .iter()
                        .filter(|(_, p)| p.start.elapsed() > Duration::from_secs(30))
                        .map(|(id, _)| id.clone())
                        .collect();
                    for id in expired {
                        if let Some(p) = pending.remove(&id) {
                            record(b, &p.request, None, "timeout", p.start);
                            let _ = p.reply.send(Err("timeout"));
                            write(b, &mut input,&json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":p.request["id"],"reason":"timeout"}})).await?;
                        }
                    }
                }
                StdioInput::Frame(frame) => {
                    let value: Value = serde_json::from_slice(&frame?).checked("invalid_input")?;
                    if value["jsonrpc"] != "2.0" {
                        return Err(fail("invalid_input"));
                    }
                    running(b)?;
                    if let Some(id) = value.get("id") {
                        if value.get("method").is_some() {
                            let _ = b.notifications.send(value.to_string());
                        } else if let Some(p) = pending.remove(&id.to_string()) {
                            update(b, &p.request, &value, p.start);
                            let _ = p.reply.send(Ok(value));
                        }
                    } else if value["method"].is_string() {
                        b.telemetry
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .notifications += 1;
                        let _ = b.notifications.send(value.to_string());
                    }
                    break;
                }
                StdioInput::Request(e) => {
                    let Some(e) = e else { return Ok(()) };
                    active(b).await?;
                    if e.message.get("method").is_none() {
                        write(b, &mut input, &e.message).await?;
                        let _ = e.reply.send(Ok(Value::Null));
                    } else if let Some(id) = e.message.get("id") {
                        let id = id.to_string();
                        if pending.len() >= 64 || pending.contains_key(&id) {
                            let _ = e.reply.send(Err("duplicate_request"));
                            continue;
                        }
                        pending.insert(
                            id.clone(),
                            Pending {
                                request: e.message,
                                reply: e.reply,
                                start: Instant::now(),
                            },
                        );
                        write(b, &mut input, &pending[&id].request).await?;
                    } else {
                        write(b, &mut input, &e.message).await?;
                        if e.message["method"] == "notifications/cancelled" {
                            let id = e.message["params"]["requestId"].to_string();
                            if let Some(p) = pending.remove(&id) {
                                record(b, &p.request, None, "cancelled", p.start);
                                let _ = p.reply.send(Err("cancelled"));
                            }
                        }
                        let _ = e.reply.send(Ok(Value::Null));
                    }
                }
            }
        }
    }
}
async fn write(b: &Bridge, input: &mut tokio::process::ChildStdin, v: &Value) -> Result<()> {
    let mut bytes = serde_json::to_vec(v).checked("invalid_input")?;
    bytes.push(b'\n');
    if bytes.len() > 1048576 {
        return Err(fail("resource_limit"));
    }
    running(b)?;
    tokio::select! {
        biased;
        _ = b.cancel.cancelled() => Err(fail("grant_revoked")),
        result = tokio::time::timeout(Duration::from_secs(5), async {
            input.write_all(&bytes).await?;
            input.flush().await
        }) => result.checked("deadline_exceeded")?.checked("unavailable"),
    }
}
fn update(b: &Bridge, request: &Value, response: &Value, start: Instant) {
    if request["method"] == "initialize"
        && response["jsonrpc"] == "2.0"
        && response["result"]["protocolVersion"].is_string()
        && response["result"]["capabilities"].is_object()
        && response["result"]["serverInfo"]["name"].is_string()
        && response["result"]["serverInfo"]["version"].is_string()
    {
        b.telemetry
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .connected = true;
    }
    if request["method"] == "tools/list"
        && let Some(tools) = response["result"]["tools"].as_array()
    {
        b.telemetry.lock().unwrap_or_else(|p| p.into_inner()).tools = Some(tools.len());
    }
    if request["method"] == "tools/call" {
        let state = if response["jsonrpc"] != "2.0"
            || response.get("error").is_some()
            || !response["result"].is_object()
        {
            "protocol_error"
        } else if response["result"]["isError"] == true {
            "tool_error"
        } else {
            "success"
        };
        record(b, request, Some(response), state, start);
    }
}
fn record(b: &Bridge, r: &Value, _response: Option<&Value>, state: &str, start: Instant) {
    if r["method"] != "tools/call" {
        return;
    }
    let identity = format!(
        "{}:{}:{}",
        b.connection,
        r["id"],
        start.saturating_duration_since(b.clock).as_nanos()
    );
    let tool = r["params"]["name"].as_str().unwrap_or("unknown");
    let record = json!({"identity":identity,"rpcId":r["id"],"mcpConnection":b.connection,"tool":tool.chars().filter(|c|!c.is_control()).take(160).collect::<String>(),"status":state,"durationMs":if ["success","tool_error","protocol_error"].contains(&state){Some(start.elapsed().as_secs_f64()*1000.)}else{None},"observedAt":chrono::Utc::now().to_rfc3339(),"agentSession":null,"content":"not-persisted","modelTokens":null});
    b.telemetry
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .pending_records += 1;
    let ticket = RecordTicket {
        telemetry: b.telemetry.clone(),
        confirmed: None,
    };
    let manager = b.manager.clone();
    let id = b.lease.id.clone();
    let _ = crate::blocking::try_spawn(move || {
        let mut ticket = ticket;
        let result = (|| {
            let store = manager.advanced.lock().checked("unavailable")?;
            let db = store.db()?;
            db.execute(
                "INSERT OR IGNORE INTO connector_calls(plugin_id,identity,record) VALUES(?1,?2,?3)",
                params![id, identity, record.to_string()],
            )
            .checked("unavailable")
        })();
        if let Ok(changed) = result {
            ticket.confirmed = Some(changed == 1);
        }
    });
}
struct RecordTicket {
    telemetry: Arc<Mutex<Telemetry>>,
    confirmed: Option<bool>,
}
impl Drop for RecordTicket {
    fn drop(&mut self) {
        let mut telemetry = self.telemetry.lock().unwrap_or_else(|p| p.into_inner());
        telemetry.pending_records = telemetry.pending_records.saturating_sub(1);
        match self.confirmed {
            Some(changed) => telemetry.calls += u64::from(changed),
            None => {
                telemetry.unrecorded_calls = telemetry.unrecorded_calls.saturating_add(1);
                telemetry.last_error = Some("observation_unavailable");
            }
        }
    }
}

#[cfg(test)]
mod publication_tests {
    use super::*;
    use local_agent_core::plugin::external::{StateAction, StateRequest};

    fn pending() -> (Live, tokio::task::AbortHandle, Cancellation) {
        let cancel = Cancellation::default();
        let task = tokio::spawn(std::future::pending());
        let abort = task.abort_handle();
        (
            Live {
                cancel: cancel.clone(),
                task,
                host: "127.0.0.1:1".into(),
                token: "fixture".into(),
                state: Arc::new(Mutex::new(Telemetry::default())),
            },
            abort,
            cancel,
        )
    }
    async fn stopped(abort: &tokio::task::AbortHandle) {
        tokio::time::timeout(Duration::from_secs(1), async {
            while !abort.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn revocation_and_reenable_cannot_publish_a_connector_from_an_old_lease() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let (manager, query) = super::super::record_tests::host(&root);
        let manager = Arc::new(manager);
        manager
            .change(StateRequest {
                plugin_id: query.plugin_id.clone(),
                digest: query.digest.clone(),
                action: StateAction::Enable,
            })
            .await
            .unwrap();
        let expected = Lease::from(&manager.status(&query.plugin_id, &query.digest).unwrap());
        manager
            .revoke(&query.plugin_id, &query.digest, true)
            .unwrap();
        manager
            .change(StateRequest {
                plugin_id: query.plugin_id.clone(),
                digest: query.digest.clone(),
                action: StateAction::Enable,
            })
            .await
            .unwrap();
        let (live, abort, cancel) = pending();
        assert_eq!(
            manager
                .publish_connector(&query, &expected, live)
                .unwrap_err()
                .code,
            "grant_revoked"
        );
        assert!(manager.connectors.lock().unwrap().is_empty());
        assert!(cancel.is_cancelled());
        stopped(&abort).await;
    }

    #[tokio::test]
    async fn cancellation_before_admission_stops_an_unpublished_connector() {
        let (live, abort, cancel) = pending();
        let work = super::super::super::run_sync(move || {
            drop(live);
            Ok(())
        });
        drop(work);
        assert!(cancel.is_cancelled());
        stopped(&abort).await;
    }
}

#[cfg(test)]
mod observation_tests {
    use super::*;
    use local_agent_core::plugin::external::{StateAction, StateRequest};

    pub(super) async fn bridge(root: &Path) -> Bridge {
        let (manager, query) = super::super::record_tests::host(root);
        let manager = Arc::new(manager);
        let status = manager
            .change(StateRequest {
                plugin_id: query.plugin_id,
                digest: query.digest,
                action: StateAction::Enable,
            })
            .await
            .unwrap();
        let (sender, _) = mpsc::channel(1);
        let (notifications, _) = broadcast::channel(1);
        Bridge {
            lease: Lease::from(&status),
            manager,
            cancel: Cancellation::default(),
            host: "127.0.0.1:1".into(),
            token: "fixture".into(),
            transport: Transport::Stdio(sender),
            notifications,
            telemetry: Arc::new(Mutex::new(Telemetry::default())),
            connection: "fixture".into(),
            clock: Instant::now(),
            core: Arc::new(Mutex::new(local_agent_core::Store::open(root).unwrap())),
            http_pending: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }
    pub(super) async fn settled(bridge: &Bridge) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while bridge.telemetry.lock().unwrap().pending_records != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn actual_proxy_metadata_projects_to_tool_facts_without_content_or_invented_session() {
        use local_agent_core::observation::retention::{FactLedger, ToolFilter};
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let bridge = bridge(&root).await;
        let private = "private-tool-content-must-not-persist";
        let request = |id| json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"search","arguments":{"private":private}}});
        for (id, result) in [
            (
                1,
                json!({"result":{"content":[{"type":"text","text":private}]}}),
            ),
            (
                2,
                json!({"result":{"isError":true,"content":[{"type":"text","text":private}]}}),
            ),
            (3, json!({"error":{"code":-32000,"message":private}})),
        ] {
            let mut response = result;
            response["jsonrpc"] = json!("2.0");
            response["id"] = json!(id);
            update(&bridge, &request(id), &response, Instant::now());
        }
        for (id, state) in [(4, "timeout"), (5, "cancelled"), (6, "transport_error")] {
            record(&bridge, &request(id), None, state, Instant::now());
        }
        update(
            &bridge,
            &json!({"jsonrpc":"2.0","id":7,"method":"tools/list"}),
            &json!({"jsonrpc":"2.0","id":7,"result":{"tools":[]}}),
            Instant::now(),
        );
        settled(&bridge).await;
        let inventory = bridge
            .manager
            .retention_tool_impact(30, chrono::Utc::now())
            .unwrap();
        assert_eq!(inventory.records, 6);
        assert_eq!(inventory.within_window, 3);
        assert_eq!(inventory.protected.unknown_outcome, 3);
        assert_eq!(inventory.capacity_limit, Some(10_000));
        assert!(inventory.classification_complete);
        let store = bridge.manager.advanced.lock().unwrap();
        let db = store.db().unwrap();
        assert!(
            !db.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='retention_tool_staging')",
                [],
                |r| r.get::<_, bool>(0)
            )
            .unwrap()
        );
        let raw = db
            .prepare("SELECT id,record FROM connector_calls ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(raw.len(), 6);
        assert!(raw.iter().all(|(_, r)| !r.contains(private)));
        let ledger =
            FactLedger::for_connector_store(db, "00000000-0000-4000-8000-000000000001").unwrap();
        ledger
            .stage_tool_batch(&raw.iter().map(|(id, _)| *id).collect::<Vec<_>>())
            .unwrap();
        let summary = ledger.tool_staging_summary(&ToolFilter::default()).unwrap();
        assert_eq!(
            (
                summary.calls,
                summary.returned,
                summary.tool_errors,
                summary.protocol_errors,
                summary.transport_errors,
                summary.timeouts,
                summary.cancelled
            ),
            (6, 1, 1, 1, 1, 1, 1)
        );
        assert_eq!(summary.duration_samples, 3);
        assert_eq!(summary.error_denominator, 4);
        assert_eq!(summary.error_rate, Some(0.75));
        assert_eq!(summary.model_tokens, None);
        assert_eq!(summary.unknown_agent_calls, 6);
        assert_eq!(summary.unknown_session_calls, 6);
        assert_eq!(summary.unknown_service_calls, 6);
        assert_eq!(summary.coverage, "staged_only");
        let after = db
            .prepare("SELECT id,record FROM connector_calls ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(after, raw);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dropped_response_and_shutdown_do_not_wait_for_observation_storage() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let bridge = bridge(&root).await;
        let (held, ready) = tokio::sync::oneshot::channel();
        let (release, gate) = std::sync::mpsc::channel();
        let holder = std::thread::spawn({
            let manager = bridge.manager.clone();
            move || {
                let _guard = manager.advanced.lock().unwrap();
                held.send(()).unwrap();
                let _ = gate.recv_timeout(Duration::from_secs(4));
            }
        });
        ready.await.unwrap();
        let began = Instant::now();
        let watch = HttpCall {
            bridge: bridge.clone(),
            key: "fixture".into(),
            cancel: Cancellation::default(),
            request: json!({"jsonrpc":"2.0","method":"tools/call","id":1,"params":{"name":"safe","arguments":{"secret":"do-not-persist"}}}),
            start: Instant::now(),
            finished: false,
        };
        drop(watch);
        assert_eq!(bridge.telemetry.lock().unwrap().pending_records, 1);
        bridge.manager.stop();
        let elapsed = began.elapsed();
        release.send(()).unwrap();
        holder.join().unwrap();
        settled(&bridge).await;
        assert!(elapsed < Duration::from_secs(1));
        let telemetry = bridge.telemetry.lock().unwrap();
        assert_eq!(
            (
                telemetry.calls,
                telemetry.unrecorded_calls,
                telemetry.pending_records
            ),
            (1, 0, 0)
        );
        drop(telemetry);
        let store = bridge.manager.advanced.lock().unwrap();
        let record: String = store
            .db()
            .unwrap()
            .query_row("SELECT record FROM connector_calls", [], |r| r.get(0))
            .unwrap();
        assert!(!record.contains("do-not-persist"));
        assert_eq!(
            serde_json::from_str::<Value>(&record).unwrap()["status"],
            "incomplete"
        );
    }

    #[tokio::test]
    async fn failed_metadata_write_is_reported_without_changing_a_tool_result() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let bridge = bridge(&root).await;
        bridge.manager.advanced.lock().unwrap().db().unwrap().execute_batch("CREATE TRIGGER reject_call BEFORE INSERT ON connector_calls BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
        let request =
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"safe"}});
        let response = json!({"jsonrpc":"2.0","id":2,"result":{"content":[]}});
        let original = response.clone();
        update(&bridge, &request, &response, Instant::now());
        settled(&bridge).await;
        assert_eq!(response, original);
        let telemetry = bridge.telemetry.lock().unwrap();
        assert_eq!(
            (
                telemetry.calls,
                telemetry.unrecorded_calls,
                telemetry.pending_records
            ),
            (0, 1, 0)
        );
        assert_eq!(telemetry.last_error, Some("observation_unavailable"));
    }
}

#[cfg(test)]
mod cancellation_tests {
    use super::observation_tests::{bridge, settled};
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn authorization_wait_is_cancelled_before_a_locked_database_is_released() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let bridge = bridge(&root).await;
        let (held, ready) = oneshot::channel();
        let (release, gate) = std::sync::mpsc::channel();
        let holder = std::thread::spawn({
            let manager = bridge.manager.clone();
            move || {
                let _lock = manager.db.lock().unwrap();
                held.send(()).unwrap();
                let _ = gate.recv_timeout(Duration::from_secs(3));
            }
        });
        ready.await.unwrap();
        let checking = tokio::spawn({
            let bridge = bridge.clone();
            async move { active(&bridge).await }
        });
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!checking.is_finished());
        bridge.cancel.cancel();
        let result = tokio::time::timeout(Duration::from_millis(500), checking).await;
        release.send(()).unwrap();
        holder.join().unwrap();
        assert_eq!(result.unwrap().unwrap().unwrap_err().code, "grant_revoked");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn busy_stdio_notifications_cannot_postpone_an_expired_call_or_stop() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let bridge = bridge(&root).await;
        let mut child = tokio::process::Command::new("/usr/bin/python3")
            .args(["-u", "-c", "import time\nwhile True:\n print('{\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}',flush=True)\n time.sleep(0.001)"])
            .env_clear()
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn().unwrap();
        let input = child.stdin.take().unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        tokio::time::timeout(
            Duration::from_secs(2),
            crate::plugin_host::external::broker::read_frame(&mut output),
        )
        .await
        .unwrap()
        .unwrap();
        let (_sender, receiver) = mpsc::channel(1);
        let (reply, received) = oneshot::channel();
        let mut pending = BTreeMap::from([(
            "1".to_owned(),
            Pending {
                request: json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"fixture"}}),
                reply,
                start: Instant::now() - Duration::from_secs(31),
            },
        )]);
        let worker = tokio::spawn({
            let bridge = bridge.clone();
            async move { stdio_loop(&bridge, input, output, receiver, &mut pending).await }
        });
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), received)
                .await
                .unwrap()
                .unwrap(),
            Err("timeout")
        );
        assert!(bridge.telemetry.lock().unwrap().notifications > 0);
        bridge.cancel.cancel();
        assert_eq!(
            tokio::time::timeout(Duration::from_millis(500), worker)
                .await
                .unwrap()
                .unwrap()
                .unwrap_err()
                .code,
            "grant_revoked"
        );
        child.kill().await.unwrap();
        child.wait().await.unwrap();
        settled(&bridge).await;
        assert_eq!(bridge.telemetry.lock().unwrap().calls, 1);
    }
}
