use super::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    sync::{Arc, RwLock},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Provider {
    pub provider_key: String,
    pub plugin_id: String,
    pub implementation_version: String,
    pub implementation_identity: String,
    pub capability: Capability,
    pub execution_kind: ExecutionKind,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Product {
    pub id: String,
    pub display_name: String,
    pub source_id: String,
    pub config_id: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Source {
    pub id: String,
    pub provider_key: String,
    pub product_ids: Vec<String>,
    pub status: ProviderStatus,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogSnapshot {
    pub api_version: String,
    pub revision: Decimal,
    pub providers: Vec<Provider>,
    pub products: Vec<Product>,
    pub sources: Vec<Source>,
}
/// Snapshots are immutable; failed validation never changes the published catalog.
pub struct Catalog(RwLock<Arc<CatalogSnapshot>>);
impl Catalog {
    pub fn new(snapshot: CatalogSnapshot) -> Result<Self> {
        snapshot.validate()?;
        Ok(Self(RwLock::new(Arc::new(snapshot))))
    }
    pub fn snapshot(&self) -> Result<Arc<CatalogSnapshot>> {
        self.0
            .read()
            .map(|s| s.clone())
            .map_err(|_| Failure::new(ErrorCode::Unavailable))
    }
    pub fn replace(&self, expected: &Decimal, next: CatalogSnapshot) -> Result<()> {
        next.validate()?;
        let mut current = self
            .0
            .write()
            .map_err(|_| Failure::new(ErrorCode::Unavailable))?;
        if &current.revision != expected || next.revision.value() <= expected.value() {
            return Err(Failure::new(ErrorCode::StaleSnapshot));
        }
        *current = Arc::new(next);
        Ok(())
    }
}
pub(crate) fn external_id(id: &str, plugin: &str) -> bool {
    if !(5..=100).contains(&plugin.len())
        || !plugin.contains('.')
        || plugin.starts_with('.')
        || plugin.ends_with('.')
        || plugin.contains("..")
        || plugin.starts_with("builtin.")
        || plugin.starts_with("local-agent.")
        || plugin.starts_with("io.localagent.")
        || !plugin
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b".-".contains(&b))
    {
        return false;
    }
    id.strip_prefix(plugin)
        .and_then(|s| s.strip_prefix(':'))
        .is_some_and(|local| {
            !local.is_empty()
                && local.len() <= 128
                && local
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
        })
}
impl CatalogSnapshot {
    pub fn validate(&self) -> Result<()> {
        let invalid = || Failure::new(ErrorCode::InvalidInput);
        if self.api_version != API_VERSION {
            return Err(Failure::new(ErrorCode::IncompatibleApi));
        }
        let mut keys = HashSet::new();
        let mut capabilities = HashSet::new();
        for p in &self.providers {
            if p.provider_key.is_empty()
                || p.plugin_id.is_empty()
                || p.implementation_identity.is_empty()
                || p.capability.name != "domain.session-source"
                || p.capability.major != 1
                || p.capability.instance_id.is_empty()
                || !keys.insert(&p.provider_key)
                || !capabilities.insert((
                    &p.capability.name,
                    p.capability.major,
                    &p.capability.instance_id,
                ))
            {
                return Err(Failure::new(ErrorCode::CapabilityConflict));
            }
        }
        let mut source_ids = HashSet::new();
        let mut product_ids = HashSet::new();
        for source in &self.sources {
            let provider = self
                .providers
                .iter()
                .find(|p| p.provider_key == source.provider_key)
                .ok_or_else(invalid)?;
            if source.id != provider.capability.instance_id
                || !source_ids.insert(&source.id)
                || source.product_ids.is_empty()
                || source.product_ids.iter().collect::<HashSet<_>>().len()
                    != source.product_ids.len()
            {
                return Err(invalid());
            }
            if provider.execution_kind != ExecutionKind::Builtin
                && !external_id(&source.id, &provider.plugin_id)
            {
                return Err(invalid());
            }
        }
        for product in &self.products {
            let source = self
                .sources
                .iter()
                .find(|s| s.id == product.source_id)
                .ok_or_else(invalid)?;
            let provider = self
                .providers
                .iter()
                .find(|p| p.provider_key == source.provider_key)
                .ok_or_else(invalid)?;
            if product.display_name.is_empty()
                || !source.product_ids.contains(&product.id)
                || !product_ids.insert(&product.id)
                || (provider.execution_kind != ExecutionKind::Builtin
                    && (!external_id(&product.id, &provider.plugin_id)
                        || product.config_id.is_some()))
            {
                return Err(invalid());
            }
        }
        if self
            .sources
            .iter()
            .any(|s| s.product_ids.iter().any(|id| !product_ids.contains(id)))
        {
            return Err(invalid());
        }
        Ok(())
    }
}
