//! Describe only actual mounted domain adapters, including shared CLI/desktop sources.
use super::{NativeError, NativeResult, NativeSessions};
use crate::domain as d;

impl NativeSessions {
    /// General metadata does not widen the legacy session picker or read resources.
    pub fn domain_capabilities(&self) -> NativeResult<Vec<d::CapabilityEntry>> {
        use d::ReadOperation::*;
        let catalog = self.domain_catalog()?;
        let mut entries = Vec::new();
        for source in &catalog.sources {
            let provider = catalog
                .providers
                .iter()
                .find(|p| p.provider_key == source.provider_key)
                .ok_or_else(|| NativeError::new("unavailable", "领域目录未通过核查"))?;
            for (name, operations) in [
                ("domain.session-source", vec![Scan, ReadConversation]),
                ("domain.activity-source", vec![CollectActivity]),
                ("domain.memory-source", vec![ListMemory, ReadMemory]),
                ("domain.call-source", vec![ScanCalls]),
                ("domain.mcp-source", vec![CatalogFacts, ExecutionFacts]),
            ] {
                let mut provider = provider.clone();
                provider.capability.name = name.into();
                provider.provider_key = format!("builtin:{name}@1:{}", source.id);
                entries.push(d::CapabilityEntry {
                    provider,
                    status: source.status.clone(),
                    operations,
                });
            }
        }
        entries.push(d::CapabilityEntry {
            provider: self.domain_mcp_provider(),
            // A read facade is available, but metadata never grants access to its sources.
            status: d::ProviderStatus {
                enabled: true,
                authorization: d::Authorization::Required,
                grant_revision: 1u64.into(),
                runtime_active: true,
                freshness: d::Freshness::Unread,
            },
            operations: vec![CatalogFacts, ExecutionFacts],
        });
        Ok(entries)
    }
    /// Read-only metadata. No scan, worker, connection, counts or native paths.
    pub fn domain_catalog(&self) -> NativeResult<d::CatalogSnapshot> {
        let state = self.lock()?;
        let mut providers = Vec::new();
        let mut products = Vec::new();
        let mut sources = Vec::new();
        for source in &state.sources {
            let Some(provider) = self
                .source_plugins
                .sources
                .get(source.agent_id.as_str())
                .and_then(|p| p.domain_provider())
            else {
                continue;
            };
            let source_products: Vec<_> = self
                .directory()
                .products()
                .iter()
                .filter(|p| p.family == source.agent_id)
                .map(|p| d::Product {
                    id: p.id.clone(),
                    display_name: p.name.clone(),
                    source_id: p.family.clone(),
                    config_id: Some(p.config_id.clone()),
                })
                .collect();
            let active = self.source_active(&source.agent_id);
            let status = d::ProviderStatus {
                enabled: source.enabled,
                authorization: if source.enabled {
                    d::Authorization::Authorized
                } else if source.authorization_revision > 1 {
                    d::Authorization::Revoked
                } else {
                    d::Authorization::Required
                },
                grant_revision: source.authorization_revision.into(),
                runtime_active: active,
                freshness: if !source.enabled || !active {
                    d::Freshness::Unread
                } else if source.status == "ready" {
                    d::Freshness::Current
                } else if source.last_successful_scan.is_some() {
                    d::Freshness::Stale
                } else if source.error.is_some() {
                    d::Freshness::Failed
                } else {
                    d::Freshness::Unread
                },
            };
            sources.push(d::Source {
                id: source.agent_id.clone(),
                provider_key: provider.provider_key.clone(),
                product_ids: source_products.iter().map(|p| p.id.clone()).collect(),
                status,
            });
            products.extend(source_products);
            providers.push(provider);
        }
        let snapshot = d::CatalogSnapshot {
            api_version: d::API_VERSION.into(),
            revision: 1u64.into(),
            providers,
            products,
            sources,
        };
        snapshot
            .validate()
            .map_err(|_| NativeError::new("unavailable", "领域目录未通过核查"))?;
        Ok(snapshot)
    }
}
