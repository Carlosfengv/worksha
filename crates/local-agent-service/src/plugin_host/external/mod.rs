//! External packages are an additive host. They never replace built-in domain services.
pub(crate) mod advanced;
pub(crate) mod api;
mod broker;
pub(crate) mod catalog;
mod contributions;
#[cfg(test)]
mod database_tests;
mod diagnostics;
pub(crate) mod domain;
mod domain_store;
mod fact_query;
pub(crate) mod full;
mod installation;
mod object_contributions;
mod package;
pub(crate) mod policy;
pub(crate) mod presets;
pub(crate) mod sdk;
mod store;
#[cfg(test)]
mod tests;
pub(crate) mod worker;

use local_agent_core::plugin::external::*;
use local_agent_core::providers::vault::Vault;
use rusqlite::Connection;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

// A damaged optional catalog must not take the original service offline. Keep
// the failure explicit instead of substituting a new/empty database.
enum Database {
    Ready(Mutex<Connection>),
    Unavailable(Error),
}
impl Database {
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        match self {
            Self::Ready(db) => db.lock().checked("unavailable"),
            Self::Unavailable(error) => Err(error.clone()),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Error {
    pub code: &'static str,
}
pub(crate) type Result<T> = std::result::Result<T, Error>;
pub(crate) fn fail(code: &'static str) -> Error {
    Error { code }
}
pub(crate) async fn run_sync<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    crate::blocking::run(work)
        .await
        .map_err(|error| match error {
            crate::blocking::Error::Busy => fail("busy"),
            crate::blocking::Error::Unavailable => fail("unavailable"),
        })?
}
pub(crate) trait Checked<T> {
    fn checked(self, code: &'static str) -> Result<T>;
}
impl<T, E> Checked<T> for std::result::Result<T, E> {
    fn checked(self, code: &'static str) -> Result<T> {
        self.map_err(|_| fail(code))
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.code)
    }
}
impl std::error::Error for Error {}
pub(crate) fn message(code: &str) -> &'static str {
    match code {
        "bridge_unavailable" => "Agent 连接桥接不可用，请确认已启动并启用模型连接桥接。",
        "connection_unverified" => "请先在 Providers 或模型服务页面验证目标模型。",
        "connection_result_unknown" => "写入结果待核查。请先查看 Agent 的连接状态。",
        "credential_unavailable" => "本机连接凭据暂不可用。",
        "permission_denied" => "请先授权此插件所需的具体来源或连接。",
        "grant_revoked" => "授权或插件版本已变化，请重新读取。",
        "plugin_disabled" => "插件当前未启用。",
        "incompatible_api" => "插件要求的接口版本不兼容。",
        "dependency_missing" => "请先启用插件所依赖的扩展。",
        "dependency_cycle" => "插件依赖存在循环，未执行插件。",
        "publisher_untrusted" => "请核对发布者公钥后明确同意安装。",
        "publisher_changed" => "发布者公钥与已信任记录不同。",
        "signature_invalid" => "插件签名或文件摘要不一致。",
        "stale_snapshot" => "快照已失效，请重新读取。",
        "object_context_required" => "请从对应对象详情中打开此摘要，并单独确认字段与用途。",
        "deadline_exceeded" => "插件运行超时，任务已终止。",
        "resource_limit" => "插件超过资源限制，任务已终止。",
        "plan_capacity_full" => {
            "配置计划名额已满。请在配置备份与恢复中放弃不再需要的未执行预览；已应用、待核查和带备份的记录会继续保留。"
        }
        "worker_crashed" => "插件运行失败，原有功能继续可用。",
        "protocol_binding_active" => "请先在此插件中解除协议绑定，再卸载插件。",
        "catalog_binding_active" => {
            "请先解除此插件的模型目录绑定，再卸载插件；停用或撤权不会自动改用其他目录。"
        }
        "invalid_record" => "插件扩展记录损坏或版本不兼容，请保留原文件与备份，核对后再操作。",
        "busy" => "本机同步任务繁忙，请稍后重试。",
        "unavailable" => "插件扩展数据暂不可用；请保留原文件并检查权限或备份。现有功能仍可使用。",
        "unsupported" => "此能力尚未对外开放。",
        "not_found" => "插件或视图不可用。",
        _ => "插件操作未完成，请核对包格式和当前状态。",
    }
}

#[derive(Clone)]
pub(crate) struct Cached {
    pub observed_at: String,
    pub report: Option<GatewayReport>,
    pub collected_at: std::time::Instant,
    pub digest: String,
    pub epoch: String,
    pub revision: String,
    pub snapshot: String,
    pub sessions: Vec<SessionCandidate>,
    pub source_revision: Option<String>,
    pub quota: Option<Quota>,
}
pub(crate) struct Manager {
    installation: Mutex<()>,
    objects: Mutex<object_contributions::Host>,
    full: Mutex<full::Host>,
    policy: Mutex<policy::Host>,
    policy_presence: Arc<std::sync::RwLock<policy::Presence>>,
    full_publisher_revisions: Mutex<BTreeMap<String, Option<u64>>>,
    domain: Mutex<domain_store::Host>,
    diagnostics: diagnostics::Diagnostics,
    root: PathBuf,
    db: Database,
    development: bool,
    cache: Mutex<BTreeMap<String, Arc<Cached>>>,
    gates: Mutex<BTreeMap<String, Weak<tokio::sync::Mutex<()>>>>,
    activation: Arc<tokio::sync::Mutex<()>>,
    workers: Arc<tokio::sync::Semaphore>,
    catalog_instance: String,
    vault: Arc<dyn Vault>,
    stopping: AtomicBool,
    advanced: Mutex<advanced::Store>,
    connectors: Mutex<BTreeMap<String, advanced::Live>>,
    source_watches: Mutex<BTreeMap<String, tokio::task::JoinHandle<()>>>,
    pub(crate) connection_reviews: Mutex<crate::agent_connections::Reviews>,
}
pub(crate) use full::ConnectionAccess;
impl Manager {
    pub(crate) fn stop(&self) {
        self.stopping.store(true, Ordering::Release);
        for (_, task) in std::mem::take(
            &mut *self
                .source_watches
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        ) {
            task.abort();
        }
        self.connectors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
    fn gate(&self, id: &str) -> Result<Arc<tokio::sync::Mutex<()>>> {
        let mut gates = self.gates.lock().checked("unavailable")?;
        // Owned guards keep live gates shared; idle plugin identities need no lock.
        gates.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = gates.get(id).and_then(Weak::upgrade) {
            return Ok(gate);
        }
        let gate = Arc::new(tokio::sync::Mutex::new(()));
        gates.insert(id.to_owned(), Arc::downgrade(&gate));
        Ok(gate)
    }
    fn ensure_running(&self) -> Result<()> {
        if self.stopping.load(Ordering::Acquire) {
            Err(fail("plugin_disabled"))
        } else {
            Ok(())
        }
    }
}
