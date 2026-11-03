//! Compatibility consumption of a request source. Endpoint, client/peer,
//! routing, attempt clocks, billing and exact links never leave the host frame.
use super::Request;
use crate::domain::{self as d, requests as r};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

/// A compiled composition reads the one host-bound source, then returns the
/// public result. It cannot access the private request or authorize a source.
pub trait RequestReader: Send + Sync {
    fn identity(&self) -> &str;
    fn read_request(
        &self,
        source: &mut dyn r::RequestSource,
        read: &r::RequestRead,
    ) -> d::Result<r::RequestSnapshot>;
}
struct BuiltinReader;
impl RequestReader for BuiltinReader {
    fn identity(&self) -> &str {
        concat!("builtin.request-reader@", env!("CARGO_PKG_VERSION"))
    }
    fn read_request(
        &self,
        source: &mut dyn r::RequestSource,
        read: &r::RequestRead,
    ) -> d::Result<r::RequestSnapshot> {
        source.read_request(read)
    }
}
pub struct RequestProjection {
    native: Arc<dyn RequestReader>,
    gateway: Arc<dyn RequestReader>,
    epoch: String,
    sequence: AtomicU64,
}
impl Default for RequestProjection {
    fn default() -> Self {
        Self::with_readers(Arc::new(BuiltinReader), Arc::new(BuiltinReader))
            .expect("compiled reader identities are valid")
    }
}
impl RequestProjection {
    /// Select readers when creating a disposable history index, before any
    /// publication or frozen query exists. There is no live replacement/cache reuse.
    pub fn with_readers(
        native: Arc<dyn RequestReader>,
        gateway: Arc<dyn RequestReader>,
    ) -> d::Result<Self> {
        if [&native, &gateway]
            .into_iter()
            .any(|r| r.identity().is_empty() || r.identity().len() > 192)
        {
            return Err(d::Failure::new(d::ErrorCode::InvalidInput));
        }
        Ok(Self {
            native,
            gateway,
            epoch: uuid::Uuid::new_v4().to_string(),
            sequence: AtomicU64::new(0),
        })
    }
    pub fn capabilities(&self) -> Vec<d::CapabilityEntry> {
        [
            provider("native", self.native.identity()),
            provider("gateway", self.gateway.identity()),
        ]
        .into_iter()
        .map(capability)
        .collect()
    }
    pub(crate) fn project(&self, row: Request) -> Result<Request, String> {
        let (slot, unit, reader) = match row.source.as_str() {
            "native" => ("native", r::RequestUnit::ModelResponse, &self.native),
            "gateway" => ("gateway", r::RequestUnit::LogicalRequest, &self.gateway),
            _ => return Err(invalid()),
        };
        let sequence = self
            .sequence
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_add(1))
            .map_err(|_| invalid())?;
        let p = provider(slot, reader.identity());
        let read = r::RequestRead {
            provider_key: p.provider_key,
            implementation_identity: p.implementation_identity,
            read_revision: format!("{}:{sequence}", self.epoch),
            source_id: row.agent_id.clone(),
            record_id: row.id.clone(),
            unit,
        };
        let snapshot = {
            let mut source = PublishedRequest {
                expected: &read,
                row: &row,
                read: false,
            };
            let snapshot = reader
                .read_request(&mut source, &read)
                .map_err(|_| invalid())?;
            if !source.read {
                return Err(invalid());
            }
            snapshot
        };
        if provider(slot, reader.identity()).implementation_identity != read.implementation_identity
        {
            return Err(invalid());
        }
        consume(row, &read, snapshot)
    }
}
pub fn builtin_capabilities() -> Vec<d::CapabilityEntry> {
    ["native", "gateway"]
        .into_iter()
        .map(|slot| capability(provider(slot, BuiltinReader.identity())))
        .collect()
}
fn provider(slot: &str, identity: &str) -> d::Provider {
    d::Provider {
        provider_key: format!("builtin:domain.request-source@1:{slot}"),
        plugin_id: "builtin.requests".into(),
        implementation_version: env!("CARGO_PKG_VERSION").into(),
        implementation_identity: format!("request-projection:{slot}:{identity}"),
        capability: d::Capability {
            name: "domain.request-source".into(),
            major: 1,
            instance_id: slot.into(),
        },
        execution_kind: d::ExecutionKind::Builtin,
    }
}
fn capability(provider: d::Provider) -> d::CapabilityEntry {
    d::CapabilityEntry {
        provider,
        status: d::ProviderStatus {
            enabled: true,
            authorization: d::Authorization::Authorized,
            grant_revision: 1u64.into(),
            runtime_active: true,
            freshness: d::Freshness::Current,
        },
        operations: vec![d::ReadOperation::ReadRequests],
    }
}
fn invalid() -> String {
    "请求结果未通过统一领域契约核查，原记录保持不变".into()
}
fn timestamp(value: &str) -> Option<String> {
    d::timestamp(value).map(|_| value.into())
}
fn finite(value: Option<f64>) -> Option<f64> {
    value.filter(|v| v.is_finite() && *v >= 0.0)
}
fn observation(row: &Request, unit: r::RequestUnit) -> r::RequestObservation {
    r::RequestObservation {
        id: row.id.clone(),
        source_id: row.agent_id.clone(),
        unit,
        product_id: row.agent_product_id.clone(),
        session_ref: row.session_id.clone(),
        session_title: row.session_title.clone(),
        project_ref: row.project_id.clone(),
        project_name: row.project_name.clone(),
        occurred_at: timestamp(&row.started_at),
        ended_at: row.ended_at.as_deref().and_then(timestamp),
        model: row.model.clone(),
        provider_key: row.provider_key.clone(),
        provider_name: row.provider.clone(),
        outcome: row.status.clone(),
        usage: (&row.usage).into(),
        usage_missing: (row.usage_missing as u64).into(),
        attempts: (row.attempts.len() as u64).into(),
        duration_ms: finite(row.duration_ms),
        ttft_ms: finite(row.ttft_ms),
        throughput: finite(row.throughput),
    }
}
struct PublishedRequest<'a> {
    expected: &'a r::RequestRead,
    row: &'a Request,
    read: bool,
}
impl r::RequestSource for PublishedRequest<'_> {
    fn read_request(&mut self, read: &r::RequestRead) -> d::Result<r::RequestSnapshot> {
        if read != self.expected || self.read {
            return Err(d::Failure::new(d::ErrorCode::StaleSnapshot));
        }
        self.read = true;
        let result = r::RequestSnapshot {
            read: read.clone(),
            record: observation(self.row, read.unit),
        };
        result.validate(read)?;
        Ok(result)
    }
}
fn consume(
    mut row: Request,
    read: &r::RequestRead,
    snapshot: r::RequestSnapshot,
) -> Result<Request, String> {
    snapshot.validate(read).map_err(|_| invalid())?;
    let mut sealed = observation(&row, read.unit);
    // Display labels are replaceable; identity and evidence used by correlation,
    // timing, statistics and billing must still belong to this exact host frame.
    sealed.session_title = snapshot.record.session_title.clone();
    sealed.project_name = snapshot.record.project_name.clone();
    sealed.provider_name = snapshot.record.provider_name.clone();
    if sealed != snapshot.record {
        return Err(invalid());
    }
    let record = snapshot.record;
    row.id = record.id;
    row.agent_id = record.source_id;
    row.agent_product_id = record.product_id;
    row.session_id = record.session_ref;
    row.session_title = record.session_title;
    row.project_id = record.project_ref;
    row.project_name = record.project_name;
    if let Some(at) = record.occurred_at {
        row.started_at = at;
    }
    if record.ended_at.is_some() {
        row.ended_at = record.ended_at;
    }
    row.model = record.model;
    row.provider_key = record.provider_key;
    row.provider = record.provider_name;
    row.status = record.outcome;
    row.usage = crate::sessions::Usage {
        input_tokens: record.usage.input_tokens.map(|n| n.value()),
        output_tokens: record.usage.output_tokens.map(|n| n.value()),
        cached_input_tokens: record.usage.cached_input_tokens.map(|n| n.value()),
        cache_write_input_tokens: record.usage.cache_write_input_tokens.map(|n| n.value()),
        reasoning_output_tokens: record.usage.reasoning_output_tokens.map(|n| n.value()),
        total_tokens: record.usage.total_tokens.map(|n| n.value()),
    };
    row.usage_missing = usize::try_from(record.usage_missing.value()).map_err(|_| invalid())?;
    row.duration_ms = record.duration_ms;
    row.ttft_ms = record.ttft_ms;
    row.throughput = record.throughput;
    Ok(row)
}
#[cfg(test)]
mod tests;
