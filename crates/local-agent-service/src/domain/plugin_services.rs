//! Versioned, explicitly granted cross-plugin reads reuse domain admission and
//! source leases. Consumers never receive another plugin's store or resources.
use super::query;
use crate::{
    AppState,
    plugin_host::external::{self, Checked, Result, fail},
};
use local_agent_core::{
    domain as d,
    plugin::{
        contributions::DataQuery,
        external::{GatewayReport, SourceReport, Status},
    },
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn input(state: &AppState, source: &str) -> Result<d::UnifiedSessionQuery> {
    let directory = query::directory(state)?;
    let entry = directory
        .catalog
        .sources
        .iter()
        .find(|s| s.id == source)
        .ok_or(fail("dependency_missing"))?;
    if entry.status.authorization != d::Authorization::Authorized {
        return Err(fail("permission_denied"));
    }
    if !entry.status.runtime_active {
        return Err(fail("dependency_missing"));
    }
    let provider = directory
        .catalog
        .providers
        .iter()
        .find(|p| p.provider_key == entry.provider_key)
        .ok_or(fail("dependency_missing"))?;
    Ok(d::UnifiedSessionQuery {
        scope: d::QueryScope {
            kind: if provider.execution_kind == d::ExecutionKind::Builtin {
                d::ScopeKind::Builtin
            } else {
                d::ScopeKind::External
            },
            source_ids: vec![source.into()],
        },
        filter: d::SessionFilter::default(),
        snapshot_token: None,
        cursor: None,
    })
}
fn lease(state: &AppState, query: &d::UnifiedSessionQuery) -> Result<String> {
    let directory = query::directory(state)?;
    let source = directory
        .catalog
        .sources
        .iter()
        .find(|s| query.scope.source_ids.contains(&s.id))
        .ok_or(fail("dependency_missing"))?;
    let provider = directory
        .catalog
        .providers
        .iter()
        .find(|p| p.provider_key == source.provider_key)
        .ok_or(fail("dependency_missing"))?;
    let vector = serde_json::json!([
        provider,
        source.status.authorization,
        source.status.grant_revision,
        source.status.runtime_active
    ]);
    Ok(format!(
        "{:x}",
        Sha256::digest(vector.to_string().as_bytes())
    ))
}
pub(crate) async fn refresh_plugin_report(
    state: &AppState,
    before: Status,
    view_id: Option<String>,
) -> Result<Value> {
    let state = state.clone();
    external::run_sync(move || {
        let bindings = state
            .plugins
            .report_queries(&before.manifest.id, &before.digest)?;
        if let Some(view_id) = view_id {
            let binding = bindings
                .iter()
                .find(|binding| binding.view_id == view_id)
                .ok_or(fail("invalid_input"))?;
            refresh_view(&state, &before, binding)?;
        } else {
            // Older callers omit the view ID. Publish each readable view independently.
            let mut result = None;
            for binding in &bindings {
                let next = refresh_view(&state, &before, binding);
                if result.is_none() || next.is_ok() {
                    result = Some(next);
                }
            }
            result.ok_or(fail("invalid_input"))??;
        }
        Ok(json!({"refreshed":true}))
    })
    .await
}

fn refresh_view(state: &AppState, before: &Status, binding: &DataQuery) -> Result<()> {
    let query = input(state, &binding.source_id)?;
    let before_lease = lease(state, &query)?;
    let page = query::sessions(state, query.clone())?;
    let summary = page
        .source_summaries
        .iter()
        .find(|s| s.source_id == binding.source_id)
        .ok_or(fail("unavailable"))?;
    let source_name = query::directory(state)?
        .catalog
        .products
        .iter()
        .filter(|p| p.source_id == binding.source_id)
        .map(|p| p.display_name.clone())
        .collect::<Vec<_>>()
        .join(" / ");
    if lease(state, &query)? != before_lease {
        return Err(fail("grant_revoked"));
    }
    let now = chrono::Utc::now().to_rfc3339();
    state.plugins.publish_report(
        before,
        std::slice::from_ref(&binding.view_id),
        GatewayReport {
            requests: "0".into(),
            attempts: "0".into(),
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
            range_start: now.clone(),
            range_end: now,
            sources: vec![SourceReport {
                source_id: binding.source_id.clone(),
                source_name,
                sessions: summary.sessions.value().to_string(),
                total_tokens: summary
                    .usage
                    .total_tokens
                    .as_ref()
                    .map(|n| n.value().to_string()),
                lease: before_lease,
                coverage: serde_json::to_value(page.coverage)
                    .checked("invalid_record")?
                    .as_str()
                    .ok_or(fail("invalid_record"))?
                    .into(),
            }],
        },
    )
}
pub(crate) fn validate_plugin_report(state: &AppState, value: &Value) -> Result<()> {
    let Some(sources) = value["report"]["sources"].as_array() else {
        return Ok(());
    };
    for source in sources {
        let query = input(
            state,
            source["sourceId"].as_str().ok_or(fail("invalid_record"))?,
        )?;
        if source["lease"].as_str() != Some(lease(state, &query)?.as_str()) {
            return Err(fail("stale_snapshot"));
        }
    }
    Ok(())
}
