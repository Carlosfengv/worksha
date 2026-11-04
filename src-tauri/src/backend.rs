use local_agent_core::runtime::{self, ServiceRecord};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
mod context;
mod lifecycle;
#[cfg(test)]
pub(crate) mod test_server;
pub(crate) use context::ConnectionContext;
pub(crate) use lifecycle::Diagnostics;
use lifecycle::{Category, Connection, Lifecycle, Phase, Status};
use tauri::Manager;
use tauri_plugin_shell::{
    ShellExt,
    process::{CommandChild, CommandEvent},
};

pub(crate) struct Backend {
    client: reqwest::Client,
    lifecycle: Arc<Mutex<Lifecycle>>,
    child: Arc<Mutex<Option<(String, CommandChild)>>>,
    reconnect_capacity: Arc<tokio::sync::Semaphore>,
}

impl Backend {
    fn new(
        client: reqwest::Client,
        connection: Option<Connection>,
        startup_error: Option<String>,
    ) -> Self {
        Self {
            client,
            lifecycle: Arc::new(Mutex::new(Lifecycle::new(connection, startup_error))),
            child: Arc::new(Mutex::new(None)),
            reconnect_capacity: Arc::new(tokio::sync::Semaphore::new(1)),
        }
    }
    fn state(&self) -> Result<std::sync::MutexGuard<'_, Lifecycle>, String> {
        self.lifecycle
            .lock()
            .map_err(|_| "后台连接状态暂不可读，请重新打开应用".into())
    }
    pub(crate) fn diagnostics(&self) -> Result<Diagnostics, String> {
        Ok(self.state()?.snapshot())
    }
    pub(crate) async fn reconnect(
        &self,
        directory: &std::path::Path,
    ) -> Result<Diagnostics, String> {
        let permit = Arc::new(
            self.reconnect_capacity
                .clone()
                .try_acquire_owned()
                .map_err(|_| local_error("backend_reconnect_busy", "已有连接核对正在进行"))?,
        );
        let operation = uuid::Uuid::new_v4().to_string();
        {
            let mut state = self.state()?;
            if state.stopping {
                return Err(local_error("backend_stopping", "应用正在退出，未更换连接"));
            }
            state.status = Status::Reconnecting;
            state.recovery_id = Some(operation.clone());
            state.record(
                &operation,
                Phase::Recovery,
                Category::ReconnectStarted,
                None,
                None,
            );
        }
        let _reset = RecoveryReset {
            lifecycle: self.lifecycle.clone(),
            operation: operation.clone(),
        };
        let result: Result<Diagnostics, String> = async {
            let record = existing_with_capacity(directory, &self.client, Some(permit.clone()))
                .await?
                .ok_or("未找到可验证的后台。请先启动已有常驻服务，或正常重新打开应用后再试")?;
            let connection = Connection::from(record);
            // A verified publication cannot transfer ownership of a live child.
            if self
                .child
                .lock()
                .map_err(|_| "后台子进程状态暂不可读")?
                .as_ref()
                .is_some_and(|(generation, _)| generation != &connection.instance_id)
            {
                return Err("原后台仍由当前应用持有，未切换到另一个实例".into());
            }
            let mut state = self.state()?;
            state.install(connection)?;
            state.record(
                &operation,
                Phase::Recovery,
                Category::Reconnected,
                None,
                None,
            );
            Ok(state.snapshot())
        }
        .await;
        if result.is_err()
            && let Ok(mut state) = self.state()
            && state.recovery_id.as_deref() == Some(&operation)
            && !state.stopping
        {
            state.status = Status::Disconnected;
            state.record(
                &operation,
                Phase::Recovery,
                Category::ReconnectFailed,
                None,
                None,
            );
        }
        result.map_err(|reason| local_error("backend_reconnect_failed", &reason))
    }
    fn connection(&self) -> Result<Connection, String> {
        let state = self.state()?;
        if state.stopping {
            return Err(local_error("backend_stopping", "应用正在退出，未发送操作"));
        }
        if state.status != Status::Connected {
            return Err(local_error(
                "backend_disconnected",
                state
                    .startup_error
                    .as_deref()
                    .unwrap_or("本地连接未确认，请在设置中核对并重新连接后台"),
            ));
        }
        state
            .connection
            .clone()
            .ok_or_else(|| local_error("backend_disconnected", "尚未连接本地后台"))
    }
    #[cfg(test)]
    pub(crate) fn test_connection(endpoint: String) -> Self {
        Self::test_connection_mode(endpoint, false)
    }
    #[cfg(test)]
    pub(crate) fn test_connection_mode(endpoint: String, persistent: bool) -> Self {
        Self::test_with_client(
            reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap(),
            endpoint,
            "export-fixture-token".into(),
            persistent,
        )
    }
    #[cfg(test)]
    pub(crate) fn test_install_connection(&self, endpoint: String) {
        self.state()
            .unwrap()
            .install(Connection {
                endpoint,
                token: "replacement-fixture-token".into(),
                persistent: false,
                generation: uuid::Uuid::new_v4().to_string(),
                instance_id: uuid::Uuid::new_v4().to_string(),
            })
            .unwrap();
    }
    #[cfg(test)]
    fn test_with_client(
        client: reqwest::Client,
        endpoint: String,
        token: String,
        persistent: bool,
    ) -> Self {
        Self::new(
            client,
            Some(Connection {
                endpoint,
                token,
                persistent,
                generation: uuid::Uuid::new_v4().to_string(),
                instance_id: uuid::Uuid::new_v4().to_string(),
            }),
            None,
        )
    }
    pub(crate) fn context(&self) -> Result<ConnectionContext, String> {
        Ok(ConnectionContext::new(
            self.client.clone(),
            self.lifecycle.clone(),
            self.connection()?,
        ))
    }
    pub(crate) async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        self.context()?.request(method, path, body).await
    }
    async fn send(
        client: &reqwest::Client,
        connection: &Connection,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        let mutation = !read_only_request(&method, path);
        let mut request = client
            .request(method, format!("{}{path}", connection.endpoint))
            .bearer_auth(&connection.token);
        if path.starts_with("/api/plugins") || path.starts_with("/api/plugin-extensions") {
            request = request.timeout(Duration::from_secs(35));
        }
        if path.starts_with("/api/provider-")
            || path.ends_with("/verify")
            || path.starts_with("/api/account")
        {
            request = request.timeout(Duration::from_secs(30));
        }
        if path == "/api/provider-connect" || path == "/api/provider-operations" {
            request = request.timeout(Duration::from_secs(60));
        }
        if [
            "/api/request-",
            "/api/agent-usage",
            "/api/provider-usage",
            "/api/overview",
            "/api/associated-costs",
            "/api/sessions",
            "/api/projects",
        ]
        .iter()
        .any(|prefix| path.starts_with(prefix))
        {
            request = request.timeout(Duration::from_secs(120));
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let mut response = request.send().await.map_err(|reason| {
            transport_error(
                if reason.is_timeout() {
                    "transport_timeout"
                } else {
                    "transport_connection"
                },
                mutation,
            )
        })?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND && path == "/api/provider-operations/protocol" {
            return Err(serde_json::json!({"code":"write_protocol_unavailable","outcomeUnknown":false,"message":"当前后台不支持服务商结果核查，请升级后台后再保存或连接。"}).to_string());
        }
        let credential_protocol_missing = || {
            serde_json::json!({"code":"credential_protocol_unavailable","outcomeUnknown":false,"message":"当前后台不支持凭据结果核查。请正常退出旧版应用；若启用了独立常驻网关，先用原版本停止，再打开当前应用。"}).to_string()
        };
        let scoped_credential = path.starts_with("/api/provider-credentials/protocol")
            || path == "/api/provider-credentials/legacy-policy"
            || path == "/api/provider-credentials/scoped"
            || path.starts_with("/api/provider-credential-operations/scoped/")
            || path.starts_with("/api/provider-credentials/v3")
            || path.starts_with("/api/provider-credential-operations/v3/");
        if status == reqwest::StatusCode::NOT_FOUND
            && scoped_credential
            && !path.starts_with("/api/provider-credential-operations/scoped/")
            && !path.starts_with("/api/provider-credential-operations/v3/")
            && !path.starts_with("/api/provider-credentials/v3/legacy/")
        {
            return Err(credential_protocol_missing());
        }
        let settings_protocol_missing = || {
            serde_json::json!({"code":"settings_protocol_unavailable","outcomeUnknown":false,"message":"当前后台不支持按数据库核查设置，请升级后台并重新核对；本次核查未发送新的保存请求。"}).to_string()
        };
        if (path == "/api/plugins" || path.starts_with("/api/plugin-extensions"))
            && status == reqwest::StatusCode::NOT_FOUND
        {
            return Err("当前后台尚未提供外部插件功能。请先退出旧版 Local Agent；如启用了独立常驻网关，先用原版本正常停止，再打开当前应用。现有功能仍可使用。".into());
        }
        let request_export = path.starts_with("/api/request-exports")
            || path.starts_with("/api/domain/request-exports");
        if request_export && status == reqwest::StatusCode::NOT_FOUND {
            return Err("当前后台不支持分批导出。请正常退出旧版应用；若启用了独立常驻网关，先用原版本停止，再打开当前应用。".into());
        }
        let value: serde_json::Value = if request_export {
            const LIMIT: usize =
                local_agent_core::requests::export::EXPORT_RECORD_BYTES * 4 + 1024 * 1024;
            let bytes = bounded_response_bytes(&mut response, LIMIT)
                .await
                .map_err(|error| match error {
                    ResponseReadError::Capacity => "导出回复超过单批容量，未保存文件".into(),
                    ResponseReadError::Transport => {
                        transport_error("transport_invalid_response", mutation)
                    }
                })?;
            serde_json::from_slice(&bytes)
                .map_err(|_| transport_error("transport_invalid_response", mutation))?
        } else if path == "/api/provider-presets" {
            let bytes = bounded_response_bytes(
                &mut response,
                local_agent_core::domain::provider_presets::MAX_RESPONSE_BYTES,
            )
            .await
            .map_err(|_| transport_error("transport_invalid_response", false))?;
            serde_json::from_slice(&bytes)
                .map_err(|_| transport_error("transport_invalid_response", false))?
        } else if path.starts_with("/api/provider-operations")
            || path == "/api/provider-draft-recovery"
            || scoped_credential
            || path.starts_with("/api/local-tokens/")
        {
            let limit = if path.starts_with("/api/local-tokens/") {
                local_agent_core::providers::local_tokens::MAX_RESPONSE_BYTES
            } else if path.starts_with("/api/provider-credentials/v3/discovery?") {
                local_agent_core::providers::credential_operations::discovery::MAX_RESPONSE_BYTES
            } else if scoped_credential {
                local_agent_core::providers::credential_scope::MAX_RESPONSE_BYTES
            } else {
                local_agent_core::providers::operations::MAX_RESPONSE_BYTES
            };
            let bytes = bounded_response_bytes(&mut response, limit)
                .await
                .map_err(|_| transport_error("transport_invalid_response", mutation))?;
            serde_json::from_slice(&bytes).map_err(|_| {
                if scoped_credential && status == reqwest::StatusCode::NOT_FOUND {
                    credential_protocol_missing()
                } else {
                    transport_error("transport_invalid_response", mutation)
                }
            })?
        } else {
            response.json().await.map_err(|_| {
                if status == reqwest::StatusCode::NOT_FOUND
                    && path.starts_with("/api/provider-credential-operations/")
                {
                    credential_protocol_missing()
                } else if status == reqwest::StatusCode::NOT_FOUND
                    && path.starts_with("/api/settings/scoped/")
                {
                    settings_protocol_missing()
                } else {
                    transport_error("transport_invalid_response", mutation)
                }
            })?
        };
        if status == reqwest::StatusCode::NOT_FOUND
            && path.starts_with("/api/provider-credential-operations/")
            && value.get("code").and_then(|v| v.as_str()) != Some("operation_not_found")
        {
            return Err(credential_protocol_missing());
        }
        if status == reqwest::StatusCode::NOT_FOUND
            && path.starts_with("/api/settings/scoped/")
            && value.get("code").and_then(|v| v.as_str()) != Some("operation_not_found")
        {
            return Err(settings_protocol_missing());
        }
        if !status.is_success() {
            return Err(serde_json::json!({
                "code":value.get("code").and_then(|v| v.as_str()).unwrap_or("unavailable"),
                "outcomeUnknown":value.get("outcomeUnknown").and_then(|v| v.as_bool()).unwrap_or(false),
                "message":value.get("message").and_then(|v| v.as_str()).unwrap_or("操作未完成")
            })
            .to_string());
        }
        Ok(value)
    }

    pub(crate) fn stop(&self, directory: &std::path::Path) {
        let connection = if let Ok(mut state) = self.state() {
            state.stopping = true;
            state.status = Status::Stopping;
            state.record(
                &uuid::Uuid::new_v4().to_string(),
                Phase::Shutdown,
                Category::Stopping,
                None,
                None,
            );
            state.connection.clone()
        } else {
            None
        };
        if connection.as_ref().is_some_and(|c| c.persistent) {
            return;
        }
        let stopping = if let Some(connection) = &connection {
            // Adopted services have no CommandChild. Ask the authenticated owner
            // to drain and exit; never kill a PID discovered on a listening port.
            shutdown(&connection.endpoint, &connection.token).is_ok()
        } else {
            false
        };
        if let Ok(mut child) = self.child.lock()
            && let Some((_, child)) = child.take()
        {
            if !stopping
                || !connection
                    .as_ref()
                    .is_some_and(|c| wait_for_exit(directory, &c.token, Duration::from_secs(18)))
            {
                let _ = child.kill();
            }
        } else if stopping && let Some(connection) = connection {
            let _ = wait_for_exit(directory, &connection.token, Duration::from_secs(18));
        }
        if let Ok(mut state) = self.state() {
            state.status = Status::Stopped;
            state.record(
                &uuid::Uuid::new_v4().to_string(),
                Phase::Shutdown,
                Category::Stopped,
                None,
                None,
            );
        }
    }
}

struct RecoveryReset {
    lifecycle: Arc<Mutex<Lifecycle>>,
    operation: String,
}
impl Drop for RecoveryReset {
    fn drop(&mut self) {
        if let Ok(mut state) = self.lifecycle.lock()
            && state.recovery_id.as_deref() == Some(&self.operation)
            && state.status == Status::Reconnecting
        {
            state.status = Status::Disconnected;
            state.record(
                &self.operation,
                Phase::Recovery,
                Category::RecoveryCancelled,
                None,
                None,
            );
        }
    }
}

fn local_error(code: &str, message: &str) -> String {
    serde_json::json!({"code":code,"message":message,"outcomeUnknown":false}).to_string()
}

#[derive(Clone, Copy)]
pub(crate) enum ExportKind {
    Legacy,
    Unified,
}
impl ExportKind {
    pub(crate) fn path(self) -> &'static str {
        match self {
            Self::Legacy => "/api/request-exports",
            Self::Unified => "/api/domain/request-exports",
        }
    }
}
pub(crate) struct ExportCleanup {
    client: reqwest::Client,
    connection: Option<Connection>,
    operation: Option<String>,
    kind: ExportKind,
}
impl ExportCleanup {
    pub(crate) fn completed(&mut self) {
        self.operation = None;
    }
}
impl Drop for ExportCleanup {
    fn drop(&mut self) {
        let Some(operation) = self.operation.take() else {
            return;
        };
        let Some(connection) = self.connection.take() else {
            return;
        };
        let client = self.client.clone();
        let path = self.kind.path();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = client
                    .delete(format!("{}{path}/{operation}", connection.endpoint))
                    .bearer_auth(connection.token)
                    .timeout(Duration::from_secs(8))
                    .send()
                    .await;
            });
        }
    }
}

enum ResponseReadError {
    Transport,
    Capacity,
}

async fn bounded_response_bytes(
    response: &mut reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, ResponseReadError> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ResponseReadError::Transport)?
    {
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return Err(ResponseReadError::Capacity);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn read_only_request(method: &reqwest::Method, path: &str) -> bool {
    matches!(method, &reqwest::Method::GET | &reqwest::Method::HEAD)
        || path == "/api/provider-presets"
        // These POST routes accept query DTOs, without business writes. Match
        // exact routes so consent, refresh, exports and future actions stay writes.
        || (method == reqwest::Method::POST
            && matches!(
                path,
                "/api/domain/requests"
                    | "/api/domain/activity"
                    | "/api/domain/facts"
                    | "/api/domain/sessions"
                    | "/api/domain/conversation"
                    | "/api/domain/fact-conversation"
            ))
}

fn transport_error(code: &str, mutation: bool) -> String {
    serde_json::json!({
        "code":code,
        "outcomeUnknown":mutation,
        "message":if mutation {
            "未能取得操作结果，后台可能已经提交；请核查结果后再重试"
        } else {
            "本地数据读取未完成，请重试或重新打开应用"
        },
    })
    .to_string()
}

fn shutdown(endpoint: &str, token: &str) -> std::io::Result<()> {
    use std::{
        io::{Read, Write},
        net::{SocketAddr, TcpStream},
    };
    // Exit callbacks must not block_on Tauri's shared async runtime: an IPC
    // completion may already be waiting for this same native main thread.
    let address: SocketAddr = endpoint
        .strip_prefix("http://")
        .unwrap_or("")
        .parse()
        .map_err(std::io::Error::other)?;
    if !address.ip().is_loopback() || !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(std::io::Error::other("Invalid local service connection"));
    }
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(300))?;
    stream.set_write_timeout(Some(Duration::from_millis(500)))?;
    stream.set_read_timeout(Some(Duration::from_millis(500)))?;
    write!(
        stream,
        "POST /api/runtime/shutdown HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )?;
    let mut response = Vec::new();
    (&mut stream).take(1024).read_to_end(&mut response)?;
    if !response.starts_with(b"HTTP/1.1 200 ") {
        return Err(std::io::Error::other("Local service rejected shutdown"));
    }
    Ok(())
}

fn wait_for_exit(directory: &std::path::Path, token: &str, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        // The listener closes before the final flush. Only the owned runtime
        // publication disappearing (or being replaced) acknowledges process exit.
        match runtime::read_service(directory) {
            Ok(None) => return true,
            Ok(Some(record)) if record.token != token => return true,
            _ => {}
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

pub(crate) fn query_path(path: &str, query: impl serde::Serialize) -> Result<String, String> {
    let mut url =
        reqwest::Url::parse(&format!("http://localhost{path}")).map_err(|_| "查询路径无效")?;
    let value = serde_json::to_value(query).map_err(|_| "查询格式无效")?;
    if let Some(fields) = value.as_object() {
        let mut pairs = url.query_pairs_mut();
        for (key, value) in fields {
            if value.is_null() {
                continue;
            }
            pairs.append_pair(
                key,
                &value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string()),
            );
        }
    }
    Ok(format!("{}?{}", url.path(), url.query().unwrap_or("")))
}

pub(crate) fn start(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()?;
    let backend = Backend::new(client, None, None);
    let operation = uuid::Uuid::new_v4().to_string();
    backend
        .state()?
        .record(&operation, Phase::Startup, Category::Starting, None, None);
    if let Err(error) = connect_or_start(app, &backend, &operation) {
        let mut state = backend.state()?;
        state.status = Status::StartupFailed;
        state.startup_error = Some(error);
        state.record(
            &operation,
            Phase::Startup,
            Category::StartupFailed,
            None,
            None,
        );
    }
    // A failed sidecar becomes a visible, recoverable UI error, not a panic in
    // macOS didFinishLaunching that aborts the entire native process.
    app.manage(backend);
    Ok(())
}

async fn existing(
    directory: &std::path::Path,
    client: &reqwest::Client,
) -> Result<Option<ServiceRecord>, String> {
    existing_with_capacity(directory, client, None).await
}

async fn read_record(
    directory: &std::path::Path,
    capacity: Option<Arc<tokio::sync::OwnedSemaphorePermit>>,
) -> Result<Option<ServiceRecord>, String> {
    let directory = directory.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let _capacity = capacity;
        runtime::read_service(&directory)
    })
    .await
    .map_err(|_| "本地服务身份读取中断".to_string())?
}
async fn existing_with_capacity(
    directory: &std::path::Path,
    client: &reqwest::Client,
    capacity: Option<Arc<tokio::sync::OwnedSemaphorePermit>>,
) -> Result<Option<ServiceRecord>, String> {
    let Some(record) = read_record(directory, capacity.clone()).await? else {
        return Ok(None);
    };
    let mut response = match client
        .get(format!("http://127.0.0.1:{}/api/runtime", record.port))
        .bearer_auth(&record.token)
        .timeout(Duration::from_millis(800))
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => return Ok(None), // Stale publication after an abnormal exit.
    };
    if !response.status().is_success() {
        return Err("无法验证已有本地服务身份，未接入占用端口的程序".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "已有本地服务身份响应中断")?
    {
        if chunk.len() > 4096usize.saturating_sub(bytes.len()) {
            return Err("已有本地服务身份响应超出限制".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let identity: runtime::Identity =
        serde_json::from_slice(&bytes).map_err(|_| "已有本地服务身份响应无效")?;
    if identity != record.identity {
        return Err("已有本地服务身份不匹配，未接入占用端口的程序".into());
    }
    if identity.protocol != runtime::PROTOCOL || identity.version != env!("CARGO_PKG_VERSION") {
        return Err("已有本地服务与当前版本不兼容，请先正常退出旧版应用；若启用了常驻网关，先用原版本停止，再打开当前应用".into());
    }
    let after = read_record(directory, capacity).await?;
    if !after.is_some_and(|after| {
        after.identity == record.identity
            && after.port == record.port
            && after.token == record.token
    }) {
        return Err("本地服务发布已变化，请再次核对连接".into());
    }
    Ok(Some(record))
}

fn connect_or_start(
    app: &mut tauri::App,
    backend: &Backend,
    operation: &str,
) -> Result<(), String> {
    let client = &backend.client;
    let directory = runtime::desktop_data_dir(&app.config().identifier)?;
    if let Some(record) = tauri::async_runtime::block_on(existing(&directory, client))? {
        let mut state = backend.state()?;
        state.install(record.into())?;
        state.record(operation, Phase::Startup, Category::Verified, None, None);
        return Ok(());
    }
    if directory.join("gateway-lan/config.json").exists() {
        return Err("此数据目录的常驻网关未运行，请先启动常驻服务".into());
    }
    let token = uuid::Uuid::new_v4().to_string();
    let (mut events, child) = app
        .shell()
        .sidecar("local-agent-service")
        .map_err(|_| "应用包缺少本地服务，请重新安装完整应用")?
        .args(["--data-dir", &directory.to_string_lossy()])
        .env("LOCAL_AGENT_SERVICE_TOKEN", &token)
        .spawn()
        .map_err(|_| "无法启动应用包内的本地服务")?;
    let ready = tauri::async_runtime::block_on(async {
        tokio::time::timeout(
            Duration::from_secs(10),
            wait_for_startup(&directory, backend, operation, &mut events),
        )
        .await
        .map_err(|_| "本地服务启动超时，请检查已有服务后重新打开".to_string())?
    });
    let record = match ready {
        Ok(record) => record,
        Err(error) => {
            let _ = child.kill();
            tauri::async_runtime::spawn(observe_process(
                events,
                backend.lifecycle.clone(),
                backend.child.clone(),
                String::new(),
                operation.to_owned(),
                false,
            ));
            return Err(error);
        }
    };
    let generation = record.identity.instance_id.clone();
    let owned = record.token == token;
    if owned {
        *backend
            .child
            .lock()
            .map_err(|_| "无法记录后台子进程所有权")? = Some((generation.clone(), child));
    } else {
        let _ = child.kill();
    }
    {
        let mut state = backend.state()?;
        state.install(record.into())?;
        state.record(operation, Phase::Startup, Category::Verified, None, None);
    }
    let (lifecycle, child, operation) = (
        backend.lifecycle.clone(),
        backend.child.clone(),
        operation.to_owned(),
    );
    tauri::async_runtime::spawn(observe_process(
        events, lifecycle, child, generation, operation, owned,
    ));
    Ok(())
}

async fn wait_for_startup(
    directory: &std::path::Path,
    backend: &Backend,
    operation: &str,
    events: &mut tauri::async_runtime::Receiver<CommandEvent>,
) -> Result<ServiceRecord, String> {
    let mut hint = None;
    loop {
        if let Some(record) = existing(directory, &backend.client).await? {
            return Ok(record);
        }
        let exit = match tokio::time::timeout(Duration::from_millis(100), events.recv()).await {
            Ok(Some(CommandEvent::Stderr(bytes))) => {
                let message = String::from_utf8_lossy(&bytes);
                // Map only known failures. Never retain or show raw child output.
                if message.contains("固定端口被占用") {
                    hint =
                        Some("本地网关固定端口被其他程序占用，请退出旧开发包或检查端口占用后重试");
                } else if message.contains("钥匙串助手初始化失败") {
                    hint =
                        Some("本地服务无法初始化钥匙串助手，请核对应用包完整性和本机数据目录权限");
                }
                continue;
            }
            Ok(Some(CommandEvent::Terminated(payload))) => {
                backend.state()?.record(
                    operation,
                    Phase::Startup,
                    Category::ChildExited,
                    payload.code,
                    payload.signal,
                );
                match (payload.code, payload.signal) {
                    (Some(code), _) => {
                        format!("本地服务在完成启动前退出（退出码 {code}），请查看最近诊断记录")
                    }
                    (_, Some(signal)) => {
                        format!("本地服务在完成启动前被终止（信号 {signal}），请查看最近诊断记录")
                    }
                    _ => "本地服务在完成启动前退出，请查看最近诊断记录".into(),
                }
            }
            Ok(Some(CommandEvent::Error(_))) => {
                backend.state()?.record(
                    operation,
                    Phase::Startup,
                    Category::ChildError,
                    None,
                    None,
                );
                continue;
            }
            Ok(None) => "本地服务的启动事件已中断，请查看最近诊断记录".into(),
            _ => continue,
        };
        // The publication may have appeared between the initial read and the
        // child exit, for example if a separately launched service took ownership.
        return existing(directory, &backend.client)
            .await?
            .ok_or_else(|| hint.map(str::to_owned).unwrap_or(exit));
    }
}

async fn observe_process(
    mut events: tauri::async_runtime::Receiver<CommandEvent>,
    lifecycle: Arc<Mutex<Lifecycle>>,
    child: Arc<Mutex<Option<(String, CommandChild)>>>,
    instance_id: String,
    operation: String,
    owned: bool,
) {
    let mut terminated = false;
    let phase = if owned {
        Phase::Process
    } else {
        Phase::Startup
    };
    while let Some(event) = events.recv().await {
        match event {
            CommandEvent::Terminated(payload) => {
                terminated = true;
                if let Ok(mut state) = lifecycle.lock() {
                    if owned {
                        state.process_exit(&instance_id, &operation, payload.code, payload.signal);
                    } else {
                        state.record(
                            &operation,
                            phase,
                            Category::ChildExited,
                            payload.code,
                            payload.signal,
                        );
                    }
                }
                if owned
                    && let Ok(mut child) = child.lock()
                    && child.as_ref().is_some_and(|(id, _)| id == &instance_id)
                {
                    child.take();
                }
            }
            CommandEvent::Error(_) => {
                if let Ok(mut state) = lifecycle.lock() {
                    state.record(&operation, phase, Category::ChildError, None, None);
                }
            }
            _ => {} // Never retain stdout, stderr, or event error payloads.
        }
    }
    if !terminated && let Ok(mut state) = lifecycle.lock() {
        state.record(&operation, phase, Category::EventStreamClosed, None, None);
        if owned
            && state
                .connection
                .as_ref()
                .is_some_and(|c| c.instance_id == instance_id)
            && !state.stopping
        {
            state.status = Status::Disconnected;
        }
    }
}

#[cfg(test)]
mod lifecycle_tests;

#[cfg(test)]
mod tests {
    use super::{
        Backend, CommandEvent, existing, query_path, shutdown, wait_for_exit, wait_for_startup,
    };
    use local_agent_core::runtime::{self, ServiceGuard};
    use std::{
        io::{Read, Write},
        net::TcpListener,
        path::PathBuf,
    };

    #[test]
    fn unreadable_transport_replies_distinguish_read_failure_from_unknown_writes() {
        for (method, unknown) in [(reqwest::Method::GET, false), (reqwest::Method::PUT, true)] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                    .unwrap();
                let mut request = [0; 4096];
                assert!(stream.read(&mut request).unwrap() > 0);
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nnope",
                    )
                    .unwrap();
            });
            let backend = super::Backend::test_with_client(
                reqwest::Client::builder()
                    .no_proxy()
                    .timeout(std::time::Duration::from_secs(2))
                    .build()
                    .unwrap(),
                format!("http://{address}"),
                "fixture-token".into(),
                false,
            );
            let result =
                tauri::async_runtime::block_on(backend.request(method, "/api/settings", None))
                    .unwrap_err();
            let error: serde_json::Value = serde_json::from_str(&result).unwrap();
            assert_eq!(error["code"], "transport_invalid_response");
            assert_eq!(error["outcomeUnknown"], unknown);
            server.join().unwrap();
        }
    }

    struct Temp(PathBuf);
    #[test]
    fn domain_uncertainty_is_preserved_across_http_error_responses() {
        let pending = r#"{"code":"credential_result_unknown","outcomeUnknown":true,"message":"fixture pending"}"#;
        for path in [
            "/api/provider-credential-operations/fixture-id",
            "/api/settings/scoped/operations/fixture-id?storeId=fixture",
        ] {
            for (method, status, body, code, unknown) in [
                (
                    reqwest::Method::GET,
                    "503 Service Unavailable",
                    pending,
                    "credential_result_unknown",
                    true,
                ),
                (
                    reqwest::Method::POST,
                    "503 Service Unavailable",
                    pending,
                    "credential_result_unknown",
                    true,
                ),
                (
                    reqwest::Method::GET,
                    "404 Not Found",
                    "",
                    "credential_protocol_unavailable",
                    false,
                ),
                (
                    reqwest::Method::GET,
                    "404 Not Found",
                    r#"{"message":"old router"}"#,
                    "credential_protocol_unavailable",
                    false,
                ),
                (
                    reqwest::Method::GET,
                    "404 Not Found",
                    r#"{"code":"operation_not_found","message":"fixture missing"}"#,
                    "operation_not_found",
                    false,
                ),
            ] {
                let listener = TcpListener::bind("127.0.0.1:0").unwrap();
                let address = listener.local_addr().unwrap();
                let server = std::thread::spawn(move || {
                    let (mut stream, _) = listener.accept().unwrap();
                    stream
                        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                        .unwrap();
                    let mut request = [0; 4096];
                    assert!(stream.read(&mut request).unwrap() > 0);
                    write!(
                        stream,
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    )
                    .unwrap();
                });
                let backend = super::Backend::test_with_client(
                    reqwest::Client::builder()
                        .no_proxy()
                        .timeout(std::time::Duration::from_secs(2))
                        .build()
                        .unwrap(),
                    format!("http://{address}"),
                    "fixture-token".into(),
                    false,
                );
                let error = tauri::async_runtime::block_on(backend.request(method, path, None))
                    .unwrap_err();
                let value: serde_json::Value = serde_json::from_str(&error).unwrap();
                let expected_code = if path.starts_with("/api/settings/")
                    && code == "credential_protocol_unavailable"
                {
                    "settings_protocol_unavailable"
                } else {
                    code
                };
                assert_eq!(value["code"], expected_code);
                assert_eq!(value["outcomeUnknown"], unknown);
                server.join().unwrap();
            }
        }
    }
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .join(format!("local-agent-reconnect-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            Self(path.canonicalize().unwrap())
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn startup_reports_early_exit_with_safe_diagnostics_instead_of_timing_out() {
        let directory = Temp::new();
        let backend = Backend::new(reqwest::Client::new(), None, None);
        tauri::async_runtime::block_on(async {
            for (stderr, expected) in [
                ("private-sentinel", "退出码 7"),
                (
                    "钥匙串助手初始化失败：private-sentinel",
                    "无法初始化钥匙串助手",
                ),
            ] {
                let (tx, mut rx) = tokio::sync::mpsc::channel(2);
                tx.send(CommandEvent::Stderr(stderr.as_bytes().to_vec()))
                    .await
                    .unwrap();
                tx.send(CommandEvent::Terminated(
                    tauri_plugin_shell::process::TerminatedPayload {
                        code: Some(7),
                        signal: None,
                    },
                ))
                .await
                .unwrap();
                let error = tokio::time::timeout(
                    std::time::Duration::from_secs(1),
                    wait_for_startup(&directory.0, &backend, "fixture", &mut rx),
                )
                .await
                .expect("a terminated child must not wait for startup timeout")
                .err()
                .expect("there is no published service");
                assert!(error.contains(expected), "{error}");
                assert!(!error.contains("private-sentinel"));
            }
        });
        let report = serde_json::to_value(backend.diagnostics().unwrap()).unwrap();
        assert!(
            report["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|event| { event["category"] == "child_exited" && event["exitCode"] == 7 })
        );
        assert!(!report.to_string().contains("private-sentinel"));
    }

    #[test]
    fn desktop_exit_leaves_a_persistent_service_untouched() {
        let temp = Temp::new();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let guard = ServiceGuard::acquire(&temp.0).unwrap();
        let token = uuid::Uuid::new_v4().to_string();
        guard
            .publish_mode(
                listener.local_addr().unwrap().port(),
                token.clone(),
                "test",
                true,
            )
            .unwrap();
        let backend = super::Backend::test_with_client(
            reqwest::Client::new(),
            format!("http://{}", listener.local_addr().unwrap()),
            token,
            true,
        );
        backend.stop(&temp.0);
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        assert!(
            runtime::read_service(&temp.0)
                .unwrap()
                .unwrap()
                .identity
                .persistent
        );
    }

    #[test]
    fn shutdown_waits_for_the_publication_after_the_listener_is_closed() {
        let temp = Temp::new();
        let guard = ServiceGuard::acquire(&temp.0).unwrap();
        let token = uuid::Uuid::new_v4().to_string();
        guard.publish(12345, token.clone(), "test").unwrap();
        assert!(!wait_for_exit(
            &temp.0,
            &token,
            std::time::Duration::from_millis(10)
        ));
        drop(guard);
        assert!(wait_for_exit(
            &temp.0,
            &token,
            std::time::Duration::from_millis(100)
        ));
    }
    #[test]
    fn shutdown_uses_bounded_local_io_without_the_ui_async_runtime() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let token = uuid::Uuid::new_v4().to_string();
        let expected = token.clone();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(1)))
                .unwrap();
            let mut bytes = [0; 4096];
            let mut length = 0;
            // TCP may split the formatted request across several writes / reads.
            while length < bytes.len() && !bytes[..length].ends_with(b"\r\n\r\n") {
                let count = stream.read(&mut bytes[length..]).unwrap();
                assert!(count > 0, "request ended before its headers");
                length += count;
            }
            assert!(bytes[..length].ends_with(b"\r\n\r\n"));
            let request = String::from_utf8_lossy(&bytes[..length]);
            assert!(request.starts_with("POST /api/runtime/shutdown HTTP/1.1\r\n"));
            assert!(request.contains(&format!("Host: {address}\r\n")));
            assert!(request.contains(&format!("Authorization: Bearer {expected}\r\n")));
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
                .unwrap();
        });
        shutdown(&format!("http://{address}"), &token).unwrap();
        server.join().unwrap();
        assert!(shutdown("http://192.0.2.1:8080", &token).is_err());
        assert!(shutdown(&format!("http://{address}"), "bad\r\nheader").is_err());
    }

    fn probe(
        status: &str,
        version: &str,
        alter: impl FnOnce(&mut runtime::Identity),
    ) -> Result<bool, String> {
        let temp = Temp::new();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let guard = ServiceGuard::acquire(&temp.0).unwrap();
        let token = uuid::Uuid::new_v4().to_string();
        let mut identity = guard
            .publish(
                listener.local_addr().unwrap().port(),
                token.clone(),
                version,
            )
            .unwrap();
        alter(&mut identity);
        let body = serde_json::to_string(&identity).unwrap();
        let reply = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                .unwrap();
            let mut bytes = [0; 4096];
            let length = stream.read(&mut bytes).unwrap();
            let request = String::from_utf8_lossy(&bytes[..length]).to_ascii_lowercase();
            assert!(request.contains(&format!("authorization: bearer {token}")));
            stream.write_all(reply.as_bytes()).unwrap();
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let result = tauri::async_runtime::block_on(existing(&temp.0, &client))
            .map(|record| record.is_some());
        server.join().unwrap();
        result
    }

    #[test]
    fn reconnects_only_to_the_published_authenticated_identity() {
        assert!(probe("200 OK", env!("CARGO_PKG_VERSION"), |_| {}).unwrap());
        assert!(
            probe("401 Unauthorized", env!("CARGO_PKG_VERSION"), |_| {})
                .unwrap_err()
                .contains("身份")
        );
        assert!(
            probe("200 OK", env!("CARGO_PKG_VERSION"), |identity| identity
                .instance_id =
                uuid::Uuid::new_v4().to_string())
            .unwrap_err()
            .contains("不匹配")
        );
        assert!(
            probe("302 Found", env!("CARGO_PKG_VERSION"), |_| {})
                .unwrap_err()
                .contains("身份")
        );
    }

    #[test]
    fn incompatible_live_service_is_rejected_after_identity_verification() {
        for version in ["0.1.0", "0.1.1", "0.1.2"] {
            assert!(
                probe("200 OK", version, |_| {})
                    .unwrap_err()
                    .contains("不兼容"),
                "the audit release must not adopt an old service without the reviewed fixes"
            );
        }
        // An untrusted reply cannot change the protocol advertised on disk.
        assert!(
            probe("200 OK", env!("CARGO_PKG_VERSION"), |identity| identity
                .protocol +=
                1)
            .unwrap_err()
            .contains("不匹配")
        );
        assert!(
            probe("200 OK", "old-version", |_| {})
                .unwrap_err()
                .contains("不兼容")
        );
        let temp = Temp::new();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let guard = ServiceGuard::acquire(&temp.0).unwrap();
        guard
            .publish(
                listener.local_addr().unwrap().port(),
                uuid::Uuid::new_v4().to_string(),
                "old-version",
            )
            .unwrap();
        drop(listener);
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        assert!(
            tauri::async_runtime::block_on(existing(&temp.0, &client))
                .unwrap()
                .is_none(),
            "a stale incompatible publication must not block a fresh service"
        );
    }

    #[test]
    fn adopted_legacy_service_reports_plugin_unavailability_without_breaking_old_requests() {
        use std::io::{Read, Write};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            for (status, body) in [("404 Not Found", ""), ("200 OK", r#"{"stillUsable":true}"#)] {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                    .unwrap();
                let mut bytes = [0u8; 2048];
                let _ = stream.read(&mut bytes).unwrap();
                write!(stream,"HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
            }
        });
        let backend = super::Backend::test_with_client(
            reqwest::Client::builder().no_proxy().build().unwrap(),
            endpoint,
            "offline-fixture".into(),
            true,
        );
        tauri::async_runtime::block_on(async {
            let error = backend
                .request(reqwest::Method::GET, "/api/plugins", None)
                .await
                .unwrap_err();
            assert!(error.contains("当前后台尚未提供外部插件"));
            assert_eq!(
                backend
                    .request(reqwest::Method::GET, "/api/snapshot", None)
                    .await
                    .unwrap()["stillUsable"],
                true
            );
        });
        server.join().unwrap();
    }

    #[test]
    fn query_preserves_optional_null_and_reserved_character_semantics() {
        let path = query_path(
            "/api/sessions",
            serde_json::json!({
                "agent":"claude", "query":"项目 & model=a/b?#", "cursor":null, "days":7,
            }),
        )
        .unwrap();
        let url = reqwest::Url::parse(&format!("http://localhost{path}")).unwrap();
        let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(url.path(), "/api/sessions");
        assert_eq!(
            query.get("query").map(String::as_str),
            Some("项目 & model=a/b?#")
        );
        assert_eq!(query.get("days").map(String::as_str), Some("7"));
        assert!(!query.contains_key("cursor"));
    }
}
