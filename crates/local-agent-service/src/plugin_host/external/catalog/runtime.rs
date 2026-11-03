use super::{Checked, Result, a, fail, records::Record};
use crate::{AppState, provider_models};
use local_agent_core::{
    domain::provider::{self as d, ProviderAdapter},
    providers::ConfigError,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};
use zeroize::Zeroizing;

// Worker execution produces advice; the same pure ProviderAdapter contract used
// by native formats validates and consumes that advice for this exact body.
struct Advice {
    request: d::CatalogRequest,
    page: a::Page,
    body_hash: String,
    protocol: &'static str,
}
impl ProviderAdapter for Advice {
    fn catalog_request(
        &self,
        _: &d::CatalogContext,
    ) -> std::result::Result<d::CatalogRequest, ConfigError> {
        Ok(self.request.clone())
    }
    fn parse_catalog(
        &self,
        body: &Value,
        emit: &mut dyn FnMut(d::CatalogModel) -> std::result::Result<(), ConfigError>,
    ) -> std::result::Result<d::CatalogContinuation, ConfigError> {
        if super::package::hash(&serde_json::to_vec(body).expect("catalog JSON")) != self.body_hash
        {
            return Err(d::invalid("目录候选范围已变化"));
        }
        for row in &self.page.models {
            emit(row.clone())?;
        }
        Ok(self.page.continuation.clone())
    }
    fn verification_protocol(&self) -> &'static str {
        self.protocol
    }
}
fn providers(s: &AppState, target: &a::Target) -> Result<Vec<local_agent_core::Provider>> {
    match target {
        a::Target::Saved { provider_id, .. } => Ok(s
            .store
            .lock()
            .checked("unavailable")?
            .provider_by_id(provider_id)
            .checked("unavailable")?
            .into_iter()
            .collect()),
        a::Target::Draft { .. } => Ok(vec![]),
    }
}
async fn check(s: &AppState, record: &Record) -> Result<()> {
    let state = s.clone();
    let record = record.clone();
    super::super::run_sync(move || {
        state
            .plugins
            .catalog_check(&record, &providers(&state, &record.target)?)
    })
    .await
}
pub(super) async fn read(s: AppState, q: a::Query, id: String, target: a::Target) -> Result<Value> {
    serde_json::to_value(read_catalog(s, q, id, target).await?).checked("invalid_input")
}
pub(super) async fn read_catalog(
    s: AppState,
    q: a::Query,
    id: String,
    target: a::Target,
) -> Result<provider_models::Catalog> {
    read_catalog_checked(s, q, id, target, || async { Ok(()) }).await
}
pub(super) async fn read_catalog_checked<C, F>(
    s: AppState,
    q: a::Query,
    id: String,
    target: a::Target,
    guard: C,
) -> Result<provider_models::Catalog>
where
    C: Fn() -> F,
    F: std::future::Future<Output = Result<()>>,
{
    let _permit = s
        .verification_capacity
        .clone()
        .try_acquire_owned()
        .checked("resource_limit")?;
    let state = s.clone();
    let query = q.clone();
    let (record, gate) = super::super::run_sync(move || {
        let record =
            state
                .plugins
                .catalog_record(&query, &id, &target, &providers(&state, &target)?)?;
        let gate = state.plugins.gate(&query.plugin_id)?;
        Ok((record, gate))
    })
    .await?;
    let _guard = gate.try_lock_owned().checked("resource_limit")?;
    let work = async {
        guard().await?;
        let secret = if let Some(reference) = &record.input.credential_ref {
            Some(
                crate::blocking::secret(s.credentials.clone(), reference.clone())
                    .await
                    .checked("permission_denied")?,
            )
        } else {
            None
        };
        check(&s, &record).await?;
        let mut url = local_agent_core::providers::validate_endpoint(&record.input.endpoint)
            .checked("invalid_input")?;
        let context = d::CatalogContext {
            protocol: record.input.protocol.clone(),
            vendor: record.input.vendor.clone(),
            base_path: url.path().into(),
            official_catalog_root: false,
        };
        // The guest only selects the signed suffix. Even a user endpoint that
        // embeds private path material never becomes guest request context.
        let guest_context = d::CatalogContext {
            protocol: context.protocol.clone(),
            vendor: String::new(),
            base_path: String::new(),
            official_catalog_root: false,
        };
        let relative: d::CatalogRequest = serde_json::from_value(
            s.plugins
                .invoke_catalog(
                    &q,
                    "provider.catalog.request",
                    serde_json::to_value(&guest_context).checked("invalid_input")?,
                )
                .await?,
        )
        .checked("invalid_input")?;
        check(&s, &record).await?;
        if !record.policy.accepts(&guest_context, &relative) {
            return Err(fail("permission_denied"));
        }
        let request = d::CatalogRequest {
            path: format!("{}{}", context.prefix(), relative.path),
        };
        if !record.policy.accepts(&context, &request) {
            return Err(fail("permission_denied"));
        }
        url.set_path(&request.path);
        let client =
            super::super::advanced::protocol::pinned_client(&url, record.allow_private).await?;
        check(&s, &record).await?;
        let source_url = url.to_string();
        let key = secret
            .as_deref()
            .map(|b| std::str::from_utf8(b))
            .transpose()
            .checked("permission_denied")?;
        let wire = crate::gateway::capabilities::builtin()
            .resolve(&record.input.protocol)
            .checked("unsupported")?;
        let mut next = url;
        let mut models = BTreeMap::new();
        let mut cursors = BTreeSet::new();
        let mut truncated = false;
        for page in 0..5 {
            guard().await?;
            check(&s, &record).await?;
            let mut call = client.get(next.clone()).timeout(Duration::from_secs(8));
            if let Some(key) = key {
                call = call.bearer_auth(key);
                if wire.api_key_header() {
                    call = call.header("x-api-key", key);
                }
            }
            for (name, value) in wire.headers() {
                call = call.header(*name, *value);
            }
            guard().await?;
            let mut response = call.send().await.checked("unavailable")?;
            if !response.status().is_success() {
                return Err(fail(if response.status().is_redirection() {
                    "catalog_redirect"
                } else {
                    "catalog_upstream"
                }));
            }
            if response
                .content_length()
                .is_some_and(|n| n > a::MAX_BODY as u64)
            {
                return Err(fail("resource_limit"));
            }
            let mut bytes = Zeroizing::new(Vec::new());
            while let Some(chunk) = response.chunk().await.checked("unavailable")? {
                guard().await?;
                check(&s, &record).await?;
                if bytes.len().saturating_add(chunk.len()) > a::MAX_BODY {
                    return Err(fail("resource_limit"));
                }
                bytes.extend_from_slice(&chunk);
            }
            if secret
                .as_ref()
                .is_some_and(|key| super::super::broker::body_contains_secret(&bytes, key))
            {
                return Err(fail("permission_denied"));
            }
            guard().await?;
            let body: Value = serde_json::from_slice(&bytes).checked("invalid_input")?;
            let snapshot = uuid::Uuid::new_v4().to_string();
            let input = a::ParseRequest {
                snapshot: snapshot.clone(),
                body: body.clone(),
            };
            let parsed: a::Page = serde_json::from_value(
                s.plugins
                    .invoke_catalog(
                        &q,
                        "provider.catalog.parse",
                        serde_json::to_value(input).checked("invalid_input")?,
                    )
                    .await?,
            )
            .checked("invalid_input")?;
            check(&s, &record).await?;
            if parsed.snapshot != snapshot || parsed.models.len() > a::MAX_PAGE_MODELS {
                return Err(fail("invalid_input"));
            }
            let protocol = match record.policy.base_protocol.as_str() {
                "messages" => "messages",
                "responses" => "responses",
                "chat-completions" => "chat-completions",
                _ => return Err(fail("unsupported")),
            };
            let advice = Advice {
                request: request.clone(),
                page: parsed,
                body_hash: super::package::hash(
                    &serde_json::to_vec(&body).checked("invalid_input")?,
                ),
                protocol,
            };
            if !record.policy.accepts(
                &context,
                &advice.catalog_request(&context).checked("invalid_input")?,
            ) {
                return Err(fail("permission_denied"));
            }
            let (cut, continuation) =
                provider_models::read_models_with(&advice, &body, &mut models, key)
                    .checked("invalid_input")?;
            truncated |= cut;
            if !continuation.has_more {
                break;
            }
            if page == 4 || models.len() >= 2000 {
                truncated = true;
                break;
            }
            let cursor = continuation
                .after_id
                .filter(|v| provider_models::safe_label(v, 200, key))
                .ok_or(fail("invalid_input"))?;
            if !cursors.insert(cursor.clone()) {
                return Err(fail("invalid_input"));
            }
            next.query_pairs_mut()
                .clear()
                .append_pair("after_id", &cursor);
        }
        let mut result =
            provider_models::external_catalog(&record.input, models, source_url, truncated);
        let state = s.clone();
        let input = record.input.clone();
        let observations = super::super::run_sync(move || {
            let manager = state.providers.lock().checked("unavailable")?;
            Ok(manager.model_observations(
                &input.endpoint,
                &input.protocol,
                input.credential_ref.as_deref(),
            ))
        })
        .await?;
        result.observations(observations);
        check(&s, &record).await?;
        let state = s.clone();
        let query = q.clone();
        super::super::run_sync(move || state.plugins.catalog_plugin(&query).map(|_| ())).await?;
        check(&s, &record).await?;
        guard().await?;
        Ok(result)
    };
    let revoked = async {
        loop {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let manager = s.plugins.clone();
            let record = record.clone();
            super::super::run_sync(move || manager.catalog_lease(&record)).await?;
        }
    };
    tokio::select! {result=tokio::time::timeout(Duration::from_secs(15),work)=>result.checked("deadline_exceeded")?,result=revoked=>result}
}
