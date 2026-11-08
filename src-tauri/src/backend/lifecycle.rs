use serde::Serialize;
use std::{
    collections::VecDeque,
    time::{SystemTime, UNIX_EPOCH},
};

pub(super) const EVENT_LIMIT: usize = 32;

#[derive(Clone)]
pub(super) struct Connection {
    pub endpoint: String,
    pub token: String,
    pub persistent: bool,
    pub generation: String,
    pub instance_id: String,
    pub iam_protocol: bool,
    pub iam_epoch: u64,
    pub iam_session: Option<std::sync::Arc<zeroize::Zeroizing<String>>>,
}
impl Connection {
    pub(super) fn verified(
        record: local_agent_core::runtime::ServiceRecord,
        iam_protocol: bool,
    ) -> Self {
        Self {
            endpoint: format!("http://127.0.0.1:{}", record.port),
            token: record.token,
            persistent: record.identity.persistent,
            generation: uuid::Uuid::new_v4().to_string(),
            instance_id: record.identity.instance_id,
            iam_protocol,
            iam_epoch: 0,
            iam_session: None,
        }
    }
}
#[derive(Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Status {
    Starting,
    Connected,
    Disconnected,
    StartupFailed,
    Reconnecting,
    Stopping,
    Stopped,
}
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    Startup,
    Request,
    Recovery,
    Process,
    Shutdown,
}
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Category {
    Starting,
    Verified,
    StartupFailed,
    TransportTimeout,
    TransportConnection,
    InvalidResponse,
    RequestRejected,
    ChildError,
    ChildExited,
    EventStreamClosed,
    ReconnectStarted,
    ReconnectFailed,
    Reconnected,
    RecoveryCancelled,
    Stopping,
    Stopped,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Event {
    id: String,
    operation_id: String,
    at: u64,
    phase: Phase,
    category: Category,
    exit_code: Option<i32>,
    signal: Option<i32>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Diagnostics {
    pub state: Status,
    verified_at: Option<u64>,
    pub can_reconnect: bool,
    events: Vec<Event>,
}
pub(super) struct Lifecycle {
    pub connection: Option<Connection>,
    pub startup_error: Option<String>,
    pub status: Status,
    pub stopping: bool,
    pub recovery_id: Option<String>,
    pub terminal_instance: Option<String>,
    verified_at: Option<u64>,
    events: VecDeque<Event>,
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|v| v.as_millis() as u64)
        .unwrap_or(0)
}
impl Lifecycle {
    pub fn new(connection: Option<Connection>, startup_error: Option<String>) -> Self {
        let connected = connection.is_some();
        Self {
            connection,
            status: if startup_error.is_some() {
                Status::StartupFailed
            } else if connected {
                Status::Connected
            } else {
                Status::Starting
            },
            startup_error,
            stopping: false,
            recovery_id: None,
            terminal_instance: None,
            verified_at: connected.then(now),
            events: VecDeque::new(),
        }
    }
    pub fn snapshot(&self) -> Diagnostics {
        Diagnostics {
            state: self.status,
            verified_at: self.verified_at,
            can_reconnect: !self.stopping
                && matches!(
                    self.status,
                    Status::Connected | Status::Disconnected | Status::StartupFailed
                ),
            events: self.events.iter().rev().cloned().collect(),
        }
    }
    pub fn record(
        &mut self,
        operation: &str,
        phase: Phase,
        category: Category,
        exit: Option<i32>,
        signal: Option<i32>,
    ) {
        if self.events.len() == EVENT_LIMIT {
            self.events.pop_front();
        }
        self.events.push_back(Event {
            id: uuid::Uuid::new_v4().to_string(),
            operation_id: operation.into(),
            at: now(),
            phase,
            category,
            exit_code: exit,
            signal,
        });
    }
    pub fn install(&mut self, connection: Connection) -> Result<(), String> {
        if self.stopping {
            return Err("应用正在退出，未更换后台连接".into());
        }
        if self.terminal_instance.as_deref() == Some(&connection.instance_id) {
            return Err("该后台已经退出，未重新接入旧实例".into());
        }
        self.connection = Some(connection);
        self.startup_error = None;
        self.status = Status::Connected;
        self.verified_at = Some(now());
        Ok(())
    }
    pub fn process_exit(
        &mut self,
        generation: &str,
        operation: &str,
        code: Option<i32>,
        signal: Option<i32>,
    ) {
        let current_instance = self
            .connection
            .as_ref()
            .is_some_and(|c| c.instance_id == generation);
        if current_instance || self.terminal_instance.is_none() {
            self.terminal_instance = Some(generation.into());
        }
        self.record(
            operation,
            Phase::Process,
            Category::ChildExited,
            code,
            signal,
        );
        if current_instance && !self.stopping {
            self.status = Status::Disconnected;
        }
    }
}
