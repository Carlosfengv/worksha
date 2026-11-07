//! Trusted billing adapters receive one scoped request; caches and credentials stay in Billing.
use super::AccountSnapshot;
use local_agent_core::plugin::{
    BuiltinPlugin, CapabilityKey, PluginDescriptor, PluginError, PluginHost, PluginResources,
};
use std::{
    collections::BTreeMap,
    future::Future,
    path::Path,
    pin::Pin,
    sync::{Arc, OnceLock},
};
mod domain;

fn codex_capacity() -> Arc<tokio::sync::Semaphore> {
    static CAPACITY: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
    CAPACITY
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(1)))
        .clone()
}

type ReadFuture<'a> =
    Pin<Box<dyn Future<Output = Result<AccountSnapshot, &'static str>> + Send + 'a>>;
enum ReadRequest<'a> {
    Codex,
    Api {
        client: &'a reqwest::Client,
        url: &'a str,
        secret: &'a str,
    },
}
trait AccountSource: BuiltinPlugin {
    fn kind(&self) -> &'static str;
    fn read<'a>(&'a self, request: ReadRequest<'a>) -> ReadFuture<'a>;
    fn collected(&self, _directory: &Path) -> Result<Option<AccountSnapshot>, &'static str> {
        Ok(None)
    }
}
struct NativeSource(&'static str);
fn key(kind: &'static str) -> CapabilityKey {
    CapabilityKey {
        name: "account.source",
        version: 1,
        instance: kind,
    }
}
impl BuiltinPlugin for NativeSource {
    fn display_name(&self) -> String {
        let name = match self.0 {
            "codex" => "Codex",
            "claude" => "Claude",
            "deepseek" => "DeepSeek",
            "openrouter" => "OpenRouter",
            "kimi-cn" => "Kimi 国内",
            "kimi-global" => "Kimi 国际",
            _ => self.0,
        };
        format!("{name} 账户读取")
    }
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor {
            id: self.0,
            version: "1.0.0",
            provides: vec![key(self.0)],
            requires: vec![],
        }
    }
    fn prepare(&self) -> Result<PluginResources, PluginError> {
        Ok(PluginResources::default())
    }
    fn capability_metadata(
        &self,
        _: &CapabilityKey,
    ) -> local_agent_core::plugin::builtin::CapabilityMetadata {
        use local_agent_core::plugin::builtin::{
            CapabilityKind, CapabilityMetadata, ResourceScope,
        };
        CapabilityMetadata {
            kind: CapabilityKind::Query,
            resource_scope: ResourceScope::Provider,
        }
    }
}
impl AccountSource for NativeSource {
    fn kind(&self) -> &'static str {
        self.0
    }
    fn read<'a>(&'a self, request: ReadRequest<'a>) -> ReadFuture<'a> {
        Box::pin(async move {
            match (self.0, request) {
                ("codex", ReadRequest::Codex) => {
                    crate::blocking::run_with(codex_capacity(), super::codex::read)
                        .await
                        .map_err(|error| match error {
                            crate::blocking::Error::Busy => "Codex 账户读取仍在进行，请稍后重试",
                            crate::blocking::Error::Unavailable => "Codex 读取进程中断",
                        })?
                }
                (
                    "deepseek" | "openrouter" | "kimi-cn" | "kimi-global",
                    ReadRequest::Api {
                        client,
                        url,
                        secret,
                    },
                ) => super::api::read(client, self.0, url, secret).await,
                _ => Err("该账户来源尚未提供直接读取能力"),
            }
        })
    }
    fn collected(&self, directory: &Path) -> Result<Option<AccountSnapshot>, &'static str> {
        if self.0 == "claude" {
            super::read_collected(directory)
        } else {
            Ok(None)
        }
    }
}
pub(crate) struct AccountSources {
    client: reqwest::Client,
    host: Arc<PluginHost>,
    sources: BTreeMap<&'static str, Arc<dyn AccountSource>>,
}
impl AccountSources {
    pub(crate) fn registrations() -> Vec<Arc<dyn BuiltinPlugin>> {
        [
            "codex",
            "claude",
            "deepseek",
            "openrouter",
            "kimi-cn",
            "kimi-global",
        ]
        .into_iter()
        .map(|kind| Arc::new(NativeSource(kind)) as Arc<dyn BuiltinPlugin>)
        .collect()
    }
    pub(crate) fn with_host(host: Arc<PluginHost>) -> Result<Self, PluginError> {
        let sources = [
            "codex",
            "claude",
            "deepseek",
            "openrouter",
            "kimi-cn",
            "kimi-global",
        ]
        .into_iter()
        .map(|kind| Arc::new(NativeSource(kind)) as Arc<dyn AccountSource>)
        .map(|source| (source.kind(), source))
        .collect();
        let client = super::api::client()
            .map_err(|message| PluginError::new("account_http_unavailable", message))?;
        Ok(Self {
            client,
            host,
            sources,
        })
    }
    pub(crate) fn domain_capabilities(&self) -> Vec<local_agent_core::domain::CapabilityEntry> {
        use local_agent_core::domain as d;
        self.sources
            .iter()
            .map(|(kind, source)| {
                let descriptor = source.descriptor();
                let lease = self.host.acquire(&key(kind));
                let active = lease.is_ok();
                d::CapabilityEntry {
                    provider: d::Provider {
                        provider_key: format!("builtin:domain.account-source@1:{kind}"),
                        plugin_id: descriptor.id.into(),
                        implementation_version: env!("CARGO_PKG_VERSION").into(),
                        implementation_identity: format!(
                            "local-agent-service@{}:account:{kind}",
                            env!("CARGO_PKG_VERSION")
                        ),
                        capability: d::Capability {
                            name: "domain.account-source".into(),
                            major: 1,
                            instance_id: (*kind).into(),
                        },
                        execution_kind: d::ExecutionKind::Builtin,
                    },
                    status: d::ProviderStatus {
                        enabled: active,
                        // Each bound account has its own explicit refresh authorization.
                        authorization: d::Authorization::Required,
                        grant_revision: lease.map(|l| l.epoch).unwrap_or(0).into(),
                        runtime_active: active,
                        freshness: d::Freshness::Unread,
                    },
                    operations: vec![d::ReadOperation::RefreshQuota],
                }
            })
            .collect()
    }
    #[cfg(test)]
    pub(crate) fn builtin() -> Result<Self, PluginError> {
        Self::mount(
            [
                "codex",
                "claude",
                "deepseek",
                "openrouter",
                "kimi-cn",
                "kimi-global",
            ]
            .into_iter()
            .map(|kind| Arc::new(NativeSource(kind)) as Arc<dyn AccountSource>)
            .collect(),
        )
    }
    #[cfg(test)]
    fn mount(sources: Vec<Arc<dyn AccountSource>>) -> Result<Self, PluginError> {
        let client = super::api::client().map_err(|message| PluginError {
            code: "account_http_unavailable",
            message: message.into(),
        })?;
        let descriptors: Vec<&dyn BuiltinPlugin> = sources
            .iter()
            .map(|s| s.as_ref() as &dyn BuiltinPlugin)
            .collect();
        let host = PluginHost::mount(&descriptors)?;
        Ok(Self {
            client,
            host: Arc::new(host),
            sources: sources.into_iter().map(|s| (s.kind(), s)).collect(),
        })
    }
    pub(crate) fn active(&self, kind: &'static str) -> bool {
        self.host.acquire(&key(kind)).is_ok()
    }
    pub(crate) fn lease(
        &self,
        kind: &'static str,
    ) -> Result<local_agent_core::plugin::PluginLease, PluginError> {
        self.host.acquire(&key(kind))
    }
    async fn read(
        &self,
        kind: &'static str,
        request: ReadRequest<'_>,
    ) -> Result<AccountSnapshot, &'static str> {
        let lease = self
            .host
            .acquire(&key(kind))
            .map_err(|_| "只读额度插件已停止")?;
        let source = self.sources.get(kind).ok_or("账户来源未注册")?;
        let result = domain::read(source.as_ref(), request, kind, lease.epoch).await?;
        lease
            .ensure_active()
            .map_err(|_| "读取期间额度插件已停止，已丢弃结果")?;
        Ok(result)
    }
    pub(crate) async fn codex(&self) -> Result<AccountSnapshot, &'static str> {
        self.read("codex", ReadRequest::Codex).await
    }
    pub(crate) async fn api(
        &self,
        kind: &'static str,
        url: &str,
        secret: &str,
    ) -> Result<AccountSnapshot, &'static str> {
        self.read(
            kind,
            ReadRequest::Api {
                client: &self.client,
                url,
                secret,
            },
        )
        .await
    }
    pub(crate) fn collected(
        &self,
        directory: &Path,
    ) -> Result<Option<AccountSnapshot>, &'static str> {
        let lease = self
            .host
            .acquire(&key("claude"))
            .map_err(|_| "只读额度插件已停止")?;
        let result = self.sources["claude"].collected(directory)?;
        lease.ensure_active().map_err(|_| "只读额度插件已停止")?;
        Ok(result)
    }
    #[cfg(test)]
    pub(crate) fn stop_all(&self) {
        for source in self.sources.values() {
            let _ = self.host.stop(source.descriptor().id);
        }
    }
    #[cfg(test)]
    pub(super) fn stop(&self, kind: &str) {
        self.host.stop(kind).unwrap();
    }
    #[cfg(test)]
    pub(super) fn restart(&self) {
        let factories: Vec<&dyn BuiltinPlugin> = self
            .sources
            .values()
            .map(|source| source.as_ref() as &dyn BuiltinPlugin)
            .collect();
        let enabled = self
            .sources
            .values()
            .map(|source| source.descriptor().id.to_owned())
            .collect();
        self.host
            .reconfigure(
                &self.host.snapshot().unwrap().generation,
                &factories,
                &enabled,
                || Ok(()),
            )
            .unwrap();
    }
}

#[cfg(test)]
pub(super) struct ControlledSource {
    pub entered: tokio::sync::Notify,
    pub resume: tokio::sync::Notify,
    pub calls: std::sync::atomic::AtomicUsize,
    pub fail: std::sync::atomic::AtomicBool,
}
#[cfg(test)]
impl BuiltinPlugin for ControlledSource {
    fn descriptor(&self) -> PluginDescriptor {
        NativeSource("openrouter").descriptor()
    }
    fn prepare(&self) -> Result<PluginResources, PluginError> {
        Ok(PluginResources::default())
    }
}
#[cfg(test)]
impl AccountSource for ControlledSource {
    fn kind(&self) -> &'static str {
        "openrouter"
    }
    fn read<'a>(&'a self, _: ReadRequest<'a>) -> ReadFuture<'a> {
        Box::pin(async move {
            self.calls
                .fetch_add(1, std::sync::atomic::Ordering::Release);
            self.entered.notify_one();
            self.resume.notified().await;
            if self.fail.load(std::sync::atomic::Ordering::Acquire) {
                Err("受控账户读取失败")
            } else {
                Ok(AccountSnapshot::new("api", "controlled-account"))
            }
        })
    }
}
#[cfg(test)]
pub(super) fn controlled() -> (Arc<AccountSources>, Arc<ControlledSource>) {
    let source = Arc::new(ControlledSource {
        entered: Default::default(),
        resume: Default::default(),
        calls: Default::default(),
        fail: Default::default(),
    });
    (
        Arc::new(AccountSources::mount(vec![source.clone()]).unwrap()),
        source,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_directory_describes_account_adapters_without_reading_or_binding_accounts() {
        let (sources, controlled) = super::controlled();
        let entries = sources.domain_capabilities();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].provider.capability.name, "domain.account-source");
        assert_eq!(entries[0].provider.capability.instance_id, "openrouter");
        assert_eq!(
            entries[0].status.authorization,
            local_agent_core::domain::Authorization::Required
        );
        assert_eq!(
            controlled.calls.load(std::sync::atomic::Ordering::Acquire),
            0
        );
        sources.stop("openrouter");
        assert!(!sources.domain_capabilities()[0].status.runtime_active);
        assert_eq!(
            controlled.calls.load(std::sync::atomic::Ordering::Acquire),
            0
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_account_waiter_retains_exclusive_process_slot_without_using_management() {
        let capacity = codex_capacity();
        let started = Arc::new(tokio::sync::Notify::new());
        let (release, gate) = std::sync::mpsc::channel();
        let caller = tokio::spawn({
            let capacity = capacity.clone();
            let started = started.clone();
            async move {
                crate::blocking::run_with(capacity, move || {
                    started.notify_one();
                    gate.recv_timeout(std::time::Duration::from_secs(3))
                        .unwrap();
                })
                .await
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), started.notified())
            .await
            .unwrap();
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        assert!(matches!(
            crate::blocking::run_with(capacity.clone(), || panic!("second process must not start"))
                .await,
            Err(crate::blocking::Error::Busy)
        ));
        assert_eq!(crate::blocking::run(|| 42).await.unwrap(), 42);
        release.send(()).unwrap();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
        while capacity.available_permits() == 0 {
            assert!(tokio::time::Instant::now() < deadline);
            tokio::task::yield_now().await;
        }
        assert_eq!(capacity.available_permits(), 1);
    }
    struct PausedSource {
        entered: tokio::sync::Notify,
        resume: tokio::sync::Notify,
    }
    impl BuiltinPlugin for PausedSource {
        fn descriptor(&self) -> PluginDescriptor {
            NativeSource("fixture").descriptor()
        }
        fn prepare(&self) -> Result<PluginResources, PluginError> {
            Ok(PluginResources::default())
        }
    }
    impl AccountSource for PausedSource {
        fn kind(&self) -> &'static str {
            "fixture"
        }
        fn read<'a>(&'a self, _: ReadRequest<'a>) -> ReadFuture<'a> {
            Box::pin(async move {
                self.entered.notify_one();
                self.resume.notified().await;
                Ok(AccountSnapshot::new("api", "fixture"))
            })
        }
    }
    #[tokio::test]
    async fn stopped_reader_discards_late_results_without_affecting_other_sources() {
        let source = Arc::new(PausedSource {
            entered: Default::default(),
            resume: Default::default(),
        });
        let sources = Arc::new(
            AccountSources::mount(vec![source.clone(), Arc::new(NativeSource("claude"))]).unwrap(),
        );
        let read = tokio::spawn({
            let sources = sources.clone();
            async move { sources.read("fixture", ReadRequest::Codex).await }
        });
        source.entered.notified().await;
        sources.stop("fixture");
        source.resume.notify_one();
        assert!(read.await.unwrap().is_err());
        assert!(sources.active("claude"));
        assert!(sources.read("fixture", ReadRequest::Codex).await.is_err());
    }
}
