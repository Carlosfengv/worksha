use super::{
    ExportCleanup, ExportKind,
    lifecycle::{Category, Connection, Lifecycle, Phase, Status},
    local_error,
};
use std::sync::{Arc, Mutex};
#[cfg(test)]
mod tests;

/// One authenticated service connection for a compound native operation.
/// The captured token never crosses the WebView boundary.
#[derive(Clone)]
pub(crate) struct ConnectionContext {
    client: reqwest::Client,
    lifecycle: Arc<Mutex<Lifecycle>>,
    connection: Connection,
}
impl ConnectionContext {
    pub(super) fn new(
        client: reqwest::Client,
        lifecycle: Arc<Mutex<Lifecycle>>,
        connection: Connection,
    ) -> Self {
        Self {
            client,
            lifecycle,
            connection,
        }
    }
    fn state(&self) -> Result<std::sync::MutexGuard<'_, Lifecycle>, String> {
        self.lifecycle
            .lock()
            .map_err(|_| "后台连接状态暂不可读，请重新打开应用".into())
    }
    pub(crate) fn ensure_current(&self) -> Result<(), String> {
        let state = self.state()?;
        if state
            .connection
            .as_ref()
            .is_some_and(|current| self.matches_access(current))
            && state.status == Status::Connected
            && !state.stopping
        {
            Ok(())
        } else {
            Err(local_error(
                "backend_connection_changed",
                "连接已变化，请核对原后台状态后再试",
            ))
        }
    }
    pub(super) fn matches_access(&self, current: &Connection) -> bool {
        current.generation == self.connection.generation
            && current.iam_epoch == self.connection.iam_epoch
    }
    pub(crate) fn instance_id(&self) -> &str {
        &self.connection.instance_id
    }
    /// Lifetime was decoded and verified when this exact connection was installed.
    pub(crate) fn persistent(&self) -> bool {
        self.connection.persistent
    }
    pub(crate) fn export_cleanup(&self, operation: String) -> ExportCleanup {
        self.request_export_cleanup(operation, ExportKind::Legacy)
    }
    pub(crate) fn request_export_cleanup(
        &self,
        operation: String,
        kind: ExportKind,
    ) -> ExportCleanup {
        ExportCleanup {
            client: self.client.clone(),
            connection: Some(self.connection.clone()),
            operation: Some(operation),
            kind,
        }
    }
    pub(crate) async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        self.ensure_current()?;
        self.run(method, path, body, true).await
    }
    /// Only query or compensate work already started on this exact origin.
    /// Changing the application's active connection must not redirect cleanup.
    pub(crate) async fn origin_request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        self.run(method, path, body, false).await
    }
    async fn same_instance_responds(&self) -> bool {
        let budget = std::time::Duration::from_millis(800);
        let probe = async {
            let mut response = self
                .client
                .get(format!("{}/api/runtime", self.connection.endpoint))
                .bearer_auth(&self.connection.token)
                .timeout(budget)
                .send()
                .await
                .ok()?;
            if !response.status().is_success() {
                return None;
            }
            let bytes = super::bounded_response_bytes(&mut response, 4096)
                .await
                .ok()?;
            serde_json::from_slice::<local_agent_core::runtime::Identity>(&bytes).ok()
        };
        matches!(
            tokio::time::timeout(budget, probe).await,
            Ok(Some(identity))
                if identity.protocol == local_agent_core::runtime::PROTOCOL
                    && identity.version == env!("CARGO_PKG_VERSION")
                    && identity.instance_id == self.connection.instance_id
                    && identity.persistent == self.connection.persistent
                    && identity.pid > 0
        )
    }
    async fn run(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
        current_read: bool,
    ) -> Result<serde_json::Value, String> {
        let mutation = !super::read_only_request(&method, path);
        let result = super::Backend::send(&self.client, &self.connection, method, path, body).await;
        if let Err(error) = &result {
            let code = serde_json::from_str::<serde_json::Value>(error)
                .ok()
                .and_then(|v| v["code"].as_str().map(str::to_owned));
            let category = match code.as_deref() {
                Some("transport_timeout") => Category::TransportTimeout,
                Some("transport_connection") => Category::TransportConnection,
                Some("transport_invalid_response") => Category::InvalidResponse,
                _ => Category::RequestRejected,
            };
            // A slow read is not evidence that the whole service stopped. Keep
            // only this already connected origin when its authenticated identity
            // still matches. Never install a connection or replay the request.
            let live_read_timeout = current_read
                && !mutation
                && code.as_deref() == Some("transport_timeout")
                && self.ensure_current().is_ok()
                && self.same_instance_responds().await;
            if let Ok(mut state) = self.state() {
                state.record(
                    &uuid::Uuid::new_v4().to_string(),
                    Phase::Request,
                    category,
                    None,
                    None,
                );
                if state
                    .connection
                    .as_ref()
                    .is_some_and(|current| self.matches_access(current))
                    && !state.stopping
                    && !live_read_timeout
                    && matches!(
                        category,
                        Category::TransportTimeout
                            | Category::TransportConnection
                            | Category::InvalidResponse
                    )
                {
                    state.status = Status::Disconnected;
                }
            }
        } else if current_read && !mutation {
            self.ensure_current()?;
        }
        // A completed write belongs to its captured origin even after reconnect.
        result
    }
}
