use super::{Checked, Lease, Manager, Query, Result, fail, runtime::Worker};
use crate::plugin_host::external::run_sync;
use futures_util::StreamExt;
use local_agent_core::{
    Provider,
    domain::protocol::{Finished, Open, Opened, ProtocolAdapter},
    plugin::advanced::{MAX_DOCUMENT, MAX_WIRE_CHUNK},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

mod adapter;
mod completion;
use adapter::GuestProtocolAdapter;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Binding {
    pub lease: Lease,
    pub provider_id: String,
    pub endpoint: String,
    pub revision: u64,
    pub credential_ref: Option<String>,
    pub protocol: String,
    pub allow_private: bool,
}
impl Manager {
    pub(super) fn bind_protocol(
        &self,
        q: &Query,
        p: &Provider,
        allow_private: bool,
    ) -> Result<Value> {
        if local_agent_core::providers::subscription::minimax_plan(p) {
            return Err(fail("unsupported"));
        }
        let (_, policy, lease) = self.extension(q)?;
        let spec = policy.protocol.ok_or(fail("unsupported"))?;
        if spec.base_protocol != p.protocol {
            return Err(fail("incompatible_api"));
        }
        let binding = Binding {
            lease,
            provider_id: p.id.clone(),
            endpoint: p.endpoint.clone(),
            revision: p.revision,
            credential_ref: p.credential_ref.clone(),
            protocol: p.protocol.clone(),
            allow_private,
        };
        let store = self.advanced.lock().checked("unavailable")?;
        let mut bindings = store.bindings()?;
        if bindings.contains_key(&p.id) {
            return Err(fail("capability_conflict"));
        }
        if bindings.len() >= 128 {
            return Err(fail("resource_limit"));
        }
        bindings.insert(p.id.clone(), binding);
        store.save_bindings(&bindings)?;
        Ok(json!({"bound":true,"providerId":p.id,"path":spec.path}))
    }
    pub(super) fn unbind_protocol(&self, q: &Query) -> Result<Value> {
        let store = self.advanced.lock().checked("unavailable")?;
        let mut bindings = store.bindings()?;
        // Recovery belongs to the plugin identity: the current version must be
        // able to release a binding whose old digest was invalidated by upgrade.
        bindings.retain(|_, b| b.lease.id != q.plugin_id);
        store.save_bindings(&bindings)?;
        Ok(json!({"unbound":true}))
    }
}
pub(crate) struct Prepared {
    pub client: reqwest::Client,
    pub url: url::Url,
    pub body: Vec<u8>,
    adapter: GuestProtocolAdapter,
    stream: bool,
    base_protocol: String,
}
pub(crate) async fn prepare(
    manager: Arc<Manager>,
    p: &Provider,
    body: &[u8],
) -> Result<Option<Prepared>> {
    let copy = manager.clone();
    let id = p.id.clone();
    let binding = run_sync(move || {
        let binding = copy
            .advanced
            .lock()
            .checked("unavailable")?
            .bindings()?
            .remove(&id);
        if let Some(binding) = &binding {
            binding.lease.check(&copy)?;
        }
        Ok(binding)
    })
    .await?;
    let Some(b) = binding else { return Ok(None) };
    if b.endpoint != p.endpoint
        || b.revision != p.revision
        || b.credential_ref != p.credential_ref
        || b.protocol != p.protocol
    {
        return Err(fail("grant_revoked"));
    }
    if body.len() > MAX_DOCUMENT as usize {
        return Err(fail("resource_limit"));
    }
    let q = Query {
        plugin_id: b.lease.id.clone(),
        digest: b.lease.digest.clone(),
    };
    let (_, policy, _) = manager.extension(&q)?;
    let policy = policy.protocol.ok_or(fail("unsupported"))?;
    let url = target(&p.endpoint, &policy.path)?;
    let client = pinned_client(&url, b.allow_private).await?;
    let request: Value = serde_json::from_slice(body).checked("invalid_input")?;
    let stream = request["stream"] == true;
    let mut adapter = GuestProtocolAdapter::new(Worker::open(manager, &q).await?);
    let Opened::Bytes(body) = adapter.open(Open { stream, request }).await? else {
        return Err(fail("invalid_input"));
    };
    Ok(Some(Prepared {
        client,
        url,
        body,
        adapter,
        stream,
        base_protocol: policy.base_protocol,
    }))
}
pub(super) fn target(endpoint: &str, path: &str) -> Result<url::Url> {
    local_agent_core::providers::validate_endpoint(endpoint).checked("invalid_input")?;
    if !super::safe_route(path) {
        return Err(fail("invalid_input"));
    }
    let mut url = url::Url::parse(endpoint).checked("invalid_input")?;
    url.set_path(&format!("{}{path}", url.path().trim_end_matches('/')));
    Ok(url)
}
pub(crate) async fn pinned_client(url: &url::Url, allow_private: bool) -> Result<reqwest::Client> {
    if !["http", "https"].contains(&url.scheme())
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(fail("permission_denied"));
    }
    let host = url
        .host_str()
        .ok_or(fail("invalid_input"))?
        .trim_matches(['[', ']']);
    let port = url.port_or_known_default().ok_or(fail("invalid_input"))?;
    let addresses: Vec<_> = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::net::lookup_host((host, port)),
    )
    .await
    .checked("deadline_exceeded")?
    .checked("unavailable")?
    .take(16)
    .collect();
    if addresses.is_empty()
        || (!allow_private
            && addresses
                .iter()
                .any(|a| !crate::plugin_host::external::broker::public_address(a.ip())))
    {
        return Err(fail("permission_denied"));
    }
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(host, &addresses)
        .connect_timeout(Duration::from_secs(5))
        .build()
        .checked("unavailable")
}
impl Prepared {
    pub(crate) async fn response(
        self,
        response: reqwest::Response,
        credential: Option<&[u8]>,
    ) -> Result<reqwest::Response> {
        // An HTTP rejection is handled by the host without processing it in the guest.
        if !response.status().is_success() {
            return Ok(response);
        }
        self.adapter.check().await?;
        if response
            .headers()
            .get("content-encoding")
            .is_some_and(|h| h != "identity")
        {
            return Err(fail("unsupported"));
        }
        let mut result = axum::http::Response::builder().status(response.status());
        *result.headers_mut().ok_or(fail("unavailable"))? = response.headers().clone();
        result
            .headers_mut()
            .ok_or(fail("unavailable"))?
            .remove("content-length");
        result.headers_mut().ok_or(fail("unavailable"))?.insert(
            "content-type",
            axum::http::HeaderValue::from_static(if self.stream {
                "text/event-stream"
            } else {
                "application/json"
            }),
        );
        let mut filter = SecretFilter {
            secret: credential.unwrap_or_default().to_vec(),
            pending: Vec::new(),
        };
        let stream = async_stream::try_stream! {
            let mut adapter=self.adapter;let mut source=response.bytes_stream();let mut total=0usize;
            let mut completion=self.stream.then(||completion::Completion::new(&self.base_protocol));
            loop {
                let next=loop {let event=tokio::select! {r=source.next()=>Some(r),_=tokio::time::sleep(Duration::from_millis(50))=>None};if let Some(next)=event {break next;}adapter.check().await.map_err(io)?;};
                let Some(next)=next else{break};let chunk=next.map_err(|_|std::io::Error::other("upstream_failed"))?;total=total.saturating_add(chunk.len());if total>8*1024*1024 {Err(std::io::Error::other("resource_limit"))?;}
                let safe=filter.push(&chunk,false).map_err(io)?;
                for bytes in safe.chunks(MAX_WIRE_CHUNK) {
                    let v=adapter.feed(bytes).await.map_err(io)?;
                    for bytes in v.chunks {
                        if let Some(completion)=&mut completion {for bytes in completion.push(&bytes).map_err(io)? {yield axum::body::Bytes::from(bytes);}}
                        else {yield axum::body::Bytes::from(bytes);}
                    }
                }
            }
            let tail=filter.push(&[],true).map_err(io)?;
            if !tail.is_empty() {
                let v=adapter.feed(&tail).await.map_err(io)?;
                for bytes in v.chunks {
                    if let Some(completion)=&mut completion {for bytes in completion.push(&bytes).map_err(io)? {yield axum::body::Bytes::from(bytes);}}
                    else {yield axum::body::Bytes::from(bytes);}
                }
            }
            let v=match adapter.finish().await.map_err(io)? {
                Finished::Stream(v)=>v,
                Finished::Json(_)=>Err(std::io::Error::other("invalid_input"))?,
            };
            adapter.close().await.map_err(io)?;
            for bytes in v.chunks {
                if let Some(completion)=&mut completion {for bytes in completion.push(&bytes).map_err(io)? {yield axum::body::Bytes::from(bytes);}}
                else {yield axum::body::Bytes::from(bytes);}
            }
            if let Some(completion)=completion {for bytes in completion.finish() {yield axum::body::Bytes::from(bytes);}}
        };
        let stream: futures_util::stream::BoxStream<
            'static,
            std::result::Result<axum::body::Bytes, std::io::Error>,
        > = Box::pin(stream);
        Ok(reqwest::Response::from(
            result
                .body(reqwest::Body::wrap_stream(stream))
                .checked("unavailable")?,
        ))
    }
}
fn io(e: super::super::Error) -> std::io::Error {
    std::io::Error::other(e.code)
}

/// Do not deliver an echoed host credential to a guest, including across chunks
/// and JSON escapes. Retain only incomplete escape sequences / secret prefixes.
pub(super) struct SecretFilter {
    pub secret: Vec<u8>,
    pub pending: Vec<u8>,
}
impl SecretFilter {
    pub(super) fn push(&mut self, bytes: &[u8], finish: bool) -> Result<Vec<u8>> {
        self.pending.extend_from_slice(bytes);
        if self.secret.is_empty() {
            return Ok(std::mem::take(&mut self.pending));
        }
        let mut decoded = Vec::new();
        let mut starts = Vec::new();
        let mut i = 0;
        while i < self.pending.len() {
            let start = i;
            if self.pending[i] == b'\\' {
                if i + 1 == self.pending.len() && !finish {
                    break;
                }
                if self.pending.get(i + 1) == Some(&b'u') {
                    if i + 6 > self.pending.len() && !finish {
                        break;
                    }
                    if i + 6 <= self.pending.len() {
                        let high = std::str::from_utf8(&self.pending[i + 2..i + 6])
                            .ok()
                            .and_then(|s| u16::from_str_radix(s, 16).ok());
                        let width = if high.is_some_and(|n| (0xd800..=0xdbff).contains(&n)) {
                            12
                        } else {
                            6
                        };
                        if i + width > self.pending.len() && !finish {
                            break;
                        }
                        if i + width <= self.pending.len() {
                            let mut quoted = vec![b'"'];
                            quoted.extend_from_slice(&self.pending[i..i + width]);
                            quoted.push(b'"');
                            if let Ok(s) = serde_json::from_slice::<String>(&quoted) {
                                for b in s.bytes() {
                                    decoded.push(b);
                                    starts.push(start);
                                }
                                i += width;
                                continue;
                            }
                        }
                    }
                } else if let Some(b) = self.pending.get(i + 1) {
                    let value = match b {
                        b'"' => Some(b'"'),
                        b'\\' => Some(b'\\'),
                        b'/' => Some(b'/'),
                        b'n' => Some(b'\n'),
                        b'r' => Some(b'\r'),
                        b't' => Some(b'\t'),
                        b'b' => Some(8),
                        b'f' => Some(12),
                        _ => None,
                    };
                    if let Some(b) = value {
                        decoded.push(b);
                        starts.push(start);
                        i += 2;
                        continue;
                    }
                }
            }
            decoded.push(self.pending[i]);
            starts.push(start);
            i += 1;
        }
        if decoded.windows(self.secret.len()).any(|v| v == self.secret) {
            return Err(fail("permission_denied"));
        }
        let mut safe = i;
        if !finish {
            for n in (1..self.secret.len().min(decoded.len() + 1)).rev() {
                if decoded.ends_with(&self.secret[..n]) {
                    safe = safe.min(starts[decoded.len() - n]);
                    break;
                }
            }
        }
        Ok(self.pending.drain(..safe).collect())
    }
}
